//! Persistent configuration belongs to the selected board, never global preferences.
use crate::{
    embedded_ble::settings_target::{SettingsDevice, SettingsIo},
    types::*,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
pub(crate) const MAX_BYTES: usize = 4096;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceKeyProfile {
    pub schema: u8,
    pub device_custom_keys: DeviceCustomKeys,
    pub device_custom_key_double_clicks: DeviceCustomKeys,
    pub device_custom_key_long_presses: DeviceCustomKeys,
    #[serde(default)]
    pub hid: Vec<[u8; 3]>,
}
impl Default for DeviceKeyProfile {
    fn default() -> Self {
        Self {
            schema: 1,
            device_custom_keys: DeviceCustomKeys::default(),
            device_custom_key_double_clicks: DeviceCustomKeys::disabled(),
            device_custom_key_long_presses: DeviceCustomKeys::disabled(),
            hid: vec![[0, 0, 0]; 15],
        }
    }
}
impl DeviceKeyProfile {
    pub fn mapping(
        &self,
        key: DeviceCustomKeyId,
        gesture: DeviceCustomKeyGesture,
    ) -> DeviceCustomKeyMapping {
        match gesture {
            DeviceCustomKeyGesture::SingleClick => self.device_custom_keys.get(key),
            DeviceCustomKeyGesture::DoubleClick => self.device_custom_key_double_clicks.get(key),
            DeviceCustomKeyGesture::LongPress => self.device_custom_key_long_presses.get(key),
        }
        .clone()
    }
    pub fn compile(&mut self) -> Result<(), String> {
        if self.schema != 1 {
            return Err("Unsupported device key profile schema".into());
        }
        self.hid.clear();
        for keys in [
            &self.device_custom_keys,
            &self.device_custom_key_double_clicks,
            &self.device_custom_key_long_presses,
        ] {
            for key in [
                DeviceCustomKeyId::Key1,
                DeviceCustomKeyId::Key2,
                DeviceCustomKeyId::Key3,
                DeviceCustomKeyId::Key4,
                DeviceCustomKeyId::Knob,
            ] {
                let m = keys.get(key);
                crate::commands::validate_device_custom_key_mapping(m)?;
                self.hid.push(match m.action {
                    DeviceCustomKeyAction::Disabled => [1, 0, 0],
                    DeviceCustomKeyAction::SendShortcut => m
                        .shortcut
                        .as_ref()
                        .and_then(native_hid)
                        .unwrap_or([0, 0, 0]),
                    DeviceCustomKeyAction::CopyShortcut => [2, 1, 6],
                    DeviceCustomKeyAction::PasteShortcut => [2, 1, 25],
                    DeviceCustomKeyAction::UndoShortcut => [2, 1, 29],
                    _ => [0, 0, 0],
                });
            }
        }
        Ok(())
    }
}
fn native_hid(s: &ShortcutBinding) -> Option<[u8; 3]> {
    let key = s.primary.to_ascii_lowercase();
    let primary_modifier = match key.as_str() {
        "leftcontrol" | "control" | "ctrl" => 1,
        "leftshift" | "shift" => 2,
        "leftalt" | "alt" => 4,
        "leftmeta" | "leftsuper" | "leftwin" => 8,
        "rightcontrol" => 16,
        "rightshift" => 32,
        "rightalt" => 64,
        "rightmeta" | "rightsuper" | "rightwin" => 128,
        _ => 0,
    };
    let usage = if primary_modifier != 0 {
        0
    } else if key.len() == 1 {
        let c = key.as_bytes()[0];
        match c {
            b'a'..=b'z' => c - b'a' + 4,
            b'1'..=b'9' => c - b'1' + 30,
            b'0' => 39,
            _ => return None,
        }
    } else if let Some(f) = key.strip_prefix('f').and_then(|x| x.parse::<u8>().ok()) {
        if !(1..=12).contains(&f) {
            return None;
        }
        57 + f
    } else {
        match key.as_str() {
            "enter" => 40,
            "escape" | "esc" => 41,
            "backspace" => 42,
            "tab" => 43,
            "space" => 44,
            "insert" => 73,
            "home" => 74,
            "pageup" => 75,
            "delete" => 76,
            "end" => 77,
            "pagedown" => 78,
            "arrowright" | "right" => 79,
            "arrowleft" | "left" => 80,
            "arrowdown" | "down" => 81,
            "arrowup" | "up" => 82,
            _ => return None,
        }
    };
    let mut modifier = primary_modifier;
    for m in &s.modifiers {
        modifier |= match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => 1,
            "shift" => 2,
            "alt" => 4,
            "super" | "meta" | "win" | "cmd" => 8,
            _ => return None,
        };
    }
    Some([2, modifier, usage])
}
pub fn crc(bytes: &[u8]) -> u32 {
    let mut c = u32::MAX;
    for b in bytes {
        c ^= *b as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ 0xedb88320u32.wrapping_mul(c & 1);
        }
    }
    !c
}
fn fields(line: &str) -> HashMap<&str, &str> {
    line.split_ascii_whitespace()
        .filter_map(|v| v.split_once('='))
        .collect()
}
fn number(f: &HashMap<&str, &str>, name: &str) -> Result<u32, String> {
    f.get(name)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("Device key profile missing {name}"))
}
fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return Err("Invalid device key profile page".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "Invalid profile hex".into()))
        .collect()
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardKeys {
    pub device_uid: String,
    pub revision: u32,
    pub profile: DeviceKeyProfile,
}
pub fn read(io: &mut dyn SettingsIo) -> Result<Option<OnboardKeys>, String> {
    let first = io.read()?;
    let f = fields(&first.raw_line);
    if !f.contains_key("kr") {
        return Ok(None);
    }
    let device_uid = f
        .get("ki")
        .filter(|v| v.len() == 12 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("Missing stable onboard device identity")?
        .to_ascii_lowercase();
    let revision = number(&f, "kr")?;
    let len = number(&f, "ks")? as usize;
    if len > MAX_BYTES || revision == 0 {
        return Err("Invalid device key profile size/revision".into());
    }
    if len == 0 {
        return Ok(Some(OnboardKeys {
            device_uid,
            revision,
            profile: DeviceKeyProfile::default(),
        }));
    }
    let checksum = u32::from_str_radix(f.get("kc").ok_or("Missing profile CRC")?, 16)
        .map_err(|_| "Invalid profile CRC")?;
    // Reuse only a body already read from this exact hardware UID, after an
    // uncached metadata read proves revision, length and checksum unchanged.
    // This does not cache names/power/brightness or push anything to a board.
    {
        let cache = profiles().lock().unwrap_or_else(|p| p.into_inner());
        if cache.wire.get(&device_uid) == Some(&(revision, len, checksum)) {
            if let Some(keys) = cache
                .values
                .get(&device_uid)
                .filter(|keys| keys.revision == revision)
            {
                return Ok(Some(keys.clone()));
            }
        }
    }
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        io.write(&format!("DEVICE:KEY R {:04x}", bytes.len()))?;
        let mut page = None;
        for _ in 0..4 {
            let status = io.read()?;
            let f = fields(&status.raw_line);
            if number(&f, "kr")? != revision || number(&f, "ks")? as usize != len {
                return Err("Device keys changed during read. Read this device again.".into());
            }
            if number(&f, "ko")? as usize == bytes.len() {
                page = Some(unhex(f.get("kd").ok_or("Missing key page")?)?);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let page = page.ok_or("Selected device did not confirm key page offset")?;
        if page.is_empty() || page.len() > 64 || bytes.len() + page.len() > len {
            return Err("Invalid key page length".into());
        }
        bytes.extend(page);
    }
    if crc(&bytes) != checksum {
        return Err("Device key profile checksum mismatch; no cached profile substituted".into());
    }
    let mut profile: DeviceKeyProfile =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid stored device keys: {e}"))?;
    profile.compile()?;
    let keys = OnboardKeys {
        device_uid,
        revision,
        profile,
    };
    {
        let mut cache = profiles().lock().unwrap_or_else(|p| p.into_inner());
        cache
            .wire
            .insert(keys.device_uid.clone(), (revision, len, checksum));
        cache.values.insert(keys.device_uid.clone(), keys.clone());
    }
    Ok(Some(keys))
}
pub(crate) fn compact_profile_bytes(profile: &DeviceKeyProfile) -> Result<Vec<u8>, String> {
    let mut value = serde_json::to_value(profile).map_err(|e| e.to_string())?;
    let defaults =
        serde_json::to_value(DeviceCustomKeyMapping::default()).map_err(|e| e.to_string())?;
    // Only omit values that the existing serde defaults reconstruct exactly.
    // Keep each key object, including {} for a disabled key in double/long maps.
    for group in [
        "deviceCustomKeys",
        "deviceCustomKeyDoubleClicks",
        "deviceCustomKeyLongPresses",
    ] {
        if let Some(keys) = value.get_mut(group).and_then(|v| v.as_object_mut()) {
            for key in keys.values_mut() {
                if let Some(mapping) = key.as_object_mut() {
                    mapping.retain(|k, v| defaults.get(k) != Some(v));
                }
            }
        }
    }
    serde_json::to_vec(&value).map_err(|e| e.to_string())
}

pub fn write(
    io: &mut dyn SettingsIo,
    expected: u32,
    mut profile: DeviceKeyProfile,
) -> Result<OnboardKeys, String> {
    profile.compile()?;
    let bytes = compact_profile_bytes(&profile)?;
    if bytes.len() > MAX_BYTES {
        return Err(format!("Device key configuration is {} bytes; device capacity is {MAX_BYTES}. Shorten templates or application paths before saving.",bytes.len()));
    }
    let status = io.read()?;
    let header = fields(&status.raw_line);
    let revision = number(&header, "kr")
        .map_err(|_| "This firmware does not support onboard keys. Update its firmware first.")?;
    let device_uid = header
        .get("ki")
        .filter(|s| s.len() == 12 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("Missing stable onboard identity")?
        .to_ascii_lowercase();
    if revision != expected {
        return Err("Device keys changed since read. Read the selected device again.".into());
    }
    let old_crc = u32::from_str_radix(header.get("kc").ok_or("Missing key checksum")?, 16)
        .map_err(|_| "Invalid key checksum")?;
    if number(&header, "ks")? as usize == bytes.len() && old_crc == crc(&bytes) {
        return Ok(OnboardKeys {
            device_uid,
            revision,
            profile,
        });
    }
    log::info!(
        "[device-keys] selected profile write revision={expected} encoded_bytes={} chunks={}",
        bytes.len(),
        bytes.len().div_ceil(16)
    );
    let tx = (uuid::Uuid::new_v4().as_u128() as u32).max(1);
    io.write(&format!(
        "DEVICE:KEY B {expected:08x} {:04x} {:08x} {tx:08x}",
        bytes.len(),
        crc(&bytes)
    ))?;
    let result = (|| {
        for (index, chunk) in bytes.chunks(16).enumerate() {
            let hex: String = chunk.iter().map(|b| format!("{b:02x}")).collect();
            io.write(&format!("DEVICE:KEY D {tx:08x} {:04x} {hex}", index * 16))?;
        }
        io.write(&format!("DEVICE:KEY C {tx:08x}"))?;
        let expected = if expected == u32::MAX {
            1
        } else {
            expected + 1
        };
        for _ in 0..10 {
            if let Some(saved) = read(io)? {
                if saved.revision == expected {
                    if saved.profile != profile {
                        return Err(
                            "Onboard key readback differs from submitted configuration".into()
                        );
                    }
                    return Ok(saved);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
        Err("Device did not confirm persistent key profile commit".into())
    })();
    if result.is_err() {
        let _ = io.write(&format!("DEVICE:KEY X {tx:08x}"));
    }
    result
}
#[derive(Default)]
struct Profiles {
    values: HashMap<String, OnboardKeys>,
    aliases: HashMap<String, String>,
    wire: HashMap<String, (u32, usize, u32)>,
}
fn profiles() -> &'static Mutex<Profiles> {
    static V: OnceLock<Mutex<Profiles>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(Profiles::default()))
}
pub fn remember(device: &SettingsDevice, keys: &OnboardKeys) {
    let mut cache = profiles().lock().unwrap_or_else(|p| p.into_inner());
    cache.values.insert(keys.device_uid.clone(), keys.clone());
    cache.aliases.insert(
        crate::embedded_ble::settings_target::peer_key(device),
        keys.device_uid.clone(),
    );
}
pub fn active_mapping(
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
) -> DeviceCustomKeyMapping {
    #[cfg(target_os = "windows")]
    if let Some(address) = crate::embedded_ble::current_notify_keep_address() {
        let id = format!("ble:{address:012X}");
        let cache = profiles().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(p) = cache.aliases.get(&id).and_then(|uid| cache.values.get(uid)) {
            return p.profile.mapping(key, gesture);
        }
    }
    DeviceKeyProfile::default().mapping(key, gesture)
}

pub fn notification_mapping(
    value: &[u8],
) -> Option<(
    DeviceCustomKeyId,
    DeviceCustomKeyGesture,
    DeviceCustomKeyMapping,
)> {
    let text = std::str::from_utf8(value).ok()?;
    let mut fields = text.strip_prefix("DKEY:")?.split(':');
    let uid = fields.next()?;
    let key = match fields.next()? {
        "1" => DeviceCustomKeyId::Key1,
        "2" => DeviceCustomKeyId::Key2,
        "3" => DeviceCustomKeyId::Key3,
        "4" => DeviceCustomKeyId::Key4,
        "5" => DeviceCustomKeyId::Knob,
        _ => return None,
    };
    let gesture = match fields.next()? {
        "0" => DeviceCustomKeyGesture::SingleClick,
        "1" => DeviceCustomKeyGesture::DoubleClick,
        "2" => DeviceCustomKeyGesture::LongPress,
        _ => return None,
    };
    let revision: u32 = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    #[cfg(target_os = "windows")]
    {
        let address = crate::embedded_ble::current_notify_keep_address()?;
        let cache = profiles().lock().unwrap_or_else(|p| p.into_inner());
        let id = format!("ble:{address:012X}");
        if cache.aliases.get(&id)?.as_str() != uid {
            return None;
        }
        let board = cache.values.get(uid)?;
        if board.revision != revision {
            return None;
        }
        return Some((key, gesture, board.profile.mapping(key, gesture)));
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (uid, key, gesture, revision);
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_keys_do_not_inherit_another_boards_map() {
        let a = SettingsDevice {
            id: "ble:A".into(),
            transport: "ble".into(),
            label: "same".into(),
        };
        let b = SettingsDevice {
            id: "ble:B".into(),
            ..a.clone()
        };
        let mut p = DeviceKeyProfile::default();
        p.device_custom_keys.key2.action = DeviceCustomKeyAction::Disabled;
        remember(
            &a,
            &OnboardKeys {
                device_uid: "000000000001".into(),
                revision: 1,
                profile: p,
            },
        );
        let cache = profiles().lock().unwrap();
        assert_eq!(
            cache
                .values
                .get("000000000001")
                .unwrap()
                .profile
                .device_custom_keys
                .key2
                .action,
            DeviceCustomKeyAction::Disabled
        );
        assert!(!cache.aliases.contains_key(&b.id));
    }
    #[test]
    fn compact_profile_roundtrip_keeps_every_key_and_gesture() {
        let mut profile = DeviceKeyProfile::default();
        profile.compile().unwrap();
        let full = serde_json::to_vec(&profile).unwrap();
        let compact = compact_profile_bytes(&profile).unwrap();
        assert!(compact.len() < full.len() / 2);
        let decoded: DeviceKeyProfile = serde_json::from_slice(&compact).unwrap();
        assert_eq!(decoded, profile);
        let mut configured = profile.clone();
        configured.device_custom_key_double_clicks.key4 = DeviceCustomKeyMapping {
            action: DeviceCustomKeyAction::SendShortcut,
            shortcut: Some(ShortcutBinding {
                primary: "F12".into(),
                modifiers: vec![],
            }),
            ..DeviceCustomKeyMapping::default()
        };
        configured.compile().unwrap();
        let decoded: DeviceKeyProfile =
            serde_json::from_slice(&compact_profile_bytes(&configured).unwrap()).unwrap();
        assert_eq!(decoded, configured);
    }
    #[test]
    fn compile_default_native_shortcuts_and_no_reserved_loop() {
        let mut p = DeviceKeyProfile::default();
        p.compile().unwrap();
        assert_eq!(p.hid[0], [2, 16, 0]);
        assert_eq!(p.hid[1], [2, 1, 6]);
        assert_eq!(p.hid[2], [2, 1, 25]);
        assert_eq!(p.hid[3], [2, 0, 40]);
        assert!(serde_json::to_vec(&p).unwrap().len() < MAX_BYTES);
        assert_eq!(crc(b"123456789"), 0xcbf43926);
    }
}
