//! KUI/GPUI frontend for KWork.

#[cfg(any(target_family = "wasm", test))]
pub mod anim;
#[cfg(any(target_family = "wasm", test))]
mod commands;
#[cfg(any(target_family = "wasm", test))]
mod files;
pub mod history;
#[cfg(any(target_family = "wasm", test))]
mod markdown;
pub mod process;
mod questions;
#[cfg(any(target_family = "wasm", test))]
mod right_dock;
#[cfg(any(target_family = "wasm", test))]
mod sessions;
#[cfg(any(target_family = "wasm", test))]
mod shortcuts;
pub mod state;
#[path = "../../shared/submitted_plan.rs"]
mod submitted_plan;
#[cfg(any(target_family = "wasm", test))]
mod terminal_keys;
#[cfg(any(target_family = "wasm", test))]
pub mod tokens;
mod tool_calls;

#[cfg(target_family = "wasm")]
mod brand;
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
