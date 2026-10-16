//! Portable browser model operations and optional material authoring tools.
mod models;
pub use models::*;
#[cfg(feature = "material-tools")]
mod material_api;
#[cfg(feature = "material-tools")]
pub use material_api::*;

use serde::de::DeserializeOwned;
use usd_toolbox_core::Input;
use wasm_bindgen::prelude::*;

/// Export bytes plus a typed JavaScript loss array.
#[wasm_bindgen]
pub struct WasmExport {
    bytes: Vec<u8>,
    losses: JsValue,
}

#[wasm_bindgen]
impl WasmExport {
    /// Returns encoded bytes as a JavaScript `Uint8Array` copy.
    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// Moves the encoded payload out once, avoiding an extra Rust-side clone.
    /// Subsequent calls return an empty array; JavaScript owns the returned copy.
    pub fn take_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.bytes)
    }

    /// Returns the deterministic structured loss report.
    #[wasm_bindgen(getter)]
    pub fn losses(&self) -> JsValue {
        self.losses.clone()
    }
}

fn options_from_js<T: Default + DeserializeOwned>(value: Option<JsValue>) -> Result<T, JsError> {
    match value {
        Some(value) if !value.is_null() && !value.is_undefined() => from_js(value),
        Some(_) | None => Ok(T::default()),
    }
}

fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(js_error)
}

#[cfg(feature = "material-tools")]
fn to_js<T: serde::Serialize + ?Sized>(value: &T) -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(value).map_err(js_error)
}

fn js_error(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}
