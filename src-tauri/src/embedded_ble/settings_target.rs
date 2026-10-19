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
    #[cfg(target_os = "windows")]
    {
        super::windows_ble::settings_device_peer_key(device)
    }
    #[cfg(not(target_os = "windows"))]
    {
        device.id.clone()
    }
}

/// The transaction expires before the async IPC timeout. It cannot keep issuing
/// mutation packets after the caller has already received a timeout/error.
pub(crate) fn bounded<T>(
    io: &mut dyn SettingsIo,
    deadline: std::time::Instant,
    task: impl FnOnce(&mut dyn SettingsIo) -> Result<T, String>,
) -> Result<T, String> {
    struct Bounded<'a> {
        inner: &'a mut dyn SettingsIo,
        deadline: std::time::Instant,
    }
    impl Bounded<'_> {
        fn check(&self) -> Result<(), String> {
            if std::time::Instant::now() >= self.deadline {
                Err("Selected-device transaction expired. Read this device to confirm its saved settings; no automatic retry was started.".into())
            } else {
                Ok(())
            }
        }
    }
    impl SettingsIo for Bounded<'_> {
        fn read(&mut self) -> Result<DeviceSettingsStatus, String> {
            self.check()?;
            let value = self.inner.read()?;
            self.check()?;
            Ok(value)
        }
        fn write(&mut self, command: &str) -> Result<(), String> {
            self.check()?;
            let value = self.inner.write(command);
            self.check()?;
            value
        }
    }
    task(&mut Bounded {
        inner: io,
        deadline,
    })
}
