//! Local layout drafts. These functions never enumerate, connect to or write a board.
use crate::{device_key_profile::DeviceKeyProfile, types::DeviceKnobRotationAction};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, sync::{Mutex, OnceLock}};

const LIMIT: usize = 20;
const FILE_LIMIT: usize = 256 * 1024;
static ACCESS: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceLayout {
    pub id: String,
    pub name: String,
    pub revision: u32,
    pub profile: DeviceKeyProfile,
    pub knob_rotation_action: DeviceKnobRotationAction,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Library { schema: u8, layouts: Vec<DeviceLayout> }

fn read(path: &Path) -> Result<Library, String> {
    if !path.exists() { return Ok(Library { schema: 1, layouts: Vec::new() }); }
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > FILE_LIMIT as u64 {
        return Err("device_layout_file_too_large".into());
    }
    let data = fs::read(path).map_err(|e| e.to_string())?;
    if data.len() > FILE_LIMIT { return Err("device_layout_file_too_large".into()); }
    let library: Library = serde_json::from_slice(&data).map_err(|_| "device_layout_invalid_file")?;
    if library.schema != 1 || library.layouts.len() > LIMIT { return Err("device_layout_invalid_file".into()); }
    let mut ids = std::collections::HashSet::new();
    for layout in &library.layouts {
        if uuid::Uuid::parse_str(&layout.id).is_err() || !ids.insert(layout.id.clone()) || layout.revision == 0 {
            return Err("device_layout_invalid_file".into());
        }
        let mut profile = layout.profile.clone();
        profile.compile()?;
        if layout.name.trim().is_empty() || layout.name.chars().count() > 80
            || crate::device_key_profile::compact_profile_bytes(&profile)?.len() > crate::device_key_profile::MAX_BYTES {
            return Err("device_layout_invalid_file".into());
        }
    }
    Ok(library)
}
pub fn list(path: &Path) -> Result<Vec<DeviceLayout>, String> {
    let _lock = ACCESS.get_or_init(|| Mutex::new(())).lock().map_err(|_| "device_layout_busy")?;
    Ok(read(path)?.layouts)
}
pub fn save(path: &Path, id: Option<String>, revision: Option<u32>, name: String,
    mut profile: DeviceKeyProfile, knob_rotation_action: DeviceKnobRotationAction) -> Result<DeviceLayout, String> {
    let _lock = ACCESS.get_or_init(|| Mutex::new(())).lock().map_err(|_| "device_layout_busy")?;
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 { return Err("device_layout_invalid_name".into()); }
    profile.compile()?;
    if crate::device_key_profile::compact_profile_bytes(&profile)?.len() > crate::device_key_profile::MAX_BYTES {
        return Err("device_layout_profile_too_large".into());
    }
    let mut library = read(path)?;
    let index = match id.as_ref() {
        Some(id) => Some(library.layouts.iter().position(|l| &l.id == id).ok_or("device_layout_stale")?),
        None => None,
    };
    let next_revision = if let Some(index) = index {
        let old = &library.layouts[index];
        if revision != Some(old.revision) { return Err("device_layout_stale".into()); }
        old.revision.checked_add(1).ok_or("device_layout_stale")?
    } else {
        if revision.is_some() { return Err("device_layout_stale".into()); }
        if library.layouts.len() >= LIMIT { return Err("device_layout_limit".into()); }
        1
    };
    let value = DeviceLayout { id: id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        name: name.into(), revision: next_revision, profile, knob_rotation_action };
    if let Some(index) = index { library.layouts[index] = value.clone(); }
    else { library.layouts.push(value.clone()); }
    write(path, &library)?;
    Ok(value)
}
pub fn delete(path: &Path, id: &str, revision: u32) -> Result<(), String> {
    let _lock = ACCESS.get_or_init(|| Mutex::new(())).lock().map_err(|_| "device_layout_busy")?;
    let mut library = read(path)?;
    let index = library.layouts.iter().position(|l| l.id == id && l.revision == revision)
        .ok_or("device_layout_stale")?;
    library.layouts.remove(index);
    write(path, &library)
}
fn write(path: &Path, library: &Library) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(library).map_err(|e| e.to_string())?;
    if data.len() > FILE_LIMIT { return Err("device_layout_file_too_large".into()); }
    crate::persistence::write_local_device_layouts(path, &data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_layouts_preserve_named_profiles_and_reject_stale_updates() {
        let root = std::env::temp_dir().join(format!("listener-layouts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap(); let path = root.join("layouts.json");
        let a = save(&path, None, None, "Writing".into(), DeviceKeyProfile::default(), DeviceKnobRotationAction::SystemVolume).unwrap();
        let b = save(&path, None, None, "Reading".into(), DeviceKeyProfile::default(), DeviceKnobRotationAction::Disabled).unwrap();
        let changed = save(&path, Some(a.id.clone()), Some(a.revision), "Writing revised".into(), a.profile.clone(), a.knob_rotation_action).unwrap();
        assert!(save(&path, Some(a.id.clone()), Some(a.revision), "Stale".into(), a.profile.clone(), a.knob_rotation_action).is_err());
        assert!(delete(&path, &a.id, a.revision).is_err());
        assert_eq!(list(&path).unwrap()[1].id, b.id);
        delete(&path, &a.id, changed.revision).unwrap();
        assert_eq!(list(&path).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn local_layouts_do_not_overwrite_bad_files_or_accept_invalid_profiles() {
        let root = std::env::temp_dir().join(format!("listener-layouts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap(); let path = root.join("layouts.json");
        fs::write(&path, b"broken").unwrap();
        assert!(save(&path,None,None,"A".into(),DeviceKeyProfile::default(),DeviceKnobRotationAction::Disabled).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken");
        fs::remove_file(&path).unwrap();
        let mut profile = DeviceKeyProfile::default(); profile.schema = 99;
        assert!(save(&path,None,None,"A".into(),profile,DeviceKnobRotationAction::Disabled).is_err());
        let mut profile = DeviceKeyProfile::default(); profile.device_custom_keys.key1.paste_template = "x".repeat(5000);
        assert!(save(&path,None,None,"A".into(),profile,DeviceKnobRotationAction::Disabled).is_err());
        assert!(!path.exists()); fs::remove_dir_all(root).unwrap();
    }
}
