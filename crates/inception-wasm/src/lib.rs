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
//! - `cause`: the recursively projected nested Inception error, when present
//!
//! Ordinary external sources from [`inception::Inceptioned`] are deliberately
//! never inspected, so their messages cannot cross the FFI boundary.

use inception::{InceptionError, Location};
use js_sys::{Array, Error, Object, Reflect};
use wasm_bindgen::JsValue;

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
    let projected = Error::new(error.trace_message().as_ref());
    let target: &JsValue = projected.as_ref();

    set(target, "code", &JsValue::from_str(error.code()));
    set(
        target,
        "description",
        &JsValue::from_str(InceptionError::description(error)),
    );
    set(target, "location", &location(error.created_at()));
    set(target, "entries", &entries(error));

    if let Some(cause) = error.nested_error() {
        set(target, "cause", project(cause).as_ref());
    }

    projected
}

fn location(location: Location) -> JsValue {
    let object = Object::new();
    let target: &JsValue = object.as_ref();
    set(target, "file", &JsValue::from_str(location.file()));
    set(
        target,
        "line",
        &JsValue::from_f64(f64::from(location.line())),
    );
    set(
        target,
        "column",
        &JsValue::from_f64(f64::from(location.column())),
    );
    object.into()
}

fn entries(error: &dyn InceptionError) -> JsValue {
    let visible = Array::new();
    for entry in error
        .entries()
        .into_iter()
        .filter(|entry| !entry.is_hidden())
    {
        let object = Object::new();
        let target: &JsValue = object.as_ref();
        set(target, "key", &JsValue::from_str(entry.key()));
        set(target, "value", &JsValue::from_str(entry.value()));
        visible.push(&object);
    }
    visible.into()
}

fn set(target: &JsValue, key: &str, value: &JsValue) {
    // A fresh built-in Error/Object/Array normally accepts these properties.
    // Ignore host exceptions to keep error conversion infallible during an
    // already-failing operation.
    let _ = Reflect::set(target, &JsValue::from_str(key), value);
}
