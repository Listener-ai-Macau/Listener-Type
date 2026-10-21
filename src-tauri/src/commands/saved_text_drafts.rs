#[tauri::command]
pub fn list_saved_text_drafts(window: Window) -> Result<Vec<crate::saved_text_drafts::SavedTextDraft>, String> {
    ensure_main_window(&window)?;
    crate::saved_text_drafts::list(&crate::persistence::saved_text_drafts_path().map_err(|_| "saved_draft_read")?)
}
#[tauri::command]
pub fn save_text_draft(window: Window, id: String, name: String, original: String, text: String) -> Result<crate::saved_text_drafts::SavedTextDraft, String> {
    ensure_main_window(&window)?;
    crate::saved_text_drafts::save(&crate::persistence::saved_text_drafts_path().map_err(|_| "saved_draft_write")?,id,name,original,text)
}
#[tauri::command]
pub fn delete_saved_text_draft(window: Window, id: String) -> Result<(), String> {
    ensure_main_window(&window)?;
    crate::saved_text_drafts::delete(&crate::persistence::saved_text_drafts_path().map_err(|_| "saved_draft_write")?, &id)
}
