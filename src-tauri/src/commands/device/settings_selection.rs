//! The settings page configures an explicit device, independently of voice input.
use super::settings::*;
use crate::embedded_ble::settings_target::{self, SettingsDevice, SettingsIo};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedDeviceSettings {
    pub device: SettingsDevice,
    pub revision: u32,
    pub read_token: String,
    pub snapshot: DeviceSettingsSnapshot,
}

struct ReadLease {
    id: String,
    revision: u32,
    at: Instant,
}
fn read_leases() -> &'static Mutex<HashMap<String, ReadLease>> {
    static LEASES: OnceLock<Mutex<HashMap<String, ReadLease>>> = OnceLock::new();
    LEASES.get_or_init(|| Mutex::new(HashMap::new()))
}
fn issue_read_token(device: &SettingsDevice, revision: u32) -> String {
    let mut leases = read_leases().lock().unwrap_or_else(|p| p.into_inner());
    leases.retain(|_, lease| lease.at.elapsed() < Duration::from_secs(600));
    if leases.len() >= 64 {
        if let Some(oldest) = leases
            .iter()
            .min_by_key(|(_, l)| l.at)
            .map(|(k, _)| k.clone())
        {
            leases.remove(&oldest);
        }
    }
    let token = uuid::Uuid::new_v4().to_string();
    leases.insert(
        token.clone(),
        ReadLease {
            id: device.id.clone(),
            revision,
            at: Instant::now(),
        },
    );
    token
}
fn claim_read_token(token: &str, device: &SettingsDevice, revision: u32) -> Result<(), String> {
    let lease = read_leases()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(token)
        .ok_or("Read this selected device before saving its settings.")?;
    if lease.id != device.id
        || lease.revision != revision
        || lease.at.elapsed() >= Duration::from_secs(600)
    {
        return Err("The settings form belongs to a different device or an expired read. Read the selected device again.".into());
    }
    Ok(())
}

pub async fn list_settings_devices() -> Result<Vec<SettingsDevice>, String> {
    run_device_settings_blocking("device list", settings_target::list).await
}

pub async fn get_selected_device_settings(
    device: SettingsDevice,
) -> Result<SelectedDeviceSettings, String> {
    run_device_settings_blocking("selected readback", move || {
        settings_target::with_device(&device, |io| {
            let status = io.read()?;
            Ok(SelectedDeviceSettings {
                read_token: issue_read_token(&device, status.settings_revision),
                revision: status.settings_revision,
                snapshot: device_settings_snapshot_from_status(status),
                device: device.clone(),
            })
        })
    })
    .await
}

pub async fn set_selected_device_settings(
    device: SettingsDevice,
    revision: u32,
    read_token: String,
    request: DeviceSettingsUpdateRequest,
) -> Result<SelectedDeviceSettings, String> {
    validate_device_settings_request(&request)?;
    claim_read_token(&read_token, &device, revision)?;
    run_device_settings_blocking("selected write", move || {
        settings_target::with_device(&device, |io| {
            write_selected_settings(io, revision, &request).map(|(revision, snapshot)| {
                SelectedDeviceSettings {
                    read_token: issue_read_token(&device, revision),
                    device: device.clone(),
                    revision,
                    snapshot,
                }
            })
        })
    })
    .await
}

fn write_selected_settings(
    io: &mut dyn SettingsIo,
    expected_revision: u32,
    request: &DeviceSettingsUpdateRequest,
) -> Result<(u32, DeviceSettingsSnapshot), String> {
    let before = io.read()?;
    // A device switch, stale draft, another Type instance or another settings
    // writer must never turn the old form into a valid write to this target.
    if expected_revision == 0 || before.settings_revision != expected_revision {
        return Err("Device settings changed since they were read. Read this selected device again before saving.".into());
    }
    let mut final_status = before.clone();
    let baseline = device_settings_snapshot_from_status(before);
    let enabled = request.plugged_low_power_enabled && request.plugged_low_power_idle_minutes > 0;
    let rename = request.ble_name != baseline.ble_name;
    let commands = device_settings_update_commands(
        request,
        baseline.led_zone_brightness_supported,
        enabled,
        rename,
        Some(&baseline),
    )?;
    // Repack with a firmware revision condition on *every* packet. The
    // previous packer's 63-byte limit did not reserve space for this guard.
    let assignments: Vec<String> = commands
        .iter()
        .flat_map(|command| {
            command
                .trim_start_matches("DEVICE:SET ")
                .split_ascii_whitespace()
                .map(str::to_string)
        })
        .collect();
    let mut revision = expected_revision;
    let mut cursor = 0;
    while cursor < assignments.len() {
        let mut command = format!("DEVICE:SET rev={revision}");
        while cursor < assignments.len()
            && command.len() + assignments[cursor].len() + 2
                <= DEVICE_SETTINGS_BLE_CONTROL_MAX_BYTES
        {
            command.push(' ');
            command.push_str(&assignments[cursor]);
            cursor += 1;
        }
        if command == format!("DEVICE:SET rev={revision}") {
            return Err("Setting cannot fit in a guarded control packet.".into());
        }
        io.write(&command)?;
        let expected_next = if revision == u32::MAX {
            1
        } else {
            revision + 1
        };
        // BLE writes are acknowledged at ATT before the host task may have
        // committed NVS. Never send the next revision until it is observed.
        let mut observed = None;
        for _ in 0..8 {
            let status = io.read()?;
            if status.settings_revision == expected_next {
                observed = Some(status);
                break;
            }
            if status.settings_revision != revision {
                return Err(
                    "Another writer changed this device during save. Read settings again.".into(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(70));
        }
        final_status = observed.ok_or("Selected device did not confirm the settings revision.")?;
        revision = final_status.settings_revision;
    }
    let mut snapshot = device_settings_snapshot_from_status(final_status);
    let mut saved_snapshot = snapshot.clone();
    // Pending name application is expected here; all other fields must have
    // reached NVS before the advertising handoff is requested.
    saved_snapshot.ble_name_pending_restart = false;
    let mismatches = device_settings_readback_mismatches(
        &saved_snapshot,
        request,
        baseline.led_zone_brightness_supported,
        enabled,
    );
    if !mismatches.is_empty() {
        return Err(format!(
            "Selected device settings readback mismatch: {}",
            mismatches.join("; ")
        ));
    }
    if rename || snapshot.ble_name_pending_restart {
        // Apply only through the still-pinned handle. Do not run a global
        // pairing cleanup, change the voice target, or try another transport.
        io.write("DEVICE:APPLY_BLE_NAME")?;
        std::thread::sleep(std::time::Duration::from_millis(350));
        match io.read() {
            Ok(status) if status.ble_name == request.ble_name => { revision = status.settings_revision; snapshot = device_settings_snapshot_from_status(status); },
            Ok(_) => return Err("Name was saved but the selected device returned an unexpected name.".into()),
            Err(_) => snapshot.detail = Some("Name was saved and reconnect was requested. Read this device again after Bluetooth reconnects.".into()),
        }
    }
    Ok((revision, snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded_ble::DeviceSettingsStatus;
    #[test]
    fn read_lease_rejects_another_device_even_when_revisions_match() {
        let a = SettingsDevice {
            id: "usb:A".into(),
            transport: "usb".into(),
            label: "B".into(),
        };
        let b = SettingsDevice {
            id: "ble:B".into(),
            transport: "ble".into(),
            label: "B".into(),
        };
        let token = issue_read_token(&a, 7);
        assert!(claim_read_token(&token, &b, 7).is_err());
        let token = issue_read_token(&a, 7);
        assert!(claim_read_token(&token, &a, 7).is_ok());
        assert!(claim_read_token(&token, &a, 7).is_err());
    }
    struct Board {
        status: DeviceSettingsStatus,
        writes: Vec<String>,
        stale_reads: u8,
        race: bool,
    }
    fn board() -> Board {
        Board {
            writes: vec![],
            stale_reads: 0,
            race: false,
            status: DeviceSettingsStatus {
                brightness_percent: 100,
                plugged_brightness_percent: 100,
                battery_brightness_percent: 100,
                status_led_brightness_percent: 100,
                key_led_brightness_percent: 100,
                knob_led_brightness_percent: 100,
                edge_led_brightness_percent: 100,
                led_zone_brightness_supported: true,
                compact_set_supported: true,
                low_power_idle_minutes: 1,
                plugged_low_power_idle_minutes: 3,
                battery_low_power_idle_minutes: 1,
                plugged_low_power_enabled: true,
                voice_auto_start_enabled: true,
                voice_auto_stop_enabled: true,
                plugged_auto_shutdown_minutes: 0,
                battery_auto_shutdown_minutes: 10,
                settings_revision: 7,
                knob_rotation_action: "system_volume".into(),
                ble_name: "B".into(),
                ble_name_pending_restart: false,
                external_power_present: true,
                usb_power_present: true,
                charging: false,
                charge_full: false,
                raw_line: String::new(),
            },
        }
    }
    fn request(board: &Board) -> DeviceSettingsUpdateRequest {
        let s = &board.status;
        DeviceSettingsUpdateRequest {
            status_led_brightness_percent: s.status_led_brightness_percent,
            key_led_brightness_percent: s.key_led_brightness_percent,
            knob_led_brightness_percent: s.knob_led_brightness_percent,
            edge_led_brightness_percent: s.edge_led_brightness_percent,
            plugged_low_power_idle_minutes: s.plugged_low_power_idle_minutes,
            battery_low_power_idle_minutes: s.battery_low_power_idle_minutes,
            plugged_low_power_enabled: s.plugged_low_power_enabled,
            voice_auto_start_enabled: s.voice_auto_start_enabled,
            voice_auto_stop_enabled: s.voice_auto_stop_enabled,
            plugged_auto_shutdown_minutes: 0,
            battery_auto_shutdown_minutes: s.battery_auto_shutdown_minutes,
            ble_name: s.ble_name.clone(),
        }
    }
    impl SettingsIo for Board {
        fn read(&mut self) -> Result<DeviceSettingsStatus, String> {
            let mut result = self.status.clone();
            if self.stale_reads > 0 {
                self.stale_reads -= 1;
                result.settings_revision -= 1;
            }
            Ok(result)
        }
        fn write(&mut self, command: &str) -> Result<(), String> {
            self.writes.push(command.into());
            if command == "DEVICE:APPLY_BLE_NAME" {
                self.status.ble_name_pending_restart = false;
                return Ok(());
            }
            assert!(command.len() + 1 <= 63);
            for token in command
                .trim_start_matches("DEVICE:SET ")
                .split_ascii_whitespace()
            {
                let (key, value) = token.split_once('=').unwrap();
                match key {
                    "rev" => {
                        if self.race
                            || value.parse::<u32>().unwrap() != self.status.settings_revision
                        {
                            return Err("revision_mismatch".into());
                        }
                    }
                    "ls" => self.status.status_led_brightness_percent = value.parse().unwrap(),
                    "lk" => self.status.key_led_brightness_percent = value.parse().unwrap(),
                    "ble_name" => {
                        self.status.ble_name = value.into();
                        self.status.ble_name_pending_restart = true;
                    }
                    _ => panic!("unhandled fixture assignment {key}"),
                }
            }
            self.status.settings_revision += 1;
            self.stale_reads = 1; // ATT completes before NVS readback catches up.
            Ok(())
        }
    }
    #[test]
    fn stale_form_cannot_write_to_newly_selected_board() {
        let mut a = board();
        let mut b = board();
        b.status.settings_revision = 9;
        let mut changes = request(&a);
        changes.status_led_brightness_percent = 20;
        assert!(write_selected_settings(&mut b, 7, &changes).is_err());
        assert!(b.writes.is_empty());
        assert!(a.writes.is_empty());
        let result = write_selected_settings(&mut a, 7, &changes).unwrap();
        assert_eq!(result.1.status_led_brightness_percent, 20);
        assert_eq!(b.status.status_led_brightness_percent, 100);
    }
    #[test]
    fn firmware_race_rejection_is_not_retried_on_another_device() {
        let mut a = board();
        a.race = true;
        let mut changes = request(&a);
        changes.key_led_brightness_percent = 40;
        assert!(write_selected_settings(&mut a, 7, &changes)
            .unwrap_err()
            .contains("revision_mismatch"));
        assert_eq!(a.writes.len(), 1);
    }
    #[test]
    fn rename_confirms_saved_state_before_applying_on_selected_handle() {
        let mut a = board();
        let mut changes = request(&a);
        changes.ble_name = "TheSelectedListener".into();
        let (_, snapshot) = write_selected_settings(&mut a, 7, &changes).unwrap();
        assert_eq!(snapshot.ble_name, changes.ble_name);
        assert!(!snapshot.ble_name_pending_restart);
        assert_eq!(a.writes.last().unwrap(), "DEVICE:APPLY_BLE_NAME");
    }
    #[test]
    fn unchanged_form_does_not_apply_name_or_restart_bluetooth() {
        let mut a = board();
        let changes = request(&a);
        write_selected_settings(&mut a, 7, &changes).unwrap();
        assert!(a.writes.is_empty());
    }
}
