// Local reusable drafts have no SettingsDevice/Coordinator access and never write hardware.
#[tauri::command]
pub fn list_local_device_layouts(window: Window) -> Result<Vec<crate::device_layouts::DeviceLayout>, String> {
    ensure_main_window(&window)?;
    crate::device_layouts::list(&crate::persistence::local_device_layouts_path().map_err(|e| e.to_string())?)
}
#[tauri::command]
pub fn save_local_device_layout(window: Window, id: Option<String>, revision: Option<u32>, name: String,
    profile: crate::device_key_profile::DeviceKeyProfile, knob_rotation_action: crate::types::DeviceKnobRotationAction)
    -> Result<crate::device_layouts::DeviceLayout, String> {
    ensure_main_window(&window)?;
    crate::device_layouts::save(&crate::persistence::local_device_layouts_path().map_err(|e| e.to_string())?,
        id, revision, name, profile, knob_rotation_action)
}
#[tauri::command]
pub fn delete_local_device_layout(window: Window, id: String, revision: u32) -> Result<(), String> {
    ensure_main_window(&window)?;
    crate::device_layouts::delete(&crate::persistence::local_device_layouts_path().map_err(|e| e.to_string())?, &id, revision)
}
