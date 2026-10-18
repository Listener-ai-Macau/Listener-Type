//! Reachability diagnostics and current recognition connectivity share one ordered state.
//! A real provider response clears stale ASR failures immediately. TCP control-site
//! failures remain diagnostic evidence; two unreachable hosts do not prove all Internet is down.
//! Probes use no credentials or ASR quota and never block provider/UI recovery.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 火山 ASR 网关（与 volcengine.rs 的 WSS 端点同主机）。
/// 注意必须是裸主机名——tcp_probe 会自己拼端口。2026-09-23 tkm 事故：
/// 常量带了 ":443"，getaddrinfo 拿到非法 nodename 永远解析失败 → 探测器
/// 永远 all_bad → 键盘灯/胶囊徽章在健康网络上常亮闪了 2 分钟。
const ASR_PROBE_HOST: &str = "openspeech.bytedance.com";
/// Independent control endpoint; its failure alone says nothing about a working ASR stream.
const GENERAL_PROBE_HOST: &str = "www.baidu.com";
const PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);

pub use crate::network_health_state::NetworkHealthClass;
use crate::network_health_state::NetworkHealthState;
use std::sync::OnceLock;

impl NetworkHealthClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::AllGood => "all_good",
            Self::AsrUnreachable => "asr_unreachable",
            Self::GeneralUnreachable => "general_unreachable",
            Self::AllBad => "all_bad",
        }
    }

    /// 面向用户的胶囊文案（与后端既有中文消息同一风格）。
    pub fn user_message(self) -> &'static str {
        match self {
            Self::AllGood => "网络正常",
            Self::AsrUnreachable => "网络不佳（识别服务不可达）",
            Self::GeneralUnreachable => "识别连接正常（网络检测站不可达）",
            Self::AllBad => "网络连接异常（请检查网络连接）",
        }
    }

    /// 胶囊徽章短文案（托盘 tooltip 用 user_message 长版；徽章空间小）。
    pub fn short_label(self) -> &'static str {
        match self {
            Self::AllGood => "",
            Self::AsrUnreachable => "识别连接异常",
            Self::GeneralUnreachable => "", // A failed control site does not invalidate working ASR.
            Self::AllBad => "网络连接异常",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NetworkHealthSnapshot {
    observed_at: Instant,
    pub asr_ok: bool,
    pub asr_latency_ms: Option<u128>,
    pub general_ok: bool,
    pub general_latency_ms: Option<u128>,
}

impl NetworkHealthSnapshot {
    pub fn classify(&self) -> NetworkHealthClass {
        match (self.asr_ok, self.general_ok) {
            (true, true) => NetworkHealthClass::AllGood,
            (false, true) => NetworkHealthClass::AsrUnreachable,
            (true, false) => NetworkHealthClass::GeneralUnreachable,
            (false, false) => NetworkHealthClass::AllBad,
        }
    }

    /// 判读行（进 decisions.log，与黑洞/断流事件做时间相关）。
    pub fn log_line(&self) -> String {
        format!(
            "[net-health] class={} asr={} asr_latency_ms={} general={} general_latency_ms={}",
            self.classify().label(),
            self.asr_ok,
            self.asr_latency_ms.unwrap_or(0),
            self.general_ok,
            self.general_latency_ms.unwrap_or(0),
        )
    }
}

fn tcp_probe(host: &str) -> (bool, Option<u128>) {
    let started = Instant::now();
    let resolved = (host, 443)
        .to_socket_addrs()
        .map(|addrs| addrs.collect::<Vec<_>>())
        .ok();
    let Some(addresses) = resolved else {
        // DNS 失败也算不可达（本机断网时最常见形态）。
        return (false, None);
    };
    if addresses.is_empty() {
        return (false, None);
    }
    let mut last_err = None;
    for address in &addresses {
        match TcpStream::connect_timeout(address, PROBE_TIMEOUT) {
            Ok(_) => return (true, Some(started.elapsed().as_millis())),
            Err(error) => last_err = Some(error),
        }
    }
    log::debug!("[net-health] tcp probe failed for {host}: {last_err:?}");
    (false, None)
}

/// Blocking probes run in parallel with a shared four-second collection deadline:
/// Windows 的 getaddrinfo 存在罕见的不归路径，DNS 子线程挂死不能把
/// `join()` 一起拖死——2026-09-23 tkk 实锤：灯线程 10:47 首探后再无
/// 任何探测记录（同期 WSS 识别畅通，进程网络明明是好的），按卡死处理。
/// 收集超时的目标按不可达记；挂死的子线程随进程退出，泄漏可接受。
pub fn probe_network_health() -> NetworkHealthSnapshot {
    const PROBE_COLLECT_TIMEOUT: Duration = Duration::from_millis(4_000);
    let observed_at = Instant::now();
    let (asr_tx, asr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = asr_tx.send(tcp_probe(ASR_PROBE_HOST));
    });
    let (general_tx, general_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = general_tx.send(tcp_probe(GENERAL_PROBE_HOST));
    });
    let (asr_ok, asr_latency_ms) = asr_rx
        .recv_timeout(PROBE_COLLECT_TIMEOUT)
        .unwrap_or((false, None));
    let (general_ok, general_latency_ms) = general_rx
        .recv_timeout(PROBE_COLLECT_TIMEOUT.saturating_sub(observed_at.elapsed()))
        .unwrap_or((false, None));
    NetworkHealthSnapshot {
        observed_at,
        asr_ok,
        asr_latency_ms,
        general_ok,
        general_latency_ms,
    }
}

/// 火山侧主动记录：在黑洞掐线/云端错误帧时发射一次探测（fire-and-forget，
/// 不阻塞会话路径），判读行进日志。
pub fn log_network_health_probe_async(reason: &'static str) {
    std::thread::spawn(move || {
        let snapshot = probe_network_health();
        log::warn!("{} reason={reason}", snapshot.log_line());
        record_network_health_probe(snapshot);
    });
}

/// One owner for probe and real-provider observations; callbacks run outside its lock.
static HEALTH: Mutex<NetworkHealthState> = Mutex::new(NetworkHealthState::new());
type ChangeListener = Box<dyn Fn(NetworkHealthPayload) + Send + Sync>;
static CHANGE_LISTENER: OnceLock<ChangeListener> = OnceLock::new();

#[derive(Clone, Debug, serde::Serialize)]
pub struct NetworkHealthPayload {
    pub seq: u64,
    pub class: &'static str,
    pub badge: &'static str,
    pub message: &'static str,
    #[serde(rename = "providerQuotaExceeded")]
    pub provider_quota_exceeded: bool,
}

fn payload(state: &NetworkHealthState) -> Option<NetworkHealthPayload> {
    let class = state.class()?;
    Some(NetworkHealthPayload {
        seq: state.revision, class: class.label(), badge: class.short_label(),
        message: class.user_message(),
        provider_quota_exceeded: state.quota_exceeded(),
    })
}

pub fn latest_network_health_payload() -> Option<NetworkHealthPayload> {
    payload(&HEALTH.lock().unwrap_or_else(|p| p.into_inner()))
}

pub fn set_change_listener(listener: impl Fn(NetworkHealthPayload) + Send + Sync + 'static) {
    if CHANGE_LISTENER.set(Box::new(listener)).is_ok() {
        if let (Some(listener), Some(view)) = (CHANGE_LISTENER.get(), latest_network_health_payload()) {
            listener(view);
        }
    }
}

fn record_observation(at: Instant, asr: Option<bool>, general: Option<bool>, reason: &str) {
    let changed = {
        let mut state = HEALTH.lock().unwrap_or_else(|p| p.into_inner());
        if state.observe(at, asr, general) { payload(&state) } else { None }
    };
    if let Some(view) = changed {
        log::info!("[net-health] publish seq={} class={} reason={reason}", view.seq, view.class);
        if let Some(listener) = CHANGE_LISTENER.get() { listener(view); }
    }
}

pub fn record_network_health_probe(snapshot: NetworkHealthSnapshot) {
    record_observation(snapshot.observed_at, Some(snapshot.asr_ok), Some(snapshot.general_ok), "probe");
}

/// A decoded provider response proves that the recognition path is reachable.
/// It does not claim the independent control site recovered or alter ASR results.
pub fn record_asr_response() {
    record_observation(Instant::now(), Some(true), None, "provider_response");
}

/// A provider rejection is shown immediately while capture/audio retention
/// continues. It is separate from reachability and survives successful TCP probes.
pub fn record_asr_quota_exceeded() { record_asr_quota(true); }

pub fn record_asr_recognition_response() {
    record_asr_response();
    record_asr_quota(false);
}

fn record_asr_quota(exceeded: bool) {
    let changed = {
        let mut state = HEALTH.lock().unwrap_or_else(|p| p.into_inner());
        let at = Instant::now();
        let connectivity_changed = state.observe(at, Some(true), None);
        let quota_changed = state.observe_quota(at, exceeded);
        if connectivity_changed || quota_changed { payload(&state) } else { None }
    };
    if let Some(view) = changed {
        log::info!("[asr-availability] publish seq={} quota_exceeded={exceeded}", view.seq);
        if let Some(listener) = CHANGE_LISTENER.get() { listener(view); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn successful_provider_publishes_immediately_and_stale_probe_cannot_restore_warning() {
        let captured = std::sync::Arc::new(Mutex::new(Vec::new()));
        let events = captured.clone();
        set_change_listener(move |view| events.lock().unwrap().push(view));
        let old = Instant::now();
        record_observation(old, Some(false), Some(false), "test_outage");
        record_asr_response();
        let recovered = latest_network_health_payload().unwrap();
        assert_eq!(recovered.class, "general_unreachable");
        assert_eq!(recovered.badge, "");
        assert_eq!(captured.lock().unwrap().last().unwrap().seq, recovered.seq);
        record_observation(old, Some(false), Some(false), "late_probe");
        assert_eq!(latest_network_health_payload().unwrap().seq, recovered.seq);
        let bad = Instant::now();
        record_observation(bad, Some(false), Some(false), "new_outage");
        assert_eq!(latest_network_health_payload().unwrap().badge, "网络连接异常");
    }
    #[test]
    fn network_health_classification_covers_all_four_quadrants() {
        let good = NetworkHealthSnapshot {
            observed_at: Instant::now(),
            asr_ok: true,
            asr_latency_ms: Some(30),
            general_ok: true,
            general_latency_ms: Some(20),
        };
        assert_eq!(good.classify(), NetworkHealthClass::AllGood);
        assert_eq!(good.classify().label(), "all_good");

        let asr_only = NetworkHealthSnapshot {
            asr_ok: false,
            asr_latency_ms: None,
            ..good
        };
        assert_eq!(asr_only.classify(), NetworkHealthClass::AsrUnreachable);

        let all_bad = NetworkHealthSnapshot {
            observed_at: Instant::now(),
            asr_ok: false,
            asr_latency_ms: None,
            general_ok: false,
            general_latency_ms: None,
        };
        assert_eq!(all_bad.classify(), NetworkHealthClass::AllBad);
        assert_eq!(
            all_bad.classify().user_message(),
            "网络连接异常（请检查网络连接）"
        );

        let general_only = NetworkHealthSnapshot {
            general_ok: false,
            general_latency_ms: None,
            ..good
        };
        assert_eq!(
            general_only.classify(),
            NetworkHealthClass::GeneralUnreachable
        );
    }

    #[test]
    fn probe_hosts_are_bare_hostnames() {
        // tkm 事故回归闸：常量带 ":443" 会让 getaddrinfo 永远解析失败，
        // 探测器永远 all_bad → 键盘灯在健康网络上常亮。tcp_probe 自己拼
        // 端口，常量必须是裸主机名。
        for host in [ASR_PROBE_HOST, GENERAL_PROBE_HOST] {
            assert!(
                !host.contains(':'),
                "probe host must be bare hostname, got {host}"
            );
        }
    }

    #[test]
    fn log_line_carries_classification_for_the_watcher() {
        let snapshot = NetworkHealthSnapshot {
            observed_at: Instant::now(),
            asr_ok: false,
            asr_latency_ms: None,
            general_ok: true,
            general_latency_ms: Some(41),
        };
        let line = snapshot.log_line();
        assert!(line.contains("class=asr_unreachable"));
        assert!(line.contains("general_latency_ms=41"));
    }
}
