use wasm_bindgen::prelude::*;

/// Minimal deterministic payload for the worker-celld Wasm qualification path.
#[wasm_bindgen]
pub fn agent_card() -> String {
    r#"{"runtime":"worker-celld","language":"rust-wasm","capabilities":["wasm.module"]}"#.into()
}
