//! KUI/GPUI frontend for the DeepSeek Harness ACP desktop client.

#[cfg(any(target_family = "wasm", test))]
mod files;
#[cfg(any(target_family = "wasm", test))]
mod sessions;
pub mod state;

#[cfg(target_family = "wasm")]
mod bridge;
#[cfg(target_family = "wasm")]
mod web;
#[cfg(target_family = "wasm")]
mod welcome;

#[cfg(target_family = "wasm")]
pub use web::start;

#[cfg(not(target_family = "wasm"))]
/// Host-target placeholder; the executable frontend targets wasm32-unknown-unknown.
pub fn wasm_only() {}
