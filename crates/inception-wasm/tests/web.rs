#![cfg(target_arch = "wasm32")]

use inception::{InceptionError as _, Location};
use js_sys::{Array, Error, Function, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::wasm_bindgen_test;

#[inception::error]
enum InnerError {
    #[code("inner.failed")]
    #[description("The inner operation failed.")]
    Failed { resource: &'static str },
}

#[inception::error]
enum OuterError {
    #[code("outer.failed")]
    #[description("The outer operation failed.")]
    Failed {
        #[caused_by(inception)]
        source: InnerError,
        attempt: u32,
        #[hide]
        secret: &'static str,
    },
}

fn get(target: &JsValue, key: &str) -> JsValue {
    Reflect::get(target, &JsValue::from_str(key)).expect("property is readable")
}

#[wasm_bindgen_test]
fn projects_structure_chain_and_visible_entries() {
    let inner = InnerError::failed("mailbox");
    let original = OuterError::failed(inner, 3, "never-crosses");
    let outer_location = original.created_at();
    let inner_location = original
        .nested_error()
        .expect("outer error has nested cause")
        .created_at();
    let outer_message = original.trace_message();

    let value = inception_wasm::to_js_error_ref(&original);
    let error = value.dyn_ref::<Error>().expect("projection is an Error");
    assert_eq!(error.message(), outer_message.as_ref());
    assert_eq!(
        get(&value, "code").as_string().as_deref(),
        Some("outer.failed")
    );
    assert_eq!(
        get(&value, "description").as_string().as_deref(),
        Some("The outer operation failed.")
    );

    let location = get(&value, "location");
    assert_eq!(
        get(&location, "file").as_string().as_deref(),
        Some(outer_location.file())
    );
    assert_eq!(
        get(&location, "line").as_f64(),
        Some(f64::from(outer_location.line()))
    );
    assert_eq!(
        get(&location, "column").as_f64(),
        Some(f64::from(outer_location.column()))
    );

    let entries = Array::from(&get(&value, "entries"));
    assert_eq!(entries.length(), 1, "hidden entry must be omitted entirely");
    let entry = entries.get(0);
    assert_eq!(get(&entry, "key").as_string().as_deref(), Some("attempt"));
    assert_eq!(get(&entry, "value").as_string().as_deref(), Some("3"));
    assert!(!format!("{value:?}").contains("never-crosses"));

    let cause = get(&value, "cause");
    assert!(cause.is_instance_of::<Error>());
    assert_eq!(
        get(&cause, "code").as_string().as_deref(),
        Some("inner.failed")
    );
    let cause_location = get(&cause, "location");
    assert_eq!(
        get(&cause_location, "file").as_string().as_deref(),
        Some(inner_location.file())
    );
    assert_eq!(
        get(&cause_location, "line").as_f64(),
        Some(f64::from(inner_location.line()))
    );
    assert_eq!(
        get(&cause_location, "column").as_f64(),
        Some(f64::from(inner_location.column()))
    );
    let cause_entries = Array::from(&get(&cause, "entries"));
    assert_eq!(cause_entries.length(), 1);
    assert_eq!(
        get(&cause_entries.get(0), "value").as_string().as_deref(),
        Some("mailbox")
    );
}

#[derive(Debug)]
struct SecretExternal;

impl core::fmt::Display for SecretExternal {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("external-secret-message")
    }
}

impl core::error::Error for SecretExternal {}

#[wasm_bindgen_test]
fn external_source_message_is_not_exposed() {
    let external =
        inception::Inceptioned::new_at(SecretExternal, Location::new("boundary.rs", 9, 2));
    let value = inception_wasm::to_js_error(external);
    let error = value.unchecked_ref::<Error>();

    assert_eq!(error.message(), "External error (web::SecretExternal)");
    assert!(!format!("{value:?}").contains("external-secret-message"));
    assert!(get(&value, "cause").is_undefined());
}

#[wasm_bindgen_test]
fn owned_conversion_is_usable_as_map_err() {
    let result: Result<(), InnerError> = Err(InnerError::failed("item"));
    let projected: Result<(), JsValue> = result.map_err(inception_wasm::to_js_error);
    assert!(projected.expect_err("must fail").is_instance_of::<Error>());
}

#[derive(Debug)]
struct SelfCycle;

impl core::fmt::Display for SelfCycle {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("safe cycle")
    }
}

impl core::error::Error for SelfCycle {}

impl inception::InceptionError for SelfCycle {
    fn code(&self) -> &'static str {
        "cycle.self"
    }

    fn description(&self) -> &'static str {
        "A cycle was detected."
    }

    fn created_at(&self) -> Location {
        Location::new("cycle.rs", 1, 1)
    }

    fn nested_error(&self) -> Option<&(dyn inception::InceptionError + 'static)> {
        Some(self)
    }

    fn entries(&self) -> Vec<inception::Entry> {
        vec![inception::Entry::display("kind", &"self")]
    }
}

static SELF_CYCLE: SelfCycle = SelfCycle;

#[wasm_bindgen_test]
fn self_cycle_is_bounded_without_a_synthetic_cause() {
    let value = inception_wasm::to_js_error_ref(&SELF_CYCLE);
    assert!(value.is_instance_of::<Error>());
    assert_eq!(
        get(&value, "code").as_string().as_deref(),
        Some("cycle.self")
    );
    assert_eq!(
        get(&value, "causeTruncated").as_string().as_deref(),
        Some("cycle")
    );
    assert!(get(&value, "cause").is_undefined());
}

#[inception::error]
enum DeepError {
    #[code("deep.terminal")]
    #[description("The chain ended.")]
    Terminal,
    #[code("deep.link")]
    #[description("The chain continues.")]
    Link {
        #[caused_by(inception)]
        source: Box<DeepError>,
        depth: usize,
    },
}

#[wasm_bindgen_test]
fn owned_derived_chain_is_cut_at_the_documented_limit() {
    let mut chain = DeepError::terminal();
    for depth in 0..(inception_wasm::MAX_CAUSE_DEPTH + 5) {
        chain = DeepError::link(Box::new(chain), depth);
    }

    let outer = inception_wasm::to_js_error(chain);
    let mut current = outer;
    let mut count = 1;
    while !get(&current, "cause").is_undefined() {
        current = get(&current, "cause");
        count += 1;
    }

    assert_eq!(count, inception_wasm::MAX_CAUSE_DEPTH + 1);
    assert_eq!(
        get(&current, "causeTruncated").as_string().as_deref(),
        Some("depth-limit")
    );
    assert_eq!(
        get(&current, "code").as_string().as_deref(),
        Some("deep.link")
    );
}

struct RestoreArrayIndex {
    prototype: Object,
    previous: JsValue,
}

impl Drop for RestoreArrayIndex {
    fn drop(&mut self) {
        let key = JsValue::from_str("0");
        if self.previous.is_undefined() {
            let _ = Reflect::delete_property(&self.prototype, &key);
        } else {
            let _ = Reflect::define_property(
                &self.prototype,
                &key,
                self.previous.unchecked_ref::<Object>(),
            );
        }
    }
}

#[wasm_bindgen_test]
fn poisoned_array_index_setter_cannot_escape_projection() {
    let array_constructor = get(&js_sys::global(), "Array");
    let prototype: Object = get(&array_constructor, "prototype").unchecked_into();
    let key = JsValue::from_str("0");
    let previous = Object::get_own_property_descriptor(&prototype, &key);
    let _restore = RestoreArrayIndex {
        prototype: prototype.clone(),
        previous,
    };

    let descriptor = Object::new();
    Reflect::set(
        &descriptor,
        &JsValue::from_str("set"),
        &Function::new_no_args("throw new Error('poisoned array index')"),
    )
    .expect("descriptor accepts setter");
    Reflect::set(
        &descriptor,
        &JsValue::from_str("configurable"),
        &JsValue::TRUE,
    )
    .expect("descriptor accepts configurable flag");
    assert!(
        Reflect::define_property(&prototype, &key, &descriptor)
            .expect("prototype property can be defined")
    );

    let value = inception_wasm::to_js_error_ref(&SELF_CYCLE);
    assert!(value.is_instance_of::<Error>());
    assert_eq!(
        get(&value, "code").as_string().as_deref(),
        Some("cycle.self")
    );
    assert_eq!(
        get(&value, "description").as_string().as_deref(),
        Some("A cycle was detected.")
    );
    let entries = Array::from(&get(&value, "entries"));
    assert_eq!(entries.length(), 1);
    assert_eq!(
        get(&entries.get(0), "value").as_string().as_deref(),
        Some("self")
    );
}
