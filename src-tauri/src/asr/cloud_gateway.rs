//! Optional account service. Operator model secrets never enter this client.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use super::cloud_voiceprint::{CloudOwnerVoiceprint, CloudVoiceprintClient};
use super::volcengine::VolcengineCredentials;
use crate::persistence::CredentialsVault;
use crate::polish::ProviderProxyConfig;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudModelRoute {
    #[default]
    Byok,
    Managed,
}

#[derive(Clone, Serialize, Deserialize)]
struct AccountCredentials {
    base_url: String,
    access_token: String,
    customer: String,
}

#[derive(Clone)]
pub struct CloudGatewayClient {
    account: AccountCredentials,
    client: reqwest::Client,
    account_epoch: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccountStatus {
    pub configured: bool,
    pub signed_in: bool,
    pub customer: Option<String>,
    pub managed_enabled: bool,
    pub enrollment_state: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSessionTicket {
    pub id: String,
    pub websocket_url: String,
    pub session_token: String,
    pub profile: CloudOwnerVoiceprint,
}

struct EnrollmentPlan {
    client: CloudGatewayClient,
    id: String,
    route: CloudModelRoute,
    phrase: String,
    byok: Option<VolcengineCredentials>,
    proxy: ProviderProxyConfig,
}

pub struct EnrollmentCapture(EnrollmentPlan);

pub fn take_enrollment_capture() -> Option<EnrollmentCapture> {
    let pending = plan().lock().take()?;
    *status().lock() = (Some("uploading".into()), None);
    Some(EnrollmentCapture(pending))
}

pub async fn discard_enrollment_capture(capture: Option<EnrollmentCapture>) {
    if let Some(EnrollmentCapture(pending)) = capture {
        let _ = pending
            .client
            .request(
                reqwest::Method::DELETE,
                &format!("/v1/voiceprints/enrollments/{}", pending.id),
                None,
                None,
            )
            .await;
        *status().lock() = (None, None);
    }
}

static PLAN: OnceLock<Mutex<Option<EnrollmentPlan>>> = OnceLock::new();
static STATUS: OnceLock<Mutex<(Option<String>, Option<String>)>> = OnceLock::new();
static ACCOUNT_EPOCH: AtomicU64 = AtomicU64::new(0);
pub(crate) fn account_epoch() -> u64 {
    ACCOUNT_EPOCH.load(Ordering::Acquire)
}
fn plan() -> &'static Mutex<Option<EnrollmentPlan>> {
    PLAN.get_or_init(|| Mutex::new(None))
}
fn status() -> &'static Mutex<(Option<String>, Option<String>)> {
    STATUS.get_or_init(|| Mutex::new((None, None)))
}

fn configured_url() -> Result<String, String> {
    let value = option_env!("LISTENER_TYPE_CLOUD_BASE_URL")
        .map(str::to_string)
        .or_else(|| std::env::var("LISTENER_TYPE_CLOUD_BASE_URL").ok())
        .unwrap_or_default();
    validate_base_url(&value)?;
    Ok(value.trim_end_matches('/').to_string())
}

fn validate_base_url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| "产品云服务尚未配置，请使用自己的模型凭据".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
    {
        return Err("产品云服务地址无效".into());
    }
    Ok(url)
}

impl CloudGatewayClient {
    fn new(account: AccountCredentials) -> Result<Self, String> {
        validate_base_url(&account.base_url)?;
        if account.access_token.is_empty() || account.access_token.len() > 16384 {
            return Err("请先登录邮箱账号".into());
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(35))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "无法建立产品云服务连接")?;
        Ok(Self {
            account,
            client,
            account_epoch: account_epoch(),
        })
    }

    pub fn saved() -> Result<Self, String> {
        let value = CredentialsVault::get_cloud_account()
            .map_err(|_| "无法读取邮箱登录凭据")?
            .ok_or("请先登录邮箱账号")?;
        let account: AccountCredentials =
            serde_json::from_str(&value).map_err(|_| "邮箱登录已失效，请重新登录")?;
        if configured_url()? != account.base_url {
            return Err("云服务地址已改变，请重新登录".into());
        }
        Self::new(account)
    }

    fn still_current(&self) -> bool {
        if self.account_epoch != account_epoch() {
            return false;
        }
        Self::saved().is_ok_and(|current| {
            current.account.customer == self.account.customer
                && current.account.base_url == self.account.base_url
        })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        pcm: Option<Vec<u8>>,
    ) -> Result<Value, String> {
        if method != reqwest::Method::DELETE
            && !self.account.customer.is_empty()
            && !self.still_current()
        {
            return Err("邮箱账号已改变，已停止旧账号请求".into());
        }
        let mut request = self
            .client
            .request(method, format!("{}{}", self.account.base_url, path))
            .bearer_auth(&self.account.access_token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(pcm) = pcm {
            request = request.header("Content-Type", "audio/pcm").body(pcm);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "产品云服务请求未完成，请检查网络；声纹登记不会自动重复")?;
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                401 => "邮箱登录已过期，请重新登录",
                402 => "订阅的语音额度已用完",
                403 => "当前账号没有托管语音服务权益",
                409 => "声纹或会话状态已改变，请先刷新登记状态",
                _ => "产品云服务暂时不可用",
            }
            .into());
        }
        response
            .json()
            .await
            .map_err(|_| "产品云服务响应无效".into())
    }

    pub async fn create_session(&self) -> Result<CloudSessionTicket, String> {
        let result = self
            .request(
                reqwest::Method::POST,
                "/v1/asr/sessions",
                Some(json!({"requestId":uuid::Uuid::new_v4()})),
                None,
            )
            .await?;
        let ticket: CloudSessionTicket =
            serde_json::from_value(result).map_err(|_| "托管识别会话响应无效")?;
        let base = validate_base_url(&self.account.base_url)?;
        let websocket =
            reqwest::Url::parse(&ticket.websocket_url).map_err(|_| "托管识别地址无效")?;
        if websocket.scheme() != "wss"
            || websocket.host_str() != base.host_str()
            || websocket.port_or_known_default() != base.port_or_known_default()
            || !websocket.username().is_empty()
            || websocket.password().is_some()
            || websocket.query().is_some()
            || websocket.fragment().is_some()
            || websocket.path()
                != format!(
                    "{}/v1/asr/sessions/{}/stream",
                    base.path().trim_end_matches('/'),
                    ticket.id
                )
            || uuid::Uuid::parse_str(&ticket.id).is_err()
            || ticket.session_token.len() != 64
        {
            return Err("托管识别地址或会话凭据无效".into());
        }
        ticket.profile.validate_identity()?;
        if !self.still_current() {
            return Err("邮箱账号已改变，请重新开始识别".into());
        }
        Ok(ticket)
    }

    pub fn authorization(&self) -> String {
        format!("Bearer {}", self.account.access_token)
    }
}

pub async fn sign_in(access_token: String) -> Result<CloudAccountStatus, String> {
    let base_url = configured_url()?;
    let mut client = CloudGatewayClient::new(AccountCredentials {
        base_url,
        access_token,
        customer: String::new(),
    })?;
    let result = client
        .request(reqwest::Method::GET, "/v1/account", None, None)
        .await?;
    client.account.customer = result["customer"]
        .as_str()
        .filter(|id| id.len() == 64)
        .ok_or("邮箱账号响应无效")?
        .to_string();
    let changed = match CloudGatewayClient::saved() {
        Ok(previous) => previous.account.customer != client.account.customer,
        Err(_) => true,
    };
    if changed {
        ACCOUNT_EPOCH.fetch_add(1, Ordering::AcqRel);
        cancel_enrollment().await;
    }
    CredentialsVault::set_cloud_account(Some(
        serde_json::to_string(&client.account).map_err(|_| "无法保存登录")?,
    ))
    .map_err(|_| "无法保存邮箱登录凭据")?;
    Ok(CloudAccountStatus {
        configured: true,
        signed_in: true,
        customer: Some(client.account.customer),
        managed_enabled: result["managedEnabled"].as_bool().unwrap_or(false),
        ..Default::default()
    })
}

pub async fn sign_out() -> Result<(), String> {
    ACCOUNT_EPOCH.fetch_add(1, Ordering::AcqRel);
    cancel_enrollment().await;
    *status().lock() = (None, None);
    CredentialsVault::set_cloud_account(None).map_err(|_| "无法清除邮箱登录凭据".into())
}

pub async fn account_status() -> CloudAccountStatus {
    let (enrollment_state, error) = status().lock().clone();
    let mut result = CloudAccountStatus {
        configured: configured_url().is_ok(),
        enrollment_state,
        error,
        ..Default::default()
    };
    if let Ok(client) = CloudGatewayClient::saved() {
        match client
            .request(reqwest::Method::GET, "/v1/account", None, None)
            .await
        {
            Ok(value) => {
                result.signed_in = true;
                result.customer = Some(client.account.customer);
                result.managed_enabled = value["managedEnabled"].as_bool().unwrap_or(false);
            }
            Err(error) => result.error = Some(error),
        }
    }
    result
}

pub async fn prepare_enrollment(
    route: CloudModelRoute,
    phrase: String,
    consent: &str,
    byok: VolcengineCredentials,
    proxy: ProviderProxyConfig,
) -> Result<(), String> {
    if consent != "voiceprint-cloud-v1" {
        return Err("请先同意将三次登记录音用于云端本人识别".into());
    }
    if plan().lock().is_some() {
        return Err("声纹登记已经准备，请完成或取消当前登记".into());
    }
    let client = CloudGatewayClient::saved()?;
    let mut body = json!({"requestId":uuid::Uuid::new_v4(),"mode":route,"consentVersion":consent});
    if route == CloudModelRoute::Byok {
        CloudVoiceprintClient::new(byok.clone(), &proxy)?;
        body["accountFingerprint"] =
            json!(super::cloud_voiceprint::account_fingerprint(&byok.app_id));
    }
    let result = client
        .request(
            reqwest::Method::POST,
            "/v1/voiceprints/enrollments",
            Some(body),
            None,
        )
        .await?;
    let id = result["id"]
        .as_str()
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .ok_or("云端登记响应无效")?
        .to_string();
    if !client.still_current() {
        return Err("邮箱账号已改变，请重新登记".into());
    }
    *plan().lock() = Some(EnrollmentPlan {
        client,
        id,
        route,
        phrase,
        proxy,
        byok: if route == CloudModelRoute::Byok {
            Some(byok)
        } else {
            None
        },
    });
    *status().lock() = (Some("armed".into()), None);
    Ok(())
}

pub async fn cancel_enrollment() {
    let pending = plan().lock().take();
    *status().lock() = (None, None);
    if let Some(pending) = pending {
        let _ = pending
            .client
            .request(
                reqwest::Method::DELETE,
                &format!("/v1/voiceprints/enrollments/{}", pending.id),
                None,
                None,
            )
            .await;
    }
}

pub async fn delete_binding(
    route: CloudModelRoute,
    profile: Option<CloudOwnerVoiceprint>,
    credentials: VolcengineCredentials,
    proxy: ProviderProxyConfig,
) -> Result<(), String> {
    cancel_enrollment().await;
    if route == CloudModelRoute::Managed {
        let client = CloudGatewayClient::saved()?;
        let result = client
            .request(
                reqwest::Method::DELETE,
                "/v1/voiceprints/binding?mode=managed",
                None,
                None,
            )
            .await?;
        if result["providerDeletion"].as_str() != Some("done") {
            return Err("本机声纹已删除；云端删除结果尚未确认，请在账号内核对".into());
        }
    } else if let Some(profile) = profile {
        CloudVoiceprintClient::new(credentials, &proxy)?
            .delete(&profile)
            .await?;
        if let Ok(client) = CloudGatewayClient::saved() {
            client
                .request(
                    reqwest::Method::DELETE,
                    &format!(
                        "/v1/voiceprints/binding?mode=byok&accountFingerprint={}",
                        profile.account_fingerprint
                    ),
                    None,
                    None,
                )
                .await?;
        }
    }
    Ok(())
}

pub fn overlay_enrollment_status(value: &mut crate::speaker_verification::VoiceprintStatus) {
    let state = status().lock();
    if state.0.is_some() {
        value.local_only = false;
    }
    if state.0.as_deref() == Some("uploading") {
        value.state = "processing".into();
        value.progress = 95;
    }
    if let Some(error) = &state.1 {
        value.error = Some(error.clone());
        value.state = "error".into();
    }
}

pub async fn finish_enrollment(
    capture: Option<EnrollmentCapture>,
    pcm: Vec<u8>,
    phrase: &str,
) -> Option<Result<(CloudModelRoute, CloudOwnerVoiceprint), String>> {
    let EnrollmentCapture(pending) = capture?;
    let outcome = async {
        if pending.phrase != phrase || !pending.client.still_current() {
            return Err("登记期间邮箱账号或唤醒词已改变，请重新录制".into());
        }
        // Called only after local enrollment accepted all three samples and its bounded tail.
        let mut pcm = pcm;
        pcm.truncate(288000);
        if pcm.len() < 280000 {
            return Err("三次声纹录制不完整".into());
        }
        pcm.resize(288000, 0);
        let mut result = Value::Null;
        for (step, sample) in pcm.chunks_exact(96000).enumerate() {
            result = pending
                .client
                .request(
                    reqwest::Method::PUT,
                    &format!("/v1/voiceprints/enrollments/{}/samples/{step}", pending.id),
                    None,
                    Some(sample.to_vec()),
                )
                .await?;
        }
        if let Some(credentials) = pending.byok {
            result = pending
                .client
                .request(
                    reqwest::Method::POST,
                    &format!(
                        "/v1/voiceprints/enrollments/{}/byok-registration-start",
                        pending.id
                    ),
                    None,
                    None,
                )
                .await?;
            let audio_url = result["audioUrl"]
                .as_str()
                .ok_or("自填凭据登记未取得临时音频地址")?;
            let name = result["speakerName"]
                .as_str()
                .ok_or("自填凭据登记未取得本人名称")?;
            let provider = CloudVoiceprintClient::new(credentials, &pending.proxy)?;
            let profile = match provider.register_named(audio_url, name).await {
                Ok(profile) => profile,
                Err(error) => {
                    let _ = pending
                        .client
                        .request(
                            reqwest::Method::POST,
                            &format!("/v1/voiceprints/enrollments/{}/byok-unknown", pending.id),
                            None,
                            None,
                        )
                        .await;
                    return Err(error);
                }
            };
            result = pending
                .client
                .request(
                    reqwest::Method::POST,
                    &format!(
                        "/v1/voiceprints/enrollments/{}/byok-confirmation",
                        pending.id
                    ),
                    Some(json!(profile)),
                    None,
                )
                .await?;
        }
        if result["state"].as_str() != Some("ready") {
            return Err("云端登记结果尚未确认，请先核对状态，不要重复登记".into());
        }
        let profile: CloudOwnerVoiceprint = serde_json::from_value(result["profile"].clone())
            .map_err(|_| "云端本人声纹响应无效")?;
        profile.validate_identity()?;
        if !pending.client.still_current() {
            return Err("邮箱账号已改变，未应用旧账号声纹".into());
        }
        Ok((pending.route, profile))
    }
    .await;
    *status().lock() = match &outcome {
        Ok(_) => (Some("ready".into()), None),
        Err(error) => (Some("error".into()), Some(error.clone())),
    };
    Some(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_service_requires_https_without_credentials_or_query() {
        for url in [
            "",
            "http://cloud.test",
            "https://user:secret@cloud.test",
            "https://cloud.test?token=secret",
            "https://localhost",
        ] {
            assert!(validate_base_url(url).is_err());
        }
        assert!(validate_base_url("https://cloud.example.test").is_ok());
        assert_eq!(
            serde_json::to_value(CloudModelRoute::default()).unwrap(),
            json!("byok")
        );
    }
}
