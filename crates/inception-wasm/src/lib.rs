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
//! Each projected error has these own properties:
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

use inception::{InceptionError, Location};
use js_sys::{Array, Error, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};

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
/// property still receives the base `Error`.
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
    project(error).into()
}

fn project(error: &dyn InceptionError) -> Error {
    let (layers, truncated) = bounded_layers(error);
    let outer = project_layer(error);
    let projected_causes: Vec<Error> = layers.into_iter().skip(1).map(project_layer).collect();

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

    outer
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

fn project_layer(error: &dyn InceptionError) -> Error {
    let projected = Error::new(error.trace_message().as_ref());
    let target: &JsValue = projected.as_ref();

    define_data_property(target, "code", &JsValue::from_str(error.code()));
    define_data_property(
        target,
        "description",
        &JsValue::from_str(InceptionError::description(error)),
    );
    define_data_property(target, "location", &location(error.created_at()));
    define_data_property(target, "entries", &entries(error));

    projected
}

fn location(location: Location) -> JsValue {
    let object = Object::new();
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
    object.into()
}

fn entries(error: &dyn InceptionError) -> JsValue {
    let visible = Array::new();
    for (index, entry) in error
        .entries()
        .into_iter()
        .filter(|entry| !entry.is_hidden())
        .enumerate()
    {
        let object = Object::new();
        let target: &JsValue = object.as_ref();
        define_data_property(target, "key", &JsValue::from_str(entry.key()));
        define_data_property(target, "value", &JsValue::from_str(entry.value()));
        define_data_property(visible.as_ref(), &index.to_string(), object.as_ref());
    }
    visible.into()
}

fn define_data_property(target: &JsValue, key: &str, value: &JsValue) {
    // A null-prototype descriptor cannot inherit a poisoned setter. Every
    // operation which can invoke JavaScript is caught by `Reflect` and ignored
    // so optional structure remains best-effort during an existing failure.
    let descriptor = Object::create(JsValue::NULL.unchecked_ref());
    let descriptor_value: &JsValue = descriptor.as_ref();
    if !reflect_set(descriptor_value, "value", value)
        || !reflect_set(descriptor_value, "writable", &JsValue::TRUE)
        || !reflect_set(descriptor_value, "enumerable", &JsValue::TRUE)
        || !reflect_set(descriptor_value, "configurable", &JsValue::TRUE)
    {
        return;
    }
    let _ = Reflect::define_property(
        target.unchecked_ref::<Object>(),
        &JsValue::from_str(key),
        &descriptor,
    );
}

fn reflect_set(target: &JsValue, key: &str, value: &JsValue) -> bool {
    Reflect::set(target, &JsValue::from_str(key), value).unwrap_or(false)
}
