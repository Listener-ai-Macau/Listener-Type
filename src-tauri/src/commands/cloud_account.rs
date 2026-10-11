//! Email/subscription UI integration port. Login itself belongs to the account service.
use super::CoordinatorState;
use crate::asr::cloud_gateway::{self, CloudAccountStatus, CloudModelRoute};

#[tauri::command]
pub async fn set_cloud_account_session(access_token: String) -> Result<CloudAccountStatus, String> {
    cloud_gateway::sign_in(access_token).await
}

#[tauri::command]
pub async fn clear_cloud_account_session(coord: CoordinatorState<'_>) -> Result<(), String> {
    coord.cancel_dictation();
    cloud_gateway::sign_out().await
}

#[tauri::command]
pub async fn get_cloud_account_status() -> CloudAccountStatus {
    cloud_gateway::account_status().await
}

#[tauri::command]
pub async fn prepare_cloud_voiceprint_enrollment(
    coord: CoordinatorState<'_>,
    route: CloudModelRoute,
    consent_version: String,
) -> Result<(), String> {
    let phrase = coord.prefs().get().voice_wake_phrase;
    let proxy =
        crate::coordinator::read_asr_proxy_config("volcengine").map_err(|_| "模型网络设置无效")?;
    cloud_gateway::prepare_enrollment(
        route,
        phrase,
        &consent_version,
        crate::coordinator::read_volc_credentials(),
        proxy,
    )
    .await
}
