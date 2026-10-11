//! Volcengine ASR voiceprint registry. Separate from local wake templates and RTC IDs.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::volcengine::VolcengineCredentials;
use crate::polish::ProviderProxyConfig;

const ENDPOINT: &str = "https://openspeech.bytedance.com/api/proxy/invoke/";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudOwnerVoiceprint {
    pub id: String,
    pub speaker_name: String,
    pub account_fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudVoiceprintRecord {
    pub id: String,
    pub speaker_name: String,
}

pub(crate) fn account_fingerprint(app_id: &str) -> String {
    format!("{:x}", Sha256::digest(app_id.trim().as_bytes()))
}

impl CloudOwnerVoiceprint {
    pub fn validate_identity(&self) -> Result<(), String> {
        validate_record(&self.id, &self.speaker_name)?;
        if self.account_fingerprint.len() != 64 || !self.account_fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("云端声纹账户标识无效".into());
        }
        Ok(())
    }

    pub fn validate_for(&self, credentials: &VolcengineCredentials) -> Result<(), String> {
        if self.account_fingerprint != account_fingerprint(&credentials.app_id) {
            return Err("云端本人声纹属于另一火山账号，请重新选择声纹".into());
        }
        validate_record(&self.id, &self.speaker_name)
    }
}

fn validate_record(id: &str, name: &str) -> Result<(), String> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err("云端声纹 ID 无效，请从账号的登记列表选择".into());
    }
    let original_name = name;
    let name = name.trim();
    if name.is_empty() || name != original_name || name.parse::<i64>().is_ok()
        || matches!(name.to_ascii_lowercase().as_str(), "unknown" | "speaker" | "unassigned")
    {
        return Err("云端声纹名称不能为空或匿名说话人编号，请在火山控制台修改名称".into());
    }
    Ok(())
}

fn checked_result(payload: &Value) -> Result<&Value, String> {
    if let Some(error) = payload.pointer("/ResponseMetadata/Error") {
        if !error.is_null() {
            let code = error.get("Code").and_then(Value::as_str).unwrap_or("unknown");
            return Err(format!("火山云端声纹请求被拒绝（{code}），请检查声纹服务权限"));
        }
    }
    let result = payload.get("Result").ok_or("云端声纹响应缺少 Result")?;
    if result.get("Code").and_then(Value::as_i64) != Some(1000) {
        let code = result.get("Code").and_then(Value::as_i64).unwrap_or(-1);
        return Err(format!("火山云端声纹请求失败（{code}），请检查音频和服务权限"));
    }
    Ok(result)
}

fn voiceprint_id(value: &Value) -> Result<&str, String> {
    // The live registry returns SpeakID; published examples use SpeakId.
    // An ambiguous response cannot bind a different customer's identity.
    let live = value.get("SpeakID").and_then(Value::as_str);
    let documented = value.get("SpeakId").and_then(Value::as_str);
    if live.zip(documented).is_some_and(|(a, b)| a != b) {
        return Err("云端声纹响应 ID 不一致，未绑定本人".into());
    }
    live.or(documented).ok_or_else(|| "云端声纹响应缺少 ID".into())
}

fn records(result: &Value) -> Result<Vec<CloudVoiceprintRecord>, String> {
    let rows = result.get("AudioEmbedding").and_then(Value::as_array)
        .ok_or("云端声纹响应缺少 AudioEmbedding 列表")?;
    rows.iter().filter(|row| row.get("Status").and_then(Value::as_i64) == Some(1))
        .map(|row| {
            let id = voiceprint_id(row)?;
            let name = row.get("SpeakerName").and_then(Value::as_str).ok_or("声纹记录缺少 SpeakerName")?;
            Ok(CloudVoiceprintRecord { id: id.into(), speaker_name: name.into() })
        }).collect()
}

pub struct CloudVoiceprintClient {
    credentials: VolcengineCredentials,
    client: reqwest::Client,
    #[cfg(test)]
    test_endpoint: Option<String>,
}

enum RegistryRequestError {
    OutcomeUnknown(String),
    Rejected(String),
}

impl RegistryRequestError {
    fn message(self) -> String {
        match self {
            Self::OutcomeUnknown(message) | Self::Rejected(message) => message,
        }
    }
}

fn unique_named_registration<'a>(rows: &'a [CloudVoiceprintRecord], name: &str) -> Result<&'a str, String> {
    let mut matches = rows.iter().filter(|row| row.speaker_name == name);
    let record = matches.next().ok_or("未能确认本次云端登记，结果仍待核对")?;
    if matches.next().is_some() {
        return Err("本次云端登记有多条匹配记录，未绑定本人".into());
    }
    validate_record(&record.id, name)?;
    Ok(&record.id)
}

impl CloudVoiceprintClient {
    pub fn new(credentials: VolcengineCredentials, proxy: &ProviderProxyConfig) -> Result<Self, String> {
        if credentials.app_id.trim().is_empty() || credentials.access_token.trim().is_empty() {
            return Err("请先配置火山 App ID 和 Access Key".into());
        }
        let client = crate::polish::http_client_builder_with_proxy(ENDPOINT, 20, proxy)
            // Never forward authentication to a redirect destination.
            .redirect(reqwest::redirect::Policy::none())
            .build().map_err(|_| "无法建立云端声纹客户端".to_string())?;
        Ok(Self { credentials, client, #[cfg(test)] test_endpoint: None })
    }

    async fn request(&self, action: &str, body: Value) -> Result<Value, String> {
        self.checked_request(action, body).await.map_err(RegistryRequestError::message)
    }

    async fn checked_request(&self, action: &str, body: Value) -> Result<Value, RegistryRequestError> {
        #[cfg(test)]
        let endpoint = self.test_endpoint.as_deref().unwrap_or(ENDPOINT);
        #[cfg(not(test))]
        let endpoint = ENDPOINT;
        let response = self.client.post(endpoint).query(&[("Action", action)])
            .header("X-Api-App-Id", &self.credentials.app_id)
            .header("X-Api-Access-Key", &self.credentials.access_token)
            .json(&body).send().await
            // Do not expose reqwest errors: a registration URL can be signed.
            .map_err(|_| RegistryRequestError::OutcomeUnknown(
                "云端声纹请求未完成，请检查网络；登记超时后请先核对状态，避免重复登记".into()))?;
        if !response.status().is_success() {
            let message = format!("云端声纹 HTTP {}，请检查服务权限或网络", response.status().as_u16());
            return Err(if response.status().is_server_error() {
                RegistryRequestError::OutcomeUnknown(message)
            } else {
                RegistryRequestError::Rejected(message)
            });
        }
        let payload: Value = response.json().await
            .map_err(|_| RegistryRequestError::OutcomeUnknown("云端声纹响应不是有效 JSON".into()))?;
        checked_result(&payload).map_err(RegistryRequestError::Rejected)?;
        Ok(payload)
    }

    async fn reconcile_registration(&self, name: &str) -> Result<CloudOwnerVoiceprint, String> {
        // One bounded read-only lookup. Never repeat the create operation or
        // select a different customer's record to fill an uncertain result.
        let rows = self.list().await?;
        let id = unique_named_registration(&rows, name)?;
        let profile = self.select(id).await?;
        if profile.speaker_name != name {
            return Err("云端登记名称不匹配，未绑定本人".into());
        }
        Ok(profile)
    }

    pub async fn list(&self) -> Result<Vec<CloudVoiceprintRecord>, String> {
        let payload = self.request("ListVoiceprint", json!({"Limit":50,"Offset":0})).await?;
        records(checked_result(&payload)?)
    }

    pub async fn select(&self, id: &str) -> Result<CloudOwnerVoiceprint, String> {
        if uuid::Uuid::parse_str(id).is_err() { return Err("云端声纹 ID 无效".into()); }
        let payload = self.request("GetVoiceprint", json!({"SpeakIdList":[id]})).await?;
        let record = records(checked_result(&payload)?)?.into_iter().find(|row| row.id == id)
            .ok_or("云端本人声纹不存在或已删除，请重新登记或选择")?;
        validate_record(&record.id, &record.speaker_name)?;
        Ok(CloudOwnerVoiceprint { id: record.id, speaker_name: record.speaker_name,
            account_fingerprint: account_fingerprint(&self.credentials.app_id) })
    }

    pub async fn register(&self, audio_url: &str) -> Result<CloudOwnerVoiceprint, String> {
        self.register_named(audio_url, &format!("listener_owner_{}", uuid::Uuid::new_v4().simple())).await
    }

    pub async fn delete(&self, profile: &CloudOwnerVoiceprint) -> Result<(), String> {
        profile.validate_for(&self.credentials)?;
        self.request("UpdateVoiceprint",json!({"Action":2,"SpeakId":profile.id})).await?;
        Ok(())
    }

    pub async fn register_named(&self, audio_url: &str, name: &str) -> Result<CloudOwnerVoiceprint, String> {
        if !name.starts_with("listener_owner_") || name.len()!=47 || !name[15..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("云端登记名称无效".into());
        }
        let url = reqwest::Url::parse(audio_url.trim()).map_err(|_| "请输入可访问的 HTTPS 音频链接")?;
        if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty()
            || url.password().is_some() || matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
        { return Err("登记需要云端可读取的 HTTPS 音频链接，本机路径不能使用".into()); }
        let payload = match self.checked_request("UpdateVoiceprint",
            json!({"Action":0,"AudioUrl":url.as_str(),"SpeakerName":name})).await {
            Ok(payload) => payload,
            Err(RegistryRequestError::OutcomeUnknown(message)) => {
                return self.reconcile_registration(name).await.map_err(|_| message);
            }
            Err(RegistryRequestError::Rejected(message)) => return Err(message),
        };
        let id = voiceprint_id(checked_result(&payload)?)?;
        let profile = self.select(id).await?;
        if profile.speaker_name != name { return Err("云端登记名称不匹配，未绑定本人".into()); }
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "a3ee18be-6113-4b9a-8bc1-55fbb9f88f71";

    #[tokio::test]
    async fn registration_lost_response_is_confirmed_without_a_second_create() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::{Duration, Instant};
        let name = "listener_owner_01234567890123456789012345678901";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut actions = Vec::new();
            let mut delayed = Vec::new();
            while actions.len() < 3 && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                // Windows may inherit the listener's nonblocking mode. Request
                // bodies must use the read deadline while accept stays bounded.
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_millis(250))).unwrap();
                let mut data = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let bytes = stream.read(&mut chunk).unwrap();
                    assert!(bytes > 0);
                    data.extend_from_slice(&chunk[..bytes]);
                    assert!(data.len() < 16384);
                    if let Some(end) = data.windows(4).position(|part| part == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&data[..end]);
                        let length: usize = header.lines().find_map(|line| {
                            line.to_ascii_lowercase().strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse().ok())
                        }).unwrap();
                        if data.len() >= end + 4 + length { break; }
                    }
                }
                let request = String::from_utf8(data).unwrap();
                let action = if request.lines().next().unwrap().contains("Action=UpdateVoiceprint") {
                    assert!(request.contains("\"Action\":0"));
                    "UpdateVoiceprint"
                } else if request.lines().next().unwrap().contains("Action=ListVoiceprint") {
                    "ListVoiceprint"
                } else {
                    assert!(request.lines().next().unwrap().contains("Action=GetVoiceprint"));
                    assert!(request.contains(ID));
                    "GetVoiceprint"
                };
                actions.push(action);
                let result = if action == "UpdateVoiceprint" { json!({"Code":1000,"SpeakID":ID}) }
                    else { json!({"Code":1000,"AudioEmbedding":[{"SpeakID":ID,"SpeakerName":name,"Status":1}]}) };
                let body = json!({"Result":result}).to_string();
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                if action == "UpdateVoiceprint" {
                    // Provider has created the record but its response arrives
                    // after the client's deadline, as in the recorded failure.
                    delayed.push(std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(150));
                        let _ = stream.write_all(response.as_bytes());
                    }));
                } else {
                    stream.write_all(response.as_bytes()).unwrap();
                }
            }
            for worker in delayed { worker.join().unwrap(); }
            actions
        });
        let credentials = VolcengineCredentials {
            app_id:"test-app".into(), access_token:"test-key".into(), resource_id:"test-resource".into(),
        };
        let client = CloudVoiceprintClient {
            credentials:credentials.clone(),
            client:reqwest::Client::builder().no_proxy().timeout(Duration::from_millis(75)).build().unwrap(),
            test_endpoint:Some(endpoint),
        };
        let result = client.register_named("https://fixture.example.test/reference.wav",name).await;
        let actions = server.join().unwrap();
        assert_eq!(actions,vec!["UpdateVoiceprint","ListVoiceprint","GetVoiceprint"]);
        let profile = result.unwrap();
        assert_eq!(profile.id,ID);
        assert_eq!(profile.speaker_name,name);
        assert_eq!(profile.account_fingerprint,account_fingerprint(&credentials.app_id));
    }

    #[test]
    fn uncertain_registration_lookup_requires_one_exact_active_operation() {
        let name = "listener_owner_01234567890123456789012345678901";
        let own = CloudVoiceprintRecord { id: ID.into(), speaker_name: name.into() };
        let other = CloudVoiceprintRecord {
            id: "b3ee18be-6113-4b9a-8bc1-55fbb9f88f71".into(), speaker_name: "other-customer".into(),
        };
        assert_eq!(unique_named_registration(&[other.clone(), own.clone()], name).unwrap(), ID);
        assert!(unique_named_registration(&[], name).is_err());
        assert!(unique_named_registration(&[other], name).is_err());
        assert!(unique_named_registration(&[own.clone(), own], name).is_err());
        let deleted = records(&json!({"AudioEmbedding":[{
            "SpeakID":ID,"SpeakerName":name,"Status":2
        }]})).unwrap();
        assert!(unique_named_registration(&deleted, name).is_err());
        assert!(unique_named_registration(&[CloudVoiceprintRecord {
            id: "invalid".into(), speaker_name: name.into(),
        }], name).is_err());
    }

    #[test]
    fn account_switch_and_anonymous_names_cannot_be_owner() {
        let creds = VolcengineCredentials { app_id:"app-a".into(), access_token:"test".into(), resource_id:"test".into() };
        let mut profile = CloudOwnerVoiceprint { id:ID.into(), speaker_name:"Billy".into(), account_fingerprint:account_fingerprint("app-a") };
        assert!(profile.validate_for(&creds).is_ok());
        profile.account_fingerprint = account_fingerprint("app-b");
        assert!(profile.validate_for(&creds).is_err());
        for name in ["", "0", "-1", "Unknown"] { assert!(validate_record(ID,name).is_err()); }
    }

    #[test]
    fn registry_permission_failure_is_not_an_empty_success() {
        assert!(checked_result(&json!({"Result":{"Code":1002,"AudioEmbedding":[]}})).is_err());
        assert!(checked_result(&json!({"ResponseMetadata":{"Error":{"Code":"AccessDenied"}},"Result":{"Code":1000}})).is_err());
    }

    #[test]
    fn list_excludes_deleted_records_and_does_not_return_audio_or_embeddings() {
        let rows = records(&json!({"AudioEmbedding":[
            {"SpeakId":ID,"SpeakerName":"Billy","Status":1,"AudioUrl":"private","Embedding":[1,2]},
            {"SpeakId":"deleted","SpeakerName":"old","Status":2}
        ]})).unwrap();
        assert_eq!(rows.len(),1);
        assert_eq!(serde_json::to_value(&rows).unwrap(),json!([{"id":ID,"speakerName":"Billy"}]));
        assert!(records(&json!({"UnexpectedShape":[]})).is_err());
    }

    #[test]
    fn real_registry_id_casing_registers_and_selects_without_exposing_embeddings() {
        assert_eq!(voiceprint_id(&json!({"Code":1000,"SpeakID":ID})).unwrap(),ID);
        assert_eq!(voiceprint_id(&json!({"Code":1000,"SpeakId":ID})).unwrap(),ID);
        let rows=records(&json!({"AudioEmbedding":[{
            "SpeakID":ID,"SpeakerName":"listener_owner_01234567890123456789012345678901",
            "Status":1,"AudioURL":"private","Embedding":[1,2],"GroupID":""
        }]})).unwrap();
        assert_eq!(rows[0].id,ID);
        assert_eq!(serde_json::to_value(&rows).unwrap().as_array().unwrap()[0].as_object().unwrap().len(),2);
        assert!(voiceprint_id(&json!({"SpeakID":ID,"SpeakId":"another-customer"})).is_err());
        assert!(voiceprint_id(&json!({"SpeakID":7})).is_err());
    }
}
