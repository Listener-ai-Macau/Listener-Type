//! Explicit device-settings destinations. Voice capture keeps its own target.
use super::DeviceSettingsStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDevice {
    /// Opaque transport identity, never a display name.
    pub id: String,
    pub transport: String,
    pub label: String,
}

pub trait SettingsIo {
    fn read(&mut self) -> Result<DeviceSettingsStatus, String>;
    fn write(&mut self, command: &str) -> Result<(), String>;
}

pub fn list() -> Result<Vec<SettingsDevice>, String> {
    #[cfg(target_os = "windows")]
    {
        super::windows_ble::list_settings_devices()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("Device selection is currently supported on Windows.".into())
    }
}

pub fn with_device<T>(
    device: &SettingsDevice,
    task: impl FnOnce(&mut dyn SettingsIo) -> Result<T, String>,
) -> Result<T, String> {
    #[cfg(target_os = "windows")]
    {
        super::windows_ble::with_settings_device(device, task)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (device, task);
        Err("Device selection is currently supported on Windows.".into())
    }
}

pub fn peer_key(device: &SettingsDevice) -> String {
    #[cfg(target_os="windows")] { super::windows_ble::settings_device_peer_key(device) }
    #[cfg(not(target_os="windows"))] { device.id.clone() }
}
