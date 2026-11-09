//! UI-only locale for native menus. Never infer it from recognition/output preferences.
use std::sync::{Mutex, OnceLock};
use crate::types::PolishMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiLocale { ZhCn, ZhTw, En, Ja, Ko }

impl UiLocale {
    pub(crate) fn parse(tag: &str) -> Option<Self> {
        match tag {
            "zh-CN" => Some(Self::ZhCn), "zh-TW" => Some(Self::ZhTw),
            "en" => Some(Self::En), "ja" => Some(Self::Ja), "ko" => Some(Self::Ko), _ => None,
        }
    }
    pub(crate) fn text(self, labels: [&'static str; 5]) -> &'static str {
        labels[match self { Self::ZhCn => 0, Self::ZhTw => 1, Self::En => 2, Self::Ja => 3, Self::Ko => 4 }]
    }
    pub(crate) fn style(self, mode: PolishMode) -> &'static str {
        match mode {
            PolishMode::Raw => self.text(["原文", "原文", "Raw", "原文", "원문"]),
            PolishMode::Light => self.text(["轻度润色", "輕度潤色", "Light polish", "軽い整文", "가벼운 정리"]),
            PolishMode::Structured => self.text(["技术指令", "技術指令", "Technical instructions", "技術的な指示", "기술 지시"]),
            PolishMode::Formal => self.text(["正式表达", "正式表達", "Formal", "正式な表現", "정식 표현"]),
        }
    }
    pub(crate) fn network_status(self, class: &str) -> &'static str {
        match class {
            "all_good" => self.text(["网络正常", "網路正常", "Network connected", "ネットワーク接続は正常です", "네트워크 연결 정상"]),
            "asr_unreachable" => self.text(["识别服务暂不可达", "辨識服務暫時無法連線", "Speech service unavailable", "音声認識サービスに接続できません", "음성 인식 서비스에 연결할 수 없습니다"]),
            "general_unreachable" => self.text(["识别连接正常（检测站不可达）", "辨識連線正常（檢測站無法連線）", "Speech connected (network check unavailable)", "音声認識は接続済み（ネットワーク確認先に接続できません）", "음성 인식 연결 정상(네트워크 확인 서버에 연결할 수 없음)"]),
            _ => self.text(["请检查网络连接", "請檢查網路連線", "Check the network connection", "ネットワーク接続を確認してください", "네트워크 연결을 확인하세요"]),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Preference { preference: String, resolved: String }
static CURRENT: OnceLock<Mutex<UiLocale>> = OnceLock::new();

fn system_locale() -> Option<UiLocale> {
    #[cfg(target_os = "windows")]
    {
        let mut name = [0u16; 85];
        let length = unsafe {
            windows::Win32::Globalization::LCIDToLocaleName(
                u32::from(windows::Win32::Globalization::GetUserDefaultUILanguage()), Some(&mut name), 0)
        };
        if length > 1 { return Some(from_system_tag(&String::from_utf16_lossy(&name[..length as usize - 1]))); }
    }
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(tag) = std::env::var(key) { if !tag.is_empty() { return Some(from_system_tag(&tag)); } }
    }
    None
}

fn from_system_tag(tag: &str) -> UiLocale {
    let tag = tag.to_lowercase().replace('_', "-");
    if tag.starts_with("zh") {
        if ["hant", "-tw", "-hk", "-mo"].iter().any(|part| tag.contains(part)) { UiLocale::ZhTw } else { UiLocale::ZhCn }
    } else if tag.starts_with("ja") { UiLocale::Ja }
    else if tag.starts_with("ko") { UiLocale::Ko }
    else { UiLocale::En }
}

fn read_locale(path: &std::path::Path, system: Option<UiLocale>) -> UiLocale {
    let saved = std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<Preference>(&bytes).ok());
    match saved {
        Some(saved) if saved.preference == "system" => system.or_else(|| UiLocale::parse(&saved.resolved)).unwrap_or(UiLocale::En),
        Some(saved) => UiLocale::parse(&saved.preference).unwrap_or_else(|| system.unwrap_or(UiLocale::En)),
        None => system.unwrap_or(UiLocale::En),
    }
}

pub(crate) fn current() -> UiLocale {
    *CURRENT.get_or_init(|| Mutex::new(crate::persistence::ui_locale_path()
        .map(|path| read_locale(&path, system_locale())).unwrap_or_else(|_| system_locale().unwrap_or(UiLocale::En))))
        .lock().unwrap_or_else(|error| error.into_inner())
}

pub(crate) fn set(preference: &str, resolved: &str) -> Result<bool, String> {
    let locale = UiLocale::parse(resolved).ok_or("unsupported UI locale")?;
    if preference != "system" && UiLocale::parse(preference) != Some(locale) { return Err("invalid UI locale preference".into()); }
    let _ = current();
    let mut current = CURRENT.get().unwrap().lock().unwrap_or_else(|error| error.into_inner());
    let previous = *current;
    let saved = serde_json::to_vec(&Preference { preference: preference.into(), resolved: resolved.into() }).map_err(|error| error.to_string())?;
    let path = crate::persistence::ui_locale_path().map_err(|error| error.to_string())?;
    if std::fs::read(&path).ok().as_deref() != Some(saved.as_slice()) {
        crate::persistence::write_ui_locale(&path, &saved).map_err(|error| error.to_string())?;
    }
    *current = locale;
    Ok(previous != locale)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_ui_language_and_system_following_are_independent() {
        let directory = std::env::temp_dir().join(format!("listener-ui-locale-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap(); let path = directory.join("ui-locale.json");
        assert_eq!(read_locale(&path, Some(UiLocale::Ja)), UiLocale::Ja);
        std::fs::write(&path, br#"{"preference":"en","resolved":"en"}"#).unwrap();
        assert_eq!(read_locale(&path, Some(UiLocale::ZhCn)), UiLocale::En);
        std::fs::write(&path, br#"{"preference":"system","resolved":"en"}"#).unwrap();
        assert_eq!(read_locale(&path, Some(UiLocale::Ko)), UiLocale::Ko);
        assert_eq!(read_locale(&path, None), UiLocale::En);
        std::fs::write(&path, b"invalid").unwrap();
        assert_eq!(read_locale(&path, Some(UiLocale::ZhTw)), UiLocale::ZhTw);
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn system_tags_and_all_five_style_labels_are_localized() {
        assert_eq!(from_system_tag("zh-Hant-HK"), UiLocale::ZhTw);
        assert_eq!(from_system_tag("zh_CN.UTF-8"), UiLocale::ZhCn);
        assert_eq!(from_system_tag("ja-JP"), UiLocale::Ja);
        assert_eq!(from_system_tag("ko-KR"), UiLocale::Ko);
        for locale in [UiLocale::ZhCn, UiLocale::ZhTw, UiLocale::En, UiLocale::Ja, UiLocale::Ko] {
            for mode in [PolishMode::Raw, PolishMode::Light, PolishMode::Structured, PolishMode::Formal] { assert!(!locale.style(mode).is_empty()); }
            for status in ["all_good", "asr_unreachable", "general_unreachable", "all_bad"] { assert!(!locale.network_status(status).is_empty()); }
        }
    }
}
