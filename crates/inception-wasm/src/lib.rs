//! JavaScript `Error` projection for [`inception::InceptionError`].
//!
//! This crate is the WASM boundary companion to the target-independent
//! `inception` crate. It creates ordinary JavaScript errors; it does not define
//! a wire format or serialize an error.
//!
//! ```no_run
//! # fn operation() -> Result<(), ExampleError> { Ok(()) }
//! # #[inception::error]
//! # enum ExampleError {
//! #     #[code("example.failed")]
//! #     #[description("The example failed.")]
//! #     Failed,
//! # }
//! # fn export() -> Result<(), wasm_bindgen::JsValue> {
//! operation().map_err(inception_wasm::to_js_error)?;
//! # Ok(())
//! # }
//! ```
//!
//! In an ordinary JavaScript realm, each projected error has these own
//! properties:
//!
//! - `message`: the layer's safe [`inception::InceptionError::trace_message`]
//! - `code` and `description`: stable code and static safe description
//! - `location`: `{ file, line, column }`
//! - `entries`: an ordered array of `{ key, value }`; hidden entries are absent
//! - `cause`: the nested Inception errors, linked in outer-to-inner order
//! - `causeTruncated`: `"cycle"` or `"depth-limit"` on the last included
//!   error when the remaining cause chain cannot safely be projected
//!
//! Ordinary external sources from [`inception::Inceptioned`] are deliberately
//! never inspected, so their messages cannot cross the FFI boundary.
//!
//! Projection is best-effort in a hostile realm. The native `Error` constructor
//! is captured when the WASM module initializes. After a base error exists,
//! every JavaScript enrichment operation is caught; a rejected operation omits
//! only its optional field or substructure, never replacing the base error with
//! the thrown value. If the captured constructor itself cannot create the base
//! error, projection returns the safe trace message string instead.

use inception::{InceptionError, Location};
use js_sys::{Array, Error, Object};
use wasm_bindgen::{JsValue, prelude::wasm_bindgen};

#[wasm_bindgen(inline_js = r#"
const InceptionErrorConstructor = Error;

export function inception_new_error(message) {
    return new InceptionErrorConstructor(message);
}

export function inception_new_object() {
    return new Object();
}

export function inception_new_array() {
    return new Array();
}

export function inception_define_data_property(target, key, value) {
    return Reflect.defineProperty(target, key, {
        __proto__: null,
        value,
        writable: true,
        enumerable: true,
        configurable: true,
    });
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    fn inception_new_error(message: &str) -> Result<Error, JsValue>;

    #[wasm_bindgen(catch)]
    fn inception_new_object() -> Result<Object, JsValue>;

    #[wasm_bindgen(catch)]
    fn inception_new_array() -> Result<Array, JsValue>;

    #[wasm_bindgen(catch)]
    fn inception_define_data_property(
        target: &JsValue,
        key: &str,
        value: &JsValue,
    ) -> Result<bool, JsValue>;
}

/// Maximum number of nested causes projected after the outer error.
///
/// Longer chains retain the first [`MAX_CAUSE_DEPTH`] real causes and mark the
/// last one with `causeTruncated = "depth-limit"`. Cycles are marked with
/// `causeTruncated = "cycle"` instead. No synthetic error layer is created.
pub const MAX_CAUSE_DEPTH: usize = 64;

/// Converts an owned Inception error into a structured JavaScript `Error`.
///
/// This signature is intended for direct use with [`Result::map_err`]. The
/// projection is best-effort: a JavaScript environment that rejects an added
/// property or optional container still receives the base `Error`. The native
/// `Error` constructor is captured when this WASM module initializes so later
/// global replacement cannot change the error type. If even that constructor
/// rejects construction, the safe trace message string is returned instead.
#[must_use]
pub fn to_js_error(error: impl InceptionError) -> JsValue {
    let projected = to_js_error_ref(&error);
    drop(error);
    projected
}

/// Converts any borrowed Inception error trait object into a structured
/// JavaScript `Error`.
///
/// Projection reads the error's recorded creation location; it never captures
/// the adapter call site.
#[must_use]
pub fn to_js_error_ref(error: &dyn InceptionError) -> JsValue {
    project(error)
}

fn project(error: &dyn InceptionError) -> JsValue {
    let (layers, truncated) = bounded_layers(error);
    let message = error.trace_message();
    let Ok(outer) = project_layer(error) else {
        return JsValue::from_str(message.as_ref());
    };
    let projected_causes: Vec<Error> = layers
        .into_iter()
        .skip(1)
        .map_while(|layer| project_layer(layer).ok())
        .collect();

    if let Some(first_cause) = projected_causes.first() {
        define_data_property(outer.as_ref(), "cause", first_cause.as_ref());
    }
    for pair in projected_causes.windows(2) {
        define_data_property(pair[0].as_ref(), "cause", pair[1].as_ref());
    }
    if let Some(reason) = truncated {
        let last = projected_causes.last().unwrap_or(&outer);
        define_data_property(last.as_ref(), "causeTruncated", &JsValue::from_str(reason));
    }

    outer.into()
}

fn bounded_layers(error: &dyn InceptionError) -> (Vec<&dyn InceptionError>, Option<&'static str>) {
    let mut layers = Vec::with_capacity(MAX_CAUSE_DEPTH + 1);
    let mut visited: Vec<*const dyn InceptionError> = Vec::with_capacity(MAX_CAUSE_DEPTH + 1);
    let mut current = error;

    loop {
        let pointer = core::ptr::from_ref(current);
        visited.push(pointer);
        layers.push(current);

        let Some(next) = current.nested_error() else {
            return (layers, None);
        };
        let next_pointer = core::ptr::from_ref(next);
        if visited
            .iter()
            .any(|visited_pointer| core::ptr::eq(*visited_pointer, next_pointer))
        {
            return (layers, Some("cycle"));
        }
        if layers.len() > MAX_CAUSE_DEPTH {
            return (layers, Some("depth-limit"));
        }
        current = next;
    }
}

fn project_layer(error: &dyn InceptionError) -> Result<Error, JsValue> {
    let projected = inception_new_error(error.trace_message().as_ref())?;
    let target: &JsValue = projected.as_ref();

    define_data_property(target, "code", &JsValue::from_str(error.code()));
    define_data_property(
        target,
        "description",
        &JsValue::from_str(InceptionError::description(error)),
    );
    if let Some(location) = location(error.created_at()) {
        define_data_property(target, "location", &location);
    }
    if let Some(entries) = entries(error) {
        define_data_property(target, "entries", &entries);
    }

    Ok(projected)
}

fn location(location: Location) -> Option<JsValue> {
    let object = inception_new_object().ok()?;
    let target: &JsValue = object.as_ref();
    define_data_property(target, "file", &JsValue::from_str(location.file()));
    define_data_property(
        target,
        "line",
        &JsValue::from_f64(f64::from(location.line())),
    );
    define_data_property(
        target,
        "column",
        &JsValue::from_f64(f64::from(location.column())),
    );
    Some(object.into())
}

fn entries(error: &dyn InceptionError) -> Option<JsValue> {
    let visible = inception_new_array().ok()?;
    let mut index = 0;
    for entry in error
        .entries()
        .into_iter()
        .filter(|entry| !entry.is_hidden())
    {
        let Ok(object) = inception_new_object() else {
            continue;
        };
        let target: &JsValue = object.as_ref();
        if define_data_property(target, "key", &JsValue::from_str(entry.key()))
            && define_data_property(target, "value", &JsValue::from_str(entry.value()))
            && define_data_property(visible.as_ref(), &index.to_string(), object.as_ref())
        {
            index += 1;
        }
    }
    Some(visible.into())
}

fn define_data_property(target: &JsValue, key: &str, value: &JsValue) -> bool {
    // The JavaScript helper creates its descriptor with literal `__proto__:
    // null`, so neither `Object.create` nor an inherited setter participates.
    // Its caught binding contains descriptor construction and definition in a
    // single best-effort operation.
    inception_define_data_property(target, key, value).unwrap_or(false)
}
