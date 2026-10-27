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

/// The nonactivating question card may save only an answer the native QA
/// controller has completed in its current visible conversation. It gains no
/// general draft read, edit, delete or arbitrary-text write permission.
#[tauri::command]
pub fn save_question_answer_draft(
    coord: CoordinatorState<'_>, window: Window, id: String, name: String, text: String,
) -> Result<crate::saved_text_drafts::SavedTextDraft, String> {
    if window.label() != "qa" { return Err("saved_draft_window".into()); }
    if !coord.can_save_completed_question_answer(&text) {
        return Err("saved_answer_not_available".into());
    }
    crate::saved_text_drafts::save(
        &crate::persistence::saved_text_drafts_path().map_err(|_| "saved_draft_write")?,
        id, name, text.clone(), text,
    )
}
#[tauri::command]
pub fn delete_saved_text_draft(window: Window, id: String) -> Result<(), String> {
    ensure_main_window(&window)?;
    crate::saved_text_drafts::delete(&crate::persistence::saved_text_drafts_path().map_err(|_| "saved_draft_write")?, &id)
}

#[tauri::command]
pub fn rename_saved_text_draft(window: Window, id: String, expected_name: String, name: String) -> Result<crate::saved_text_drafts::SavedTextDraft, String> {
    ensure_main_window(&window)?;
    crate::saved_text_drafts::rename(&crate::persistence::saved_text_drafts_path().map_err(|_| "saved_draft_write")?, &id, &expected_name, &name)
}
