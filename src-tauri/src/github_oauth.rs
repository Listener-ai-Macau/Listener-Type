use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

use crate::persistence::MarketplaceGithubCredentials;

const GITHUB_BASE_URL: &str = "https://github.com";
const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const USER_AGENT: &str = "Listener Type";
const DEFAULT_INTERVAL_SECS: u32 = 5;
const DEFAULT_DEVICE_EXPIRES_IN_SECS: u32 = 900;
const EXPIRY_SKEW_SECS: i64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GithubDeviceStartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u32,
    pub expires_in: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
    pub expires_in: Option<i64>,
    pub refresh_token: Option<String>,
    pub refresh_token_expires_in: Option<i64>,
}

impl GithubTokenResponse {
    pub fn into_credentials(
        self,
        login: String,
        saved_at_epoch_secs: i64,
    ) -> MarketplaceGithubCredentials {
        MarketplaceGithubCredentials {
            access_token: self.access_token,
            token_type: self.token_type,
            scope: self.scope,
            login,
            expires_at_epoch_secs: self
                .expires_in
                .and_then(|seconds| future_epoch(saved_at_epoch_secs, seconds)),
            refresh_token: self.refresh_token,
            refresh_token_expires_at_epoch_secs: self
                .refresh_token_expires_in
                .and_then(|seconds| future_epoch(saved_at_epoch_secs, seconds)),
            saved_at_epoch_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubUser {
    pub login: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubDevicePollStatus {
    Authorized(GithubTokenResponse),
    Pending,
    SlowDown,
    Expired,
    AccessDenied,
    Error(String),
}

#[derive(Debug)]
pub enum GithubOAuthError {
    InvalidBaseUrl(String),
    Network(String),
    Decode(String),
    HttpStatus { status: u16, message: String },
    OAuth(String),
    MissingAccessToken,
    MissingLogin,
    RefreshUnavailable,
    RefreshExpired,
}

impl fmt::Display for GithubOAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GithubOAuthError::InvalidBaseUrl(message) => {
                write!(f, "invalid GitHub OAuth base URL: {message}")
            }
            GithubOAuthError::Network(message) => {
                write!(f, "GitHub OAuth network failure: {message}")
            }
            GithubOAuthError::Decode(message) => {
                write!(f, "GitHub OAuth response decode failed: {message}")
            }
            GithubOAuthError::HttpStatus { status, message } => {
                write!(f, "GitHub OAuth HTTP {status}: {message}")
            }
            GithubOAuthError::OAuth(message) => write!(f, "GitHub OAuth error: {message}"),
            GithubOAuthError::MissingAccessToken => {
                write!(f, "GitHub OAuth response did not include access_token")
            }
            GithubOAuthError::MissingLogin => {
                write!(f, "GitHub /user response did not include login")
            }
            GithubOAuthError::RefreshUnavailable => write!(
                f,
                "GitHub OAuth token has expired and no refresh_token is available"
            ),
            GithubOAuthError::RefreshExpired => write!(f, "GitHub OAuth refresh_token has expired"),
        }
    }
}

impl std::error::Error for GithubOAuthError {}

#[derive(Clone)]
pub struct GithubOAuthClient {
    client: Client,
    github_base_url: Url,
    api_base_url: Url,
}

impl GithubOAuthClient {
    pub fn production() -> Result<Self, GithubOAuthError> {
        Self::new(GITHUB_BASE_URL, GITHUB_API_BASE_URL)
    }

    pub fn new(github_base_url: &str, api_base_url: &str) -> Result<Self, GithubOAuthError> {
        Ok(Self {
            client: Client::new(),
            github_base_url: parse_base_url(github_base_url)?,
            api_base_url: parse_base_url(api_base_url)?,
        })
    }

    #[cfg(test)]
    fn with_client(
        client: Client,
        github_base_url: &str,
        api_base_url: &str,
    ) -> Result<Self, GithubOAuthError> {
        Ok(Self {
            client,
            github_base_url: parse_base_url(github_base_url)?,
            api_base_url: parse_base_url(api_base_url)?,
        })
    }

    pub async fn start_device_flow(
        &self,
        client_id: &str,
        scope: &str,
    ) -> Result<GithubDeviceStartResponse, GithubOAuthError> {
        let url = self.github_url(&["login", "device", "code"])?;
        let response = self
            .client
            .post(url)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .form(&[("client_id", client_id), ("scope", scope)])
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|error| GithubOAuthError::Network(error.to_string()))?;
        let status = response.status();
        let body = decode_json_response(response).await?;
        if !status.is_success() {
            return Err(GithubOAuthError::HttpStatus {
                status: status.as_u16(),
                message: github_error_message(&body),
            });
        }
        Ok(GithubDeviceStartResponse {
            device_code: required_string_field(&body, "device_code")?,
            user_code: required_string_field(&body, "user_code")?,
            verification_uri: required_string_field(&body, "verification_uri")?,
            interval: body["interval"]
                .as_u64()
                .unwrap_or(DEFAULT_INTERVAL_SECS as u64) as u32,
            expires_in: body["expires_in"]
                .as_u64()
                .unwrap_or(DEFAULT_DEVICE_EXPIRES_IN_SECS as u64) as u32,
        })
    }

    pub async fn poll_device_flow(
        &self,
        client_id: &str,
        device_code: &str,
    ) -> Result<GithubDevicePollStatus, GithubOAuthError> {
        let url = self.github_url(&["login", "oauth", "access_token"])?;
        let response = self
            .client
            .post(url)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .form(&[
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|error| GithubOAuthError::Network(error.to_string()))?;
        self.parse_token_response(response).await
    }

    pub async fn refresh_access_token(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<GithubTokenResponse, GithubOAuthError> {
        let url = self.github_url(&["login", "oauth", "access_token"])?;
        let response = self
            .client
            .post(url)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .form(&[
                ("client_id", client_id),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|error| GithubOAuthError::Network(error.to_string()))?;
        match self.parse_token_response(response).await? {
            GithubDevicePollStatus::Authorized(token) => Ok(token),
            GithubDevicePollStatus::Error(message) => Err(GithubOAuthError::OAuth(message)),
            GithubDevicePollStatus::Expired => Err(GithubOAuthError::RefreshExpired),
            GithubDevicePollStatus::AccessDenied => {
                Err(GithubOAuthError::OAuth("access_denied".into()))
            }
            GithubDevicePollStatus::Pending | GithubDevicePollStatus::SlowDown => Err(
                GithubOAuthError::OAuth("unexpected refresh_token polling state".into()),
            ),
        }
    }

    pub async fn authenticated_user(
        &self,
        access_token: &str,
    ) -> Result<GithubUser, GithubOAuthError> {
        let url = self.api_url(&["user"])?;
        let response = self
            .client
            .get(url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/vnd.github+json")
            .bearer_auth(access_token)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|error| GithubOAuthError::Network(error.to_string()))?;
        let status = response.status();
        let body = decode_json_response(response).await?;
        if !status.is_success() {
            return Err(GithubOAuthError::HttpStatus {
                status: status.as_u16(),
                message: github_error_message(&body),
            });
        }
        let login = body["login"].as_str().unwrap_or("").trim().to_string();
        if login.is_empty() {
            return Err(GithubOAuthError::MissingLogin);
        }
        Ok(GithubUser { login })
    }

    async fn parse_token_response(
        &self,
        response: reqwest::Response,
    ) -> Result<GithubDevicePollStatus, GithubOAuthError> {
        let status = response.status();
        let body = decode_json_response(response).await?;
        if !status.is_success() {
            return Err(GithubOAuthError::HttpStatus {
                status: status.as_u16(),
                message: github_error_message(&body),
            });
        }
        if let Some(access_token) = body["access_token"].as_str() {
            if access_token.trim().is_empty() {
                return Err(GithubOAuthError::MissingAccessToken);
            }
            return Ok(GithubDevicePollStatus::Authorized(GithubTokenResponse {
                access_token: access_token.to_string(),
                token_type: body["token_type"].as_str().unwrap_or("bearer").to_string(),
                scope: body["scope"].as_str().unwrap_or("").to_string(),
                expires_in: body["expires_in"].as_i64(),
                refresh_token: body["refresh_token"].as_str().map(ToString::to_string),
                refresh_token_expires_in: body["refresh_token_expires_in"].as_i64(),
            }));
        }

        let error = body["error"].as_str().unwrap_or("");
        let result = match error {
            "authorization_pending" => GithubDevicePollStatus::Pending,
            "slow_down" => GithubDevicePollStatus::SlowDown,
            "expired_token" => GithubDevicePollStatus::Expired,
            "access_denied" => GithubDevicePollStatus::AccessDenied,
            other if !other.is_empty() => {
                GithubDevicePollStatus::Error(github_error_message(&body))
            }
            _ => return Err(GithubOAuthError::MissingAccessToken),
        };
        Ok(result)
    }

    fn github_url(&self, segments: &[&str]) -> Result<Url, GithubOAuthError> {
        append_segments(&self.github_base_url, segments)
    }

    fn api_url(&self, segments: &[&str]) -> Result<Url, GithubOAuthError> {
        append_segments(&self.api_base_url, segments)
    }
}

pub fn current_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub fn token_needs_refresh(
    credentials: &MarketplaceGithubCredentials,
    now_epoch_secs: i64,
) -> bool {
    credentials
        .expires_at_epoch_secs
        .map(|expires_at| expires_at <= now_epoch_secs + EXPIRY_SKEW_SECS)
        .unwrap_or(false)
}

pub fn refresh_token_is_expired(
    credentials: &MarketplaceGithubCredentials,
    now_epoch_secs: i64,
) -> bool {
    credentials
        .refresh_token_expires_at_epoch_secs
        .map(|expires_at| expires_at <= now_epoch_secs + EXPIRY_SKEW_SECS)
        .unwrap_or(false)
}

fn parse_base_url(value: &str) -> Result<Url, GithubOAuthError> {
    let mut url =
        Url::parse(value).map_err(|error| GithubOAuthError::InvalidBaseUrl(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(GithubOAuthError::InvalidBaseUrl(
            "base URL must use http or https".into(),
        ));
    }
    if url.path().is_empty() {
        url.set_path("/");
    }
    Ok(url)
}

fn append_segments(base: &Url, segments: &[&str]) -> Result<Url, GithubOAuthError> {
    let mut url = base.clone();
    {
        let mut path_segments = url
            .path_segments_mut()
            .map_err(|_| GithubOAuthError::InvalidBaseUrl("base URL cannot be a base".into()))?;
        path_segments.pop_if_empty();
        for segment in segments {
            path_segments.push(segment);
        }
    }
    Ok(url)
}

fn github_error_message(body: &serde_json::Value) -> String {
    let error = body["error"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("unknown_error");
    let description = body["error_description"].as_str().unwrap_or("");
    if description.is_empty() {
        error.to_string()
    } else {
        format!("{error}: {description}")
    }
}

fn required_string_field(
    body: &serde_json::Value,
    field: &str,
) -> Result<String, GithubOAuthError> {
    body[field]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| GithubOAuthError::Decode(format!("missing required field: {field}")))
}

async fn decode_json_response(
    response: reqwest::Response,
) -> Result<serde_json::Value, GithubOAuthError> {
    let text = response
        .text()
        .await
        .map_err(|error| GithubOAuthError::Decode(format!("read body: {error:#}")))?;
    serde_json::from_str(&text)
        .map_err(|error| GithubOAuthError::Decode(format!("parse json: {error}")))
}

fn future_epoch(now_epoch_secs: i64, duration_secs: i64) -> Option<i64> {
    if duration_secs <= 0 {
        None
    } else {
        Some(now_epoch_secs.saturating_add(duration_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    fn spawn_sequence(responses: Vec<String>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 8192];
                let mut request = Vec::new();
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request_headers = String::from_utf8_lossy(&request);
                let content_length = request_headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                let header_len = request
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|pos| pos + 4)
                    .unwrap_or(request.len());
                while request.len().saturating_sub(header_len) < content_length {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                tx.send(String::from_utf8_lossy(&request).to_string())
                    .unwrap();
                stream.write_all(response.as_bytes()).unwrap();
                stream.flush().unwrap();
                let _ = stream.shutdown(Shutdown::Write);
            }
        });
        (format!("http://{addr}"), rx)
    }

    fn json_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.as_bytes().len(),
            body
        )
    }

    #[tokio::test]
    async fn device_flow_posts_to_github_contract_paths() {
        let (base, requests) = spawn_sequence(vec![
            json_response(
                r#"{"device_code":"dev","user_code":"USER-CODE","verification_uri":"https://github.com/login/device","interval":7,"expires_in":600}"#,
            ),
            json_response(
                r#"{"access_token":"access","token_type":"bearer","scope":"read:user","expires_in":28800,"refresh_token":"refresh","refresh_token_expires_in":15811200}"#,
            ),
            json_response(r#"{"login":"octocat"}"#),
        ]);
        let client = GithubOAuthClient::with_client(test_http_client(), &base, &base).unwrap();

        let start = client
            .start_device_flow("client123", "read:user")
            .await
            .unwrap();
        assert_eq!(start.user_code, "USER-CODE");
        let poll = client.poll_device_flow("client123", "dev").await.unwrap();
        let token = match poll {
            GithubDevicePollStatus::Authorized(token) => token,
            other => panic!("expected authorized token, got {other:?}"),
        };
        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh"));
        let user = client
            .authenticated_user(&token.access_token)
            .await
            .unwrap();
        assert_eq!(user.login, "octocat");

        let request0 = requests.recv().unwrap();
        let request1 = requests.recv().unwrap();
        let request2 = requests.recv().unwrap();
        assert!(request0.starts_with("POST /login/device/code "));
        assert!(request0.contains("client_id=client123"));
        assert!(request0.contains("scope=read%3Auser"));
        assert!(request1.starts_with("POST /login/oauth/access_token "));
        assert!(request1.contains("device_code=dev"));
        assert!(request2.starts_with("GET /user "));
        assert!(request2
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer access")));
    }

    #[tokio::test]
    async fn device_flow_rejects_success_response_missing_device_code() {
        let (base, _requests) = spawn_sequence(vec![json_response(
            r#"{"user_code":"USER-CODE","verification_uri":"https://github.com/login/device"}"#,
        )]);
        let client = GithubOAuthClient::with_client(test_http_client(), &base, &base).unwrap();

        let error = client
            .start_device_flow("client123", "read:user")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("device_code"));
    }

    #[tokio::test]
    async fn token_poll_classifies_pending_and_slow_down() {
        let (base, requests) = spawn_sequence(vec![
            json_response(r#"{"error":"authorization_pending"}"#),
            json_response(r#"{"error":"slow_down"}"#),
        ]);
        let client = GithubOAuthClient::with_client(test_http_client(), &base, &base).unwrap();

        assert_eq!(
            client.poll_device_flow("client", "device").await.unwrap(),
            GithubDevicePollStatus::Pending
        );
        assert_eq!(
            client.poll_device_flow("client", "device").await.unwrap(),
            GithubDevicePollStatus::SlowDown
        );
        assert!(requests
            .recv()
            .unwrap()
            .starts_with("POST /login/oauth/access_token "));
        assert!(requests
            .recv()
            .unwrap()
            .starts_with("POST /login/oauth/access_token "));
    }

    #[tokio::test]
    async fn refresh_access_token_uses_refresh_grant() {
        let (base, requests) = spawn_sequence(vec![json_response(
            r#"{"access_token":"new-access","token_type":"bearer","scope":"read:user"}"#,
        )]);
        let client = GithubOAuthClient::with_client(test_http_client(), &base, &base).unwrap();

        let token = client
            .refresh_access_token("client123", "refresh456")
            .await
            .unwrap();

        assert_eq!(token.access_token, "new-access");
        let request = requests.recv().unwrap();
        assert!(request.starts_with("POST /login/oauth/access_token "));
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("refresh_token=refresh456"));
    }

    #[test]
    fn token_refresh_state_uses_expiry_skew_and_allows_non_expiring_tokens() {
        let mut credentials = MarketplaceGithubCredentials {
            access_token: "token".into(),
            expires_at_epoch_secs: None,
            ..Default::default()
        };
        assert!(!token_needs_refresh(&credentials, 1000));

        credentials.expires_at_epoch_secs = Some(1100);
        assert!(!token_needs_refresh(&credentials, 1000));
        assert!(token_needs_refresh(&credentials, 1040));
    }

    fn test_http_client() -> Client {
        Client::builder().no_proxy().build().unwrap()
    }
}
