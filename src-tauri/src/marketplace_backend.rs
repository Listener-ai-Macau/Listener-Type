use std::fmt;
use std::time::Duration;

use bytes::Bytes;
use reqwest::{multipart, StatusCode, Url};
use serde::{Deserialize, Serialize};

pub const MARKETPLACE_API_CONTRACT_VERSION: &str = "listener-type-marketplace-v1";
pub const STYLES_PATH: &str = "styles";
pub const STYLE_UPLOAD_PATH: &[&str] = &["styles", "upload"];

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceListItem {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub author_login: String,
    pub version: String,
    pub base_mode: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub like_count: i64,
    pub download_count: i64,
    pub published_at: String,
    pub updated_at: String,
    pub origin_pack_id: Option<String>,
    pub origin_author_login: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceListPage {
    #[serde(default)]
    pub items: Vec<MarketplaceListItem>,
    #[serde(default)]
    pub next_offset: Option<u32>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub total: Option<u32>,
}

impl MarketplaceListPage {
    pub fn empty() -> Self {
        Self::default()
    }

    fn from_legacy_items(
        items: Vec<MarketplaceListItem>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Self {
        let item_count = items.len() as u32;
        let has_more = match limit {
            Some(limit) if limit > 0 => item_count >= limit,
            _ => false,
        };
        Self {
            items,
            next_offset: if has_more {
                Some(offset.unwrap_or(0).saturating_add(item_count))
            } else {
                None
            },
            has_more,
            total: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MarketplaceListResponse {
    Page(MarketplaceListPage),
    Items(Vec<MarketplaceListItem>),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceDetail {
    #[serde(flatten)]
    pub summary: MarketplaceListItem,
    pub prompt: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceMyPackItem {
    #[serde(flatten)]
    pub summary: MarketplaceListItem,
    pub state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketplaceApiErrorKind {
    InvalidUrl,
    Network,
    Unauthorized,
    NotFound,
    HttpStatus,
    Decode,
}

#[derive(Debug, Clone)]
pub struct MarketplaceApiError {
    kind: MarketplaceApiErrorKind,
    status: Option<u16>,
    message: String,
}

impl MarketplaceApiError {
    fn invalid_url(error: impl fmt::Display) -> Self {
        Self {
            kind: MarketplaceApiErrorKind::InvalidUrl,
            status: None,
            message: format!("invalid marketplace url: {error}"),
        }
    }

    fn network(error: reqwest::Error) -> Self {
        Self {
            kind: MarketplaceApiErrorKind::Network,
            status: None,
            message: format!("marketplace network failure: {error}"),
        }
    }

    fn decode(error: reqwest::Error) -> Self {
        Self {
            kind: MarketplaceApiErrorKind::Decode,
            status: None,
            message: format!("marketplace response decode failed: {error}"),
        }
    }

    fn status(status: StatusCode, body: String) -> Self {
        let kind = match status {
            StatusCode::UNAUTHORIZED => MarketplaceApiErrorKind::Unauthorized,
            StatusCode::NOT_FOUND => MarketplaceApiErrorKind::NotFound,
            _ => MarketplaceApiErrorKind::HttpStatus,
        };
        let body = body.trim();
        let detail = if body.is_empty() {
            status.to_string()
        } else {
            format!("{status}: {body}")
        };
        Self {
            kind,
            status: Some(status.as_u16()),
            message: format!("marketplace HTTP {detail}"),
        }
    }

    pub fn kind(&self) -> MarketplaceApiErrorKind {
        self.kind
    }

    pub fn status_code(&self) -> Option<u16> {
        self.status
    }
}

impl fmt::Display for MarketplaceApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MarketplaceApiError {}

#[derive(Clone)]
pub struct MarketplaceClient {
    base_url: Url,
    client: reqwest::Client,
    timeout: Duration,
}

impl MarketplaceClient {
    pub fn new(base_url: &str) -> Result<Self, MarketplaceApiError> {
        Self::with_client(base_url, reqwest::Client::new())
    }

    fn with_client(base_url: &str, client: reqwest::Client) -> Result<Self, MarketplaceApiError> {
        let mut base_url = Url::parse(base_url.trim()).map_err(MarketplaceApiError::invalid_url)?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(MarketplaceApiError::invalid_url(
                "marketplace url must use http or https",
            ));
        }
        if base_url.path().is_empty() {
            base_url.set_path("/");
        }
        Ok(Self {
            base_url,
            client,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    fn url(&self, segments: &[&str]) -> Result<Url, MarketplaceApiError> {
        let mut url = self.base_url.clone();
        {
            let mut path_segments = url
                .path_segments_mut()
                .map_err(|_| MarketplaceApiError::invalid_url("base url cannot be a base"))?;
            path_segments.pop_if_empty();
            for segment in segments {
                path_segments.push(segment);
            }
        }
        Ok(url)
    }

    async fn execute(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, MarketplaceApiError> {
        let response = request
            .timeout(self.timeout)
            .send()
            .await
            .map_err(MarketplaceApiError::network)?;
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(MarketplaceApiError::status(status, body))
        }
    }

    async fn json<T: for<'de> Deserialize<'de>>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, MarketplaceApiError> {
        self.execute(request)
            .await?
            .json::<T>()
            .await
            .map_err(MarketplaceApiError::decode)
    }

    pub async fn list_styles(
        &self,
        query: Option<&str>,
        category: Option<&str>,
        sort: Option<&str>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<MarketplaceListPage, MarketplaceApiError> {
        let mut url = self.url(&[STYLES_PATH])?;
        if let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) {
            url.query_pairs_mut().append_pair("q", query);
        }
        if let Some(category) = category.map(str::trim).filter(|value| !value.is_empty()) {
            url.query_pairs_mut().append_pair("category", category);
        }
        if let Some(sort) = sort.map(str::trim).filter(|value| !value.is_empty()) {
            url.query_pairs_mut().append_pair("sort", sort);
        }
        if let Some(limit) = limit {
            url.query_pairs_mut()
                .append_pair("limit", &limit.to_string());
        }
        if let Some(offset) = offset {
            url.query_pairs_mut()
                .append_pair("offset", &offset.to_string());
        }

        match self.json(self.client.get(url)).await? {
            MarketplaceListResponse::Page(mut page) => {
                if page.has_more && page.next_offset.is_none() {
                    page.next_offset =
                        Some(offset.unwrap_or(0).saturating_add(page.items.len() as u32));
                }
                if !page.has_more {
                    page.next_offset = None;
                }
                Ok(page)
            }
            MarketplaceListResponse::Items(items) => {
                Ok(MarketplaceListPage::from_legacy_items(items, limit, offset))
            }
        }
    }

    pub async fn style_detail(
        &self,
        style_id: &str,
    ) -> Result<MarketplaceDetail, MarketplaceApiError> {
        let url = self.url(&[STYLES_PATH, style_id])?;
        self.json(self.client.get(url)).await
    }

    pub async fn download_style_archive(
        &self,
        style_id: &str,
    ) -> Result<Bytes, MarketplaceApiError> {
        let url = self.url(&[STYLES_PATH, style_id, "download"])?;
        self.execute(self.client.get(url))
            .await?
            .bytes()
            .await
            .map_err(MarketplaceApiError::decode)
    }

    pub async fn upload_style_archive(
        &self,
        local_pack_id: &str,
        origin_pack_id: Option<&str>,
        archive_bytes: Vec<u8>,
        dev_user: &str,
    ) -> Result<serde_json::Value, MarketplaceApiError> {
        let part = multipart::Part::bytes(archive_bytes)
            .file_name(format!("{local_pack_id}.zip"))
            .mime_str("application/zip")
            .map_err(|error| MarketplaceApiError {
                kind: MarketplaceApiErrorKind::Decode,
                status: None,
                message: format!("marketplace multipart build failed: {error}"),
            })?;
        let mut form = multipart::Form::new().part("file", part);
        if let Some(origin_pack_id) = origin_pack_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            form = form.text("originPackId", origin_pack_id.to_string());
        }

        let url = self.url(STYLE_UPLOAD_PATH)?;
        self.json(
            self.client
                .post(url)
                .header("X-Dev-User", dev_user)
                .multipart(form),
        )
        .await
    }

    pub async fn like_style(
        &self,
        style_id: &str,
        dev_user: &str,
    ) -> Result<serde_json::Value, MarketplaceApiError> {
        let url = self.url(&[STYLES_PATH, style_id, "like"])?;
        self.json(self.client.post(url).header("X-Dev-User", dev_user))
            .await
    }

    pub async fn delete_style(
        &self,
        style_id: &str,
        dev_user: &str,
    ) -> Result<(), MarketplaceApiError> {
        let url = self.url(&[STYLES_PATH, style_id])?;
        self.execute(self.client.delete(url).header("X-Dev-User", dev_user))
            .await
            .map(|_| ())
    }

    pub async fn my_likes(&self, dev_user: &str) -> Result<Vec<String>, MarketplaceApiError> {
        let url = self.url(&["me", "likes"])?;
        self.json(self.client.get(url).header("X-Dev-User", dev_user))
            .await
    }

    pub async fn my_styles(
        &self,
        dev_user: &str,
    ) -> Result<Vec<MarketplaceMyPackItem>, MarketplaceApiError> {
        let url = self.url(&["me", "styles"])?;
        self.json(self.client.get(url).header("X-Dev-User", dev_user))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    const STYLE_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn test_marketplace_client(base_url: &str) -> MarketplaceClient {
        MarketplaceClient::with_client(
            base_url,
            reqwest::Client::builder().no_proxy().build().unwrap(),
        )
        .unwrap()
    }

    fn read_http_request(mut stream: TcpStream) -> Vec<u8> {
        let mut buf = [0u8; 8192];
        let mut request = Vec::new();
        let mut headers_end = None;
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                headers_end = Some(pos + 4);
                break;
            }
        }

        if let Some(headers_end) = headers_end {
            let content_length = {
                let header_text = String::from_utf8_lossy(&request[..headers_end]);
                header_text
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0)
            };
            while request.len() < headers_end + content_length {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
        }

        request
    }

    fn spawn_response(status: &str, body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(stream.try_clone().unwrap());
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).to_string()
        });
        (format!("http://{addr}"), handle)
    }

    #[test]
    fn marketplace_client_percent_encodes_style_id_path_segments() {
        let client = test_marketplace_client("http://127.0.0.1:65535/api/");
        let url = client
            .url(&[STYLES_PATH, "pack/with slash"])
            .expect("URL builds");

        assert_eq!(url.path(), "/api/styles/pack%2Fwith%20slash");
    }

    #[tokio::test]
    async fn marketplace_client_uses_styles_contract_paths() {
        let body = r#"{"items":[{"id":"550e8400-e29b-41d4-a716-446655440000","slug":"demo","name":"Demo","description":"","authorLogin":"alice","version":"1.0.0","baseMode":"structured","tags":[],"likeCount":1,"downloadCount":2,"publishedAt":"2026-06-01T00:00:00Z","updatedAt":"2026-06-01T00:00:00Z"}],"nextOffset":30,"hasMore":true,"total":52}"#;
        let (base, request_handle) = spawn_response("200 OK", body);

        let client = test_marketplace_client(&base);
        let page = client
            .list_styles(
                Some("demo pack"),
                Some("structured"),
                Some("popular"),
                Some(10),
                Some(20),
            )
            .await
            .unwrap();

        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].slug, "demo");
        assert_eq!(page.next_offset, Some(30));
        assert!(page.has_more);
        assert_eq!(page.total, Some(52));
        let request = request_handle.join().unwrap();
        assert!(request.starts_with("GET /styles?"));
        assert!(request.contains("q=demo+pack"));
        assert!(request.contains("category=structured"));
        assert!(request.contains("sort=popular"));
        assert!(request.contains("limit=10"));
        assert!(request.contains("offset=20"));
    }

    #[tokio::test]
    async fn marketplace_client_accepts_legacy_array_list_response() {
        let body = r#"[{"id":"550e8400-e29b-41d4-a716-446655440000","slug":"demo","name":"Demo","description":"","authorLogin":"alice","version":"1.0.0","baseMode":"structured","tags":[],"likeCount":1,"downloadCount":2,"publishedAt":"2026-06-01T00:00:00Z","updatedAt":"2026-06-01T00:00:00Z"}]"#;
        let (base, _request_handle) = spawn_response("200 OK", body);

        let client = test_marketplace_client(&base);
        let page = client
            .list_styles(None, None, Some("popular"), Some(1), Some(5))
            .await
            .unwrap();

        assert_eq!(page.items.len(), 1);
        assert!(page.has_more);
        assert_eq!(page.next_offset, Some(6));
        assert_eq!(page.total, None);
    }

    #[tokio::test]
    async fn marketplace_client_downloads_style_archive_bytes() {
        let (base, request_handle) = spawn_response("200 OK", "ZIPDATA");

        let client = test_marketplace_client(&base);
        let bytes = client.download_style_archive(STYLE_ID).await.unwrap();

        assert_eq!(&bytes[..], b"ZIPDATA");
        let request = request_handle.join().unwrap();
        assert!(request.starts_with(&format!("GET /styles/{STYLE_ID}/download ")));
    }

    #[tokio::test]
    async fn marketplace_client_uploads_style_archive_with_user_and_origin() {
        let body =
            r#"{"id":"550e8400-e29b-41d4-a716-446655440000","state":"pending","message":"ok"}"#;
        let (base, request_handle) = spawn_response("200 OK", body);

        let client = test_marketplace_client(&base);
        let response = client
            .upload_style_archive("local.pack", Some(STYLE_ID), b"ZIPDATA".to_vec(), "alice")
            .await
            .unwrap();

        assert_eq!(response["state"], "pending");
        let request = request_handle.join().unwrap();
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with("POST /styles/upload "));
        assert!(lower.contains("x-dev-user: alice"));
        assert!(request.contains(r#"filename="local.pack.zip""#));
        assert!(request.contains(r#"name="originPackId""#));
        assert!(request.contains(STYLE_ID));
    }

    #[tokio::test]
    async fn marketplace_client_classifies_unauthorized() {
        let (base, request_handle) = spawn_response("401 Unauthorized", r#"{"error":"auth"}"#);

        let client = test_marketplace_client(&base);
        let error = client.style_detail(STYLE_ID).await.unwrap_err();

        assert_eq!(error.kind(), MarketplaceApiErrorKind::Unauthorized);
        assert_eq!(error.status_code(), Some(401));
        assert!(request_handle
            .join()
            .unwrap()
            .starts_with(&format!("GET /styles/{STYLE_ID} ")));
    }

    #[tokio::test]
    async fn marketplace_client_classifies_not_found() {
        let (base, _request_handle) = spawn_response("404 Not Found", r#"{"error":"missing"}"#);

        let client = test_marketplace_client(&base);
        let error = client.style_detail(STYLE_ID).await.unwrap_err();

        assert_eq!(error.kind(), MarketplaceApiErrorKind::NotFound);
        assert_eq!(error.status_code(), Some(404));
    }

    #[tokio::test]
    async fn marketplace_client_classifies_network_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let client = test_marketplace_client(&format!("http://{addr}"));
        let error = client
            .list_styles(None, None, None, None, None)
            .await
            .unwrap_err();

        assert_eq!(error.kind(), MarketplaceApiErrorKind::Network);
        assert_eq!(error.status_code(), None);
    }
}
