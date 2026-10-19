//! Device / BLE / firmware domain implementations.
//! Thin Tauri wrappers remain in `commands/mod.rs`.

mod ble;
mod firmware;
mod settings;
mod settings_selection;

pub use ble::*;
pub use firmware::*;
pub use settings::*;
pub use settings_selection::*;
