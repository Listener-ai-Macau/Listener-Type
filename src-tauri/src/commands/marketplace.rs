// Marketplace + GitHub OAuth command surface.
// Included into `commands` via `include!`.

// ─────────────────────────── marketplace (Phase A) ───────────────────────────
//
// 客户端跟 marketplace backend 的 HTTP 客户端封装。Backend URL 走 prefs
// `marketplace_base_url`（默认 http://127.0.0.1:8090 开发；生产用户填 https://api.<domain>）。
// auth：GitHub OAuth device flow token 写入系统 credential vault；上传、点赞、
// 撤回和“我的发布”在调用 marketplace backend 前先用 token 调 GitHub /user
// 取得当前 login，再沿用 v1 backend 的 X-Dev-User 身份头。
//
// IPC -> REST contract v1:
// - marketplace_list      GET    /styles?q=&category=&sort=&limit=&offset=
// - marketplace_detail    GET    /styles/{id}
// - marketplace_install   GET    /styles/{id}, GET /styles/{id}/download
// - marketplace_upload    POST   /styles/upload (multipart zip)
// - marketplace_like      POST   /styles/{id}/like
// - marketplace_delete    DELETE /styles/{id}
// - marketplace_my_likes  GET    /me/likes
// - marketplace_my_packs  GET    /me/styles

/// Listener Type does not inherit any upstream-owned production marketplace.
///
/// Remote marketplace calls are disabled by default. A future Listener Type
/// backend can be enabled by setting `LISTENER_TYPE_MARKETPLACE_BASE_URL` or
/// by writing `prefs.marketplace_base_url` through a controlled config surface.
const MARKETPLACE_BACKEND_DISABLED: &str =
    "Listener Type marketplace backend is not configured; local style packs remain available.";

fn configured_marketplace_url(prefs: &UserPreferences) -> Result<Option<String>, String> {
    let env_url = std::env::var("LISTENER_TYPE_MARKETPLACE_BASE_URL").unwrap_or_default();
    let configured = if env_url.trim().is_empty() {
        prefs.marketplace_base_url.trim()
    } else {
        env_url.trim()
    };
    if configured.is_empty() {
        return Ok(None);
    }
    let parsed =
        reqwest::Url::parse(configured).map_err(|e| format!("invalid marketplace url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("marketplace url must use http or https".into());
    }
    Ok(Some(configured.trim_end_matches('/').to_string()))
}

fn marketplace_url_from_prefs(prefs: &UserPreferences) -> Result<String, String> {
    configured_marketplace_url(prefs)?.ok_or_else(|| MARKETPLACE_BACKEND_DISABLED.to_string())
}

fn marketplace_client_from_prefs(prefs: &UserPreferences) -> Result<MarketplaceClient, String> {
    let base = marketplace_url_from_prefs(prefs)?;
    MarketplaceClient::new(&base).map_err(|error| error.to_string())
}

fn optional_marketplace_client_from_prefs(
    prefs: &UserPreferences,
) -> Result<Option<MarketplaceClient>, String> {
    configured_marketplace_url(prefs)?
        .map(|base| MarketplaceClient::new(&base).map_err(|error| error.to_string()))
        .transpose()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MarketplaceTransferProgressPayload {
    operation: String,
    pack_id: String,
    phase: String,
    progress: u8,
    message: String,
}

fn emit_marketplace_transfer_progress(
    app: &AppHandle,
    operation: &str,
    pack_id: &str,
    phase: &str,
    progress: u8,
    message: &str,
) {
    let payload = MarketplaceTransferProgressPayload {
        operation: operation.to_string(),
        pack_id: pack_id.to_string(),
        phase: phase.to_string(),
        progress,
        message: message.to_string(),
    };
    let _ = app.emit("marketplace-transfer-progress", payload);
}

fn marketplace_api_error_message(action: &str, error: MarketplaceApiError) -> String {
    let guidance = match error.kind() {
        MarketplaceApiErrorKind::InvalidUrl => "backend URL is invalid",
        MarketplaceApiErrorKind::Network => "network interrupted; check the connection and retry",
        MarketplaceApiErrorKind::Unauthorized => "GitHub or marketplace authorization failed",
        MarketplaceApiErrorKind::NotFound => "style pack was not found on the marketplace backend",
        MarketplaceApiErrorKind::HttpStatus => "marketplace backend rejected the request",
        MarketplaceApiErrorKind::Decode => "marketplace backend returned an invalid response",
    };
    format!("{action} failed: {guidance}: {error}")
}

async fn marketplace_authenticated_github_login() -> Result<String, String> {
    let client = GithubOAuthClient::production().map_err(|error| error.to_string())?;
    marketplace_authenticated_github_login_with_client(&client).await
}

async fn marketplace_authenticated_github_login_with_client(
    client: &GithubOAuthClient,
) -> Result<String, String> {
    let mut credentials = CredentialsVault::marketplace_github_credentials()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "未登录：请先用 GitHub OAuth 登录风格市场".to_string())?;

    let now = current_epoch_secs();
    if token_needs_refresh(&credentials, now) {
        let client_id = get_github_oauth_client_id()?;
        credentials = refresh_marketplace_github_credentials(client, &client_id, credentials, now)
            .await
            .map_err(|error| error.to_string())?;
    }

    let user = client
        .authenticated_user(&credentials.access_token)
        .await
        .map_err(|error| format!("GitHub 用户信息验证失败：{error}"))?;

    if credentials.login != user.login {
        credentials.login = user.login.clone();
        CredentialsVault::set_marketplace_github_credentials(credentials)
            .map_err(|error| error.to_string())?;
    }

    Ok(user.login)
}

async fn refresh_marketplace_github_credentials(
    client: &GithubOAuthClient,
    client_id: &str,
    credentials: MarketplaceGithubCredentials,
    now_epoch_secs: i64,
) -> Result<MarketplaceGithubCredentials, GithubOAuthError> {
    let Some(refresh_token) = credentials
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    else {
        return Err(GithubOAuthError::RefreshUnavailable);
    };
    if refresh_token_is_expired(&credentials, now_epoch_secs) {
        return Err(GithubOAuthError::RefreshExpired);
    }

    let token = client
        .refresh_access_token(client_id, refresh_token)
        .await?;
    let login = credentials.login;
    let mut refreshed = token.into_credentials(login, current_epoch_secs());
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token = Some(refresh_token.to_string());
    }
    if refreshed.refresh_token_expires_at_epoch_secs.is_none() {
        refreshed.refresh_token_expires_at_epoch_secs =
            credentials.refresh_token_expires_at_epoch_secs;
    }
    CredentialsVault::set_marketplace_github_credentials(refreshed.clone())
        .map_err(|error| GithubOAuthError::OAuth(error.to_string()))?;
    Ok(refreshed)
}

#[tauri::command]
pub async fn marketplace_list(
    coord: CoordinatorState<'_>,
    query: Option<String>,
    category: Option<String>,
    sort: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<MarketplaceListPage, String> {
    let prefs = coord.prefs().get();
    let Some(client) = optional_marketplace_client_from_prefs(&prefs)? else {
        return Ok(MarketplaceListPage::empty());
    };
    client
        .list_styles(
            query.as_deref(),
            category.as_deref(),
            sort.as_deref(),
            limit,
            offset,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn marketplace_detail(
    coord: CoordinatorState<'_>,
    pack_id: String,
) -> Result<MarketplaceDetail, String> {
    if !is_valid_session_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    marketplace_client_from_prefs(&prefs)?
        .style_detail(&pack_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn marketplace_install(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    pack_id: String,
) -> Result<StylePack, String> {
    // 安全校验：pack_id 来自远端 backend，可能含路径遍历 segment。
    // 用跟 read_audio_recording 同样的 UUID-v4 白名单挡住 ../ / 绝对路径等。
    // backend 当前用 Uuid::new_v4 生成所有 id，合法 id 必然匹配。
    if !is_valid_session_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let client = marketplace_client_from_prefs(&prefs)?;

    emit_marketplace_transfer_progress(
        &app,
        "install",
        &pack_id,
        "metadata",
        15,
        "Reading marketplace style pack details",
    );
    // 先拉 detail 拿 authorLogin —— 装好后本地写 originAuthorLogin，
    // 后续编辑+发布时 backend 据此判 supersede（原作者）vs derivative（他人 fork）。
    let detail = client
        .style_detail(&pack_id)
        .await
        .map_err(|error| marketplace_api_error_message("marketplace detail", error))?;
    let origin_author_login = if detail.summary.author_login.trim().is_empty() {
        None
    } else {
        Some(detail.summary.author_login)
    };

    emit_marketplace_transfer_progress(
        &app,
        "install",
        &pack_id,
        "downloading",
        35,
        "Downloading style pack archive",
    );
    let bytes = client
        .download_style_archive(&pack_id)
        .await
        .map_err(|error| marketplace_api_error_message("marketplace download", error))?;

    emit_marketplace_transfer_progress(
        &app,
        "install",
        &pack_id,
        "installing",
        70,
        "Validating and installing style pack archive",
    );
    // pack_id 已经过 UUID 白名单，拼临时文件路径安全。
    let tmp = std::env::temp_dir().join(format!("listener-type-marketplace-{pack_id}.zip"));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("write temporary style pack zip: {e}"))?;
    let imported_result = coord
        .style_packs()
        .import_from_zip(&tmp)
        .map_err(|e| format!("style pack format validation failed: {e}"));
    let _ = std::fs::remove_file(&tmp);
    let imported = imported_result?;

    // 绑定 origin —— 后续编辑+发布走 derivative / supersede 分支。
    let imported = coord
        .style_packs()
        .set_origin(&imported.id, Some(pack_id.clone()), origin_author_login)
        .map_err(|e| format!("set origin failed: {e}"))?;
    emit_marketplace_transfer_progress(
        &app,
        "install",
        &pack_id,
        "finished",
        100,
        "Installed locally",
    );
    Ok(imported)
}

#[tauri::command]
pub async fn marketplace_upload(
    coord: CoordinatorState<'_>,
    app: AppHandle,
    pack_id: String,
    origin_pack_id: Option<String>,
) -> Result<serde_json::Value, String> {
    // 本地 pack id 形态：`builtin.light` / 用户 slug / Uuid。用 local 白名单挡 `..` / `/` / `\`。
    if !is_valid_local_pack_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let client = marketplace_client_from_prefs(&prefs)?;

    emit_marketplace_transfer_progress(
        &app,
        "upload",
        &pack_id,
        "auth",
        10,
        "Verifying GitHub marketplace login",
    );
    let github_login = marketplace_authenticated_github_login().await?;

    emit_marketplace_transfer_progress(
        &app,
        "upload",
        &pack_id,
        "validating",
        25,
        "Validating local style pack",
    );
    // 拉本地 pack 拿 origin_pack_id —— 装过的 pack 这里有值，
    // backend 据此判同作者就 supersede 原行（新版本），他人就 derivative（独立新 row）。
    let local_pack = coord
        .style_packs()
        .get(&pack_id)
        .map_err(|e| format!("local pack not found: {e}"))?;
    if local_pack.kind == StylePackKind::Builtin {
        return Err(
            "builtin style packs cannot be uploaded; duplicate it as an editable pack first".into(),
        );
    }
    let origin_pack_id = origin_pack_id
        .filter(|id| is_valid_session_id(id))
        .or_else(|| local_pack.origin_pack_id.clone());

    // 先 export 本地 pack → 临时 ZIP
    let tmp = std::env::temp_dir().join(format!("listener-type-marketplace-upload-{pack_id}.zip"));
    coord
        .style_packs()
        .export_to_zip(&pack_id, &tmp)
        .map_err(|e| format!("style pack format validation failed before upload: {e}"))?;
    let bytes = std::fs::read(&tmp).map_err(|e| format!("read validated style pack zip: {e}"))?;
    let _ = std::fs::remove_file(&tmp);

    emit_marketplace_transfer_progress(
        &app,
        "upload",
        &pack_id,
        "uploading",
        60,
        "Uploading style pack archive to marketplace backend",
    );
    let parsed = client
        .upload_style_archive(&pack_id, origin_pack_id.as_deref(), bytes, &github_login)
        .await
        .map_err(|error| marketplace_api_error_message("marketplace upload", error))?;

    // 本地从未绑定 origin（首次上传一个本地原创 pack）→ 把 backend 分配的 pack id 写回本地，
    // 让用户在同设备上后续编辑能继续走「同作者 supersede」分支，更新自己原创的包。
    if origin_pack_id.is_none() {
        if let Some(remote_id) = parsed.get("id").and_then(|v| v.as_str()) {
            let _ = coord.style_packs().set_origin(
                &pack_id,
                Some(remote_id.to_string()),
                Some(github_login.clone()),
            );
        }
    }

    emit_marketplace_transfer_progress(
        &app,
        "upload",
        &pack_id,
        "finished",
        100,
        "Uploaded to marketplace backend",
    );
    Ok(parsed)
}

#[tauri::command]
pub async fn marketplace_like(
    coord: CoordinatorState<'_>,
    pack_id: String,
) -> Result<serde_json::Value, String> {
    if !is_valid_session_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let client = marketplace_client_from_prefs(&prefs)?;
    let github_login = marketplace_authenticated_github_login().await?;
    client
        .like_style(&pack_id, &github_login)
        .await
        .map_err(|error| format!("like request failed: {error}"))
}

/// 撤回自己发布的 pack（后端软删 state='withdrawn'，前端列表不再可见）。
/// pack_id 来自远端，必须是 UUID-v4。
#[tauri::command]
pub async fn marketplace_delete(
    coord: CoordinatorState<'_>,
    pack_id: String,
) -> Result<(), String> {
    if !is_valid_session_id(&pack_id) {
        return Err("invalid pack id".into());
    }
    let prefs = coord.prefs().get();
    let client = marketplace_client_from_prefs(&prefs)?;
    let github_login = marketplace_authenticated_github_login().await?;
    client
        .delete_style(&pack_id, &github_login)
        .await
        .map_err(|error| format!("delete request failed: {error}"))
}

/// 拉当前用户赞过的所有 pack id，用于客户端市场页面渲染红心 + 「我赞过的」过滤。
#[tauri::command]
pub async fn marketplace_my_likes(coord: CoordinatorState<'_>) -> Result<Vec<String>, String> {
    let prefs = coord.prefs().get();
    let Some(client) = optional_marketplace_client_from_prefs(&prefs)? else {
        return Ok(Vec::new());
    };
    let github_login = match marketplace_authenticated_github_login().await {
        Ok(login) => login,
        Err(error) => {
            log::info!("[marketplace] my-likes skipped: {error}");
            return Ok(Vec::new());
        }
    };
    if github_login.is_empty() {
        return Ok(Vec::new()); // 未登录就空集合，UI 渲染无红心
    }
    client
        .my_likes(&github_login)
        .await
        .map_err(|error| format!("my-likes request failed: {error}"))
}

/// 拉当前用户发布过的 pack（含审核中/已通过/已拒绝/已撤回），用于「我的发布」页面。
#[tauri::command]
pub async fn marketplace_my_packs(
    coord: CoordinatorState<'_>,
) -> Result<Vec<MarketplaceMyPackItem>, String> {
    let prefs = coord.prefs().get();
    let Some(client) = optional_marketplace_client_from_prefs(&prefs)? else {
        return Ok(Vec::new());
    };
    let github_login = match marketplace_authenticated_github_login().await {
        Ok(login) => login,
        Err(error) => {
            log::info!("[marketplace] my-packs skipped: {error}");
            return Ok(Vec::new());
        }
    };
    if github_login.is_empty() {
        return Ok(Vec::new());
    }
    client
        .my_styles(&github_login)
        .await
        .map_err(|error| format!("my-packs request failed: {error}"))
}

// ─────────────────────── GitHub OAuth Device Flow (Phase 1) ───────────────────────
//
// Rust 后端直连 GitHub 拿 access_token + login。token 只写入系统
// credential vault；前端只拿 login 用于展示/兼容现有上传按钮状态。
// Listener Type 后端未上线前，默认不内置 OAuth App，因此该能力必须由环境变量
// 或未来配置显式开启。
//
// 配置 client_id 的两种方式（OAuth App client_id 非敏感，但必须使用 Listener Type 自有 App）：
//   1. 生产构建可在下方 GITHUB_OAUTH_CLIENT_ID 常量填 Listener Type 自有值
//   2. 启动前设置环境变量 GITHUB_OAUTH_CLIENT_ID=<your_client_id>
//
// 注册 OAuth App：
//   https://github.com/settings/applications/new
//   - Application name: Listener Type (or your fork)
//   - Homepage URL: https://github.com/Listener-ai-Macau/Listener-Type
//   - Authorization callback URL: http://localhost (Device Flow 不真用，但表单要求填)
//   - 创建后在 General 页面勾选 "Enable Device Flow"
//   - 抄 client_id 填到本常量

const GITHUB_OAUTH_CLIENT_ID: &str = "";

fn get_github_oauth_client_id() -> Result<String, String> {
    if let Ok(env_id) = std::env::var("GITHUB_OAUTH_CLIENT_ID") {
        let trimmed = env_id.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if !GITHUB_OAUTH_CLIENT_ID.is_empty() {
        return Ok(GITHUB_OAUTH_CLIENT_ID.to_string());
    }
    Err("GitHub OAuth 未配置。请为 Listener Type 注册自有 OAuth App\
        （必须勾 Enable Device Flow），把 client_id 填到 \
        src-tauri/src/commands.rs 的 GITHUB_OAUTH_CLIENT_ID 常量，\
        或在启动前设置环境变量 GITHUB_OAUTH_CLIENT_ID=<your_client_id>。"
        .to_string())
}

#[tauri::command]
pub async fn github_device_flow_start() -> Result<GithubDeviceStartResponse, String> {
    let client_id = get_github_oauth_client_id()?;
    let client = GithubOAuthClient::production().map_err(|error| error.to_string())?;
    client
        .start_device_flow(&client_id, "read:user")
        .await
        .map_err(|error| format!("调用 GitHub /login/device/code 失败：{error}"))
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GithubDevicePollResult {
    Authorized { login: String },
    Pending,
    SlowDown,
    Error { message: String },
}

#[tauri::command]
pub async fn github_device_flow_poll(
    device_code: String,
) -> Result<GithubDevicePollResult, String> {
    let client_id = get_github_oauth_client_id()?;
    let client = GithubOAuthClient::production().map_err(|error| error.to_string())?;
    let poll = client
        .poll_device_flow(&client_id, &device_code)
        .await
        .map_err(|error| format!("调用 GitHub /login/oauth/access_token 失败：{error}"))?;

    let token = match poll {
        GithubDevicePollStatus::Authorized(token) => token,
        GithubDevicePollStatus::Pending => return Ok(GithubDevicePollResult::Pending),
        GithubDevicePollStatus::SlowDown => return Ok(GithubDevicePollResult::SlowDown),
        GithubDevicePollStatus::Expired => {
            return Ok(GithubDevicePollResult::Error {
                message: "OAuth 设备码已过期，请重新发起登录".to_string(),
            })
        }
        GithubDevicePollStatus::AccessDenied => {
            return Ok(GithubDevicePollResult::Error {
                message: "你在 GitHub 上拒绝了授权".to_string(),
            })
        }
        GithubDevicePollStatus::Error(message) => {
            return Ok(GithubDevicePollResult::Error { message })
        }
    };

    let user = client
        .authenticated_user(&token.access_token)
        .await
        .map_err(|error| format!("调用 GitHub /user 失败：{error}"))?;
    let credentials = token.into_credentials(user.login.clone(), current_epoch_secs());
    CredentialsVault::set_marketplace_github_credentials(credentials)
        .map_err(|error| format!("保存 GitHub OAuth token 失败：{error}"))?;
    Ok(GithubDevicePollResult::Authorized { login: user.login })
}

// ── device domain thin Tauri wrappers ──

#[tauri::command]
pub async fn refresh_device_settings_status(
) -> Result<crate::embedded_ble::DeviceSettingsStatus, String> {
    device::refresh_device_settings_status().await
}

#[tauri::command]
pub async fn submit_embedded_audio_notifications(
    coord: CoordinatorState<'_>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    device::submit_embedded_audio_notifications(coord, notifications).await
}

#[tauri::command]
pub async fn submit_embedded_audio_streaming_notifications(
    coord: CoordinatorState<'_>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    device::submit_embedded_audio_streaming_notifications(coord, notifications).await
}

#[tauri::command]
pub async fn submit_embedded_audio_file(
    coord: CoordinatorState<'_>,
    path: String,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    device::submit_embedded_audio_file(coord, path, format).await
}

#[tauri::command]
pub async fn submit_embedded_audio_streaming_file(
    coord: CoordinatorState<'_>,
    path: String,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    device::submit_embedded_audio_streaming_file(coord, path, format).await
}

#[tauri::command]
pub async fn submit_embedded_audio_ble_once(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    device::submit_embedded_audio_ble_once(coord, timeout_ms).await
}

#[tauri::command]
pub async fn probe_embedded_audio_ble_subscription(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    device::probe_embedded_audio_ble_subscription(coord, timeout_ms).await
}

#[tauri::command]
pub async fn repair_embedded_ble_connection(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<EmbeddedBleRepairResult, String> {
    device::repair_embedded_ble_connection(coord, timeout_ms).await
}

#[tauri::command]
pub async fn recover_embedded_ble_device(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<EmbeddedBleRepairResult, String> {
    device::recover_embedded_ble_device(coord, timeout_ms).await
}
