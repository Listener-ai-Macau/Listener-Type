// Style pack list/create/import and polish-mode toggles.
// Included into `commands` via `include!`.

// ─────────────────────────── style packs ───────────────────────────

#[tauri::command]
pub fn list_style_packs(coord: CoordinatorState<'_>) -> Result<Vec<StylePack>, String> {
    let prefs = coord.prefs().get();
    coord
        .style_packs()
        .list_with_active(&prefs.active_style_pack_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_style_pack_from_template(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    template: StylePack,
    trial_only: Option<bool>,
) -> Result<StylePack, String> {
    log::info!(
        "[style-pack] command create_from_template name={} base_mode={:?}",
        template.name,
        template.base_mode
    );
    let trial_only = trial_only.unwrap_or(false);
    let created = if trial_only {
        coord.style_packs().create_for_text_trial(template)
    } else {
        coord.style_packs().create_from_template(template)
    }.map_err(|e| e.to_string())?;
    // Text workspaces save an inactive preset only. Do not rewrite preferences
    // or refresh the daily style cycle just because someone tried a template.
    if !trial_only {
        let prefs = coord.prefs().get();
        let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    }
    Ok(created)
}

#[tauri::command]
pub fn save_style_pack(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    style_pack: StylePack,
) -> Result<StylePack, String> {
    log::info!(
        "[style-pack] command save id={} kind={:?} base_mode={:?}",
        style_pack.id,
        style_pack.kind,
        style_pack.base_mode
    );
    let saved = coord
        .style_packs()
        .upsert(style_pack)
        .map_err(|e| e.to_string())?;
    if saved.kind == StylePackKind::Builtin {
        let prefs = coord.prefs().get();
        let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    }
    Ok(saved)
}

#[tauri::command]
pub fn get_style_text_trial_readiness(
    coord: CoordinatorState<'_>,
    style_pack_id: String,
) -> Result<crate::types::StyleTextTrialReadiness, String> {
    let requires_text_service = coord.style_text_trial_requires_service(&style_pack_id)?;
    let status = get_credentials(coord);
    Ok(crate::types::StyleTextTrialReadiness {
        available: true,
        requires_text_service,
        text_service_configured: status.llm_configured,
    })
}

#[tauri::command]
pub async fn try_style_pack_text(
    coord: CoordinatorState<'_>,
    style_pack_id: String,
    raw_text: String,
) -> Result<crate::types::StyleTextTrialResult, String> {
    coord.try_style_pack_text(style_pack_id, raw_text).await
}

#[tauri::command]
pub async fn try_translation_text(
    coord: CoordinatorState<'_>,
    raw_text: String,
    target_language: String,
) -> Result<crate::types::StyleTextTrialResult, String> {
    coord.try_translation_text(raw_text, target_language).await
}

#[tauri::command]
pub fn get_local_translation_status() -> Result<crate::local_translation::Status, String> {
    crate::local_translation::status()
}

/// Preparation is a separate explicit action, never a side effect of selecting
/// a language, opening a dialog, recording, or requesting a translation.
#[tauri::command]
pub async fn prepare_local_translation(source_language: String, target_language: String) -> Result<crate::local_translation::Status, String> {
    let preparation = crate::local_translation::reserve_preparation(&source_language, &target_language)?;
    tauri::async_runtime::spawn_blocking(move || preparation.run())
        .await.map_err(|_| "local_translation_runtime_failed".to_owned())?
}

#[tauri::command]
pub fn cancel_local_translation_preparation() {
    crate::local_translation::cancel_preparation();
}

#[tauri::command]
pub async fn try_local_translation_text(source_language: String, target_language: String, raw_text: String) -> Result<crate::types::StyleTextTrialResult, String> {
    let started = std::time::Instant::now();
    let pack_id = format!("local_translation:{source_language}:{target_language}");
    let pack_name = format!("{source_language} → {target_language}");
    let output_text = crate::local_translation::translate_to_language(raw_text, source_language, target_language).await?;
    Ok(crate::types::StyleTextTrialResult {
        pack_id, pack_name, output_text, used_text_service: false,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

#[tauri::command]
pub async fn copy_style_trial_result(text: String) -> Result<(), String> {
    if text.is_empty() || text.chars().count() > 100_000 {
        return Err("style_trial_clipboard_unavailable".into());
    }
    // Explicit copy only; no insertion, history entry or automatic processing.
    // Reuse the expiry/revision-aware clipboard worker already used by History.
    let copied = tauri::async_runtime::spawn_blocking(move || {
        crate::insertion::copy_explicit_text(&text)
    }).await.map_err(|_| "style_trial_clipboard_unavailable".to_owned())?;
    if copied { Ok(()) } else { Err("style_trial_clipboard_unavailable".into()) }
}

#[tauri::command]
pub async fn suggest_polite_text_edits(
    coord: CoordinatorState<'_>,
    text: String,
) -> Result<crate::types::PoliteTextEditsResult, String> {
    coord.suggest_polite_text_edits(text).await
}

#[tauri::command]
pub fn preview_style_pack_runtime(
    coord: CoordinatorState<'_>,
    style_pack: StylePack,
) -> Result<StylePackRuntimeDiagnostics, String> {
    log::info!(
        "[style-pack] command preview_runtime id={} base_mode={:?} prompt_chars={}",
        style_pack.id,
        style_pack.base_mode,
        style_pack.prompt.chars().count()
    );
    Ok(coord.preview_style_pack_runtime(&style_pack))
}

#[tauri::command]
pub fn set_active_style_pack(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    id: String,
) -> Result<StylePack, String> {
    activate_style_pack_by_id(&coord, &app, &id)
}

#[tauri::command]
pub fn set_style_pack_enabled(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<Vec<StylePack>, String> {
    log::info!(
        "[style-pack] command set_enabled requested id={} enabled={}",
        id,
        enabled
    );
    coord
        .style_packs()
        .set_enabled(&id, enabled)
        .map_err(|e| e.to_string())?;
    let mut prefs = coord.prefs().get();
    if !enabled && prefs.active_style_pack_id == id {
        prefs.active_style_pack_id = default_active_style_pack_id();
    }
    let prefs = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    coord
        .style_packs()
        .list_with_active(&prefs.active_style_pack_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reset_builtin_style_pack(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    id: String,
) -> Result<StylePack, String> {
    log::info!("[style-pack] command reset_builtin requested id={id}");
    let saved = coord
        .style_packs()
        .reset_builtin(&id)
        .map_err(|e| e.to_string())?;
    let prefs = coord.prefs().get();
    let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    Ok(saved)
}

#[tauri::command]
pub fn delete_style_pack(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    let mut prefs = coord.prefs().get();
    log::info!("[style-pack] command delete requested id={id}");
    coord
        .style_packs()
        .remove_imported(&id)
        .map_err(|e| e.to_string())?;
    if prefs.active_style_pack_id == id {
        prefs.active_style_pack_id = default_active_style_pack_id();
        let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    } else {
        refresh_tray_menu_async(&app);
    }
    Ok(())
}

#[tauri::command]
pub fn import_style_pack_from_zip(
    coord: CoordinatorState<'_>,
    zip_path: String,
) -> Result<StylePack, String> {
    log::info!("[style-pack] command import requested zip_path={zip_path}");
    coord
        .style_packs()
        .import_from_zip(std::path::Path::new(&zip_path))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_style_pack_to_zip(
    coord: CoordinatorState<'_>,
    id: String,
    target_path: String,
) -> Result<String, String> {
    log::info!(
        "[style-pack] command export requested id={} target_path={}",
        id,
        target_path
    );
    coord
        .style_packs()
        .export_to_zip(&id, std::path::Path::new(&target_path))
        .map_err(|e| e.to_string())?;
    Ok(target_path)
}

// ─────────────────────────── style toggles (compat) ───────────────────────────

#[tauri::command]
pub fn set_default_polish_mode(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    mode: PolishMode,
) -> Result<(), String> {
    activate_builtin_style_mode(&coord, &app, mode)
}

#[tauri::command]
pub fn set_style_enabled(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    mode: PolishMode,
    enabled: bool,
) -> Result<(), String> {
    let pack_id = builtin_style_pack_id(mode).to_string();
    log::info!(
        "[style-pack] compat set_style_enabled mode={:?} pack_id={} enabled={}",
        mode,
        pack_id,
        enabled
    );
    coord
        .style_packs()
        .set_enabled(&pack_id, enabled)
        .map_err(|e| e.to_string())?;
    let mut prefs = coord.prefs().get();
    if !enabled && prefs.active_style_pack_id == pack_id {
        prefs.active_style_pack_id = default_active_style_pack_id();
    }
    let _ = sync_style_pack_prefs_and_persist(&*coord, &app, prefs)?;
    Ok(())
}
