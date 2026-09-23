//! 网络健康探测（2026-09-22 用户拍板：网络不佳要有报错灯 + 定位数据）。
//!
//! 背景：ASR 长连 WSS 被网络路径间歇黑洞（~1 次/小时，TUN/Clash 已排除，
//! 真凶在 WiFi/路由/运营商/云端边，app 管不到）。本模块做两件事：
//! ① 给 UX 一个可分类的网络状态（不佳时胶囊亮"网络不佳"而不是装死）；
//! ② 积累定位数据——每次黑洞/断流时探测两个目标（火山 ASR 端点 + 一个
//!   国内对照站），判读行进 decisions.log，几天后就能回答"哪一跳在吞"。
//!
//! 探测用裸 TCP connect（SYN/ACK 即认为可达），不握手 TLS、不耗 ASR 额度。
//! 阻塞实现（std::net），调用方在异步上下文里用 `tokio::task::spawn_blocking`
//! 或独立线程包一层；预算 = 单目标 1.5s。

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 火山 ASR 网关（与 volcengine.rs 的 WSS 端点同主机）。
/// 注意必须是裸主机名——tcp_probe 会自己拼端口。2026-09-23 tkm 事故：
/// 常量带了 ":443"，getaddrinfo 拿到非法 nodename 永远解析失败 → 探测器
/// 永远 all_bad → 键盘灯/胶囊徽章在健康网络上常亮闪了 2 分钟。
const ASR_PROBE_HOST: &str = "openspeech.bytedance.com";
/// 国内对照站：它与 ASR 端点同时坏 → 本机/局域网/运营商问题；
/// 只有 ASR 坏 → 火山侧或到火山的路径问题。
const GENERAL_PROBE_HOST: &str = "www.baidu.com";
const PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkHealthClass {
    /// 两个目标都可达。
    AllGood,
    /// 只有 ASR 端点不可达：火山侧或到火山的路径在吞包。
    AsrUnreachable,
    /// 只有对照站不可达（少见；大概率也是本机网络劣化）。
    GeneralUnreachable,
    /// 两个目标都不可达：本机网络/路由器/运营商掉线。
    AllBad,
}

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
            Self::GeneralUnreachable | Self::AllBad => "网络不佳（请检查网络连接）",
        }
    }

    /// 胶囊徽章短文案（托盘 tooltip 用 user_message 长版；徽章空间小）。
    pub fn short_label(self) -> &'static str {
        match self {
            Self::AllGood => "网络正常",
            Self::AsrUnreachable | Self::GeneralUnreachable => "网络不佳",
            Self::AllBad => "无网络",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NetworkHealthSnapshot {
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

/// 阻塞探测两个目标。两个探测都放子线程、收集等待有上界(~4s/目标)：
/// Windows 的 getaddrinfo 存在罕见的不归路径，DNS 子线程挂死不能把
/// `join()` 一起拖死——2026-09-23 tkk 实锤：灯线程 10:47 首探后再无
/// 任何探测记录（同期 WSS 识别畅通，进程网络明明是好的），按卡死处理。
/// 收集超时的目标按不可达记；挂死的子线程随进程退出，泄漏可接受。
pub fn probe_network_health() -> NetworkHealthSnapshot {
    const PROBE_COLLECT_TIMEOUT: Duration = Duration::from_millis(4_000);
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
        .recv_timeout(PROBE_COLLECT_TIMEOUT)
        .unwrap_or((false, None));
    NetworkHealthSnapshot {
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
        record_network_health_class(snapshot.classify());
    });
}

/// 最近一次探测结果（状态灯数据源）。事件探测与常驻探测都写这里。
static LATEST_CLASS: Mutex<Option<NetworkHealthClass>> = Mutex::new(None);
/// 灯线程的消费标记：任何写入方(事件探测/常驻探测)分级变化时置位，
/// 灯线程 2s 轮询取走并刷新托盘——黑洞突发只持续几秒,60s 巡检一圈回来
/// 网络早恢复了,靠巡检永远看不到红灯;事件探测必须即时点亮。
static CHANGE_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 记录最新分级；返回它是否相对上一份发生了变化（灯只在变化时才动/记日志）。
pub fn record_network_health_class(class: NetworkHealthClass) -> bool {
    let mut latest = LATEST_CLASS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let changed = *latest != Some(class);
    if changed {
        *latest = Some(class);
        CHANGE_PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    changed
}

/// 灯线程用：是否有未消费的分级变化（取走即清）。
pub fn take_network_health_change_pending() -> bool {
    CHANGE_PENDING.swap(false, std::sync::atomic::Ordering::SeqCst)
}

/// 状态灯/会话起点预判用：最近一次探测的分级；None=还没测过。
pub fn latest_network_health_class() -> Option<NetworkHealthClass> {
    *LATEST_CLASS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_health_classification_covers_all_four_quadrants() {
        let good = NetworkHealthSnapshot {
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
            asr_ok: false,
            asr_latency_ms: None,
            general_ok: false,
            general_latency_ms: None,
        };
        assert_eq!(all_bad.classify(), NetworkHealthClass::AllBad);
        assert_eq!(
            all_bad.classify().user_message(),
            "网络不佳（请检查网络连接）"
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
