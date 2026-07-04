//! 极简 CLI 参数解析 — 用于支持桌面环境快捷键调起 Listener Type 触发听写 / QA。
//!
//! 这条路径的来历：Linux Wayland 协议层面禁止"应用监听全局键盘"（除了焦点窗口），
//! 因此 rdev 在 Wayland 上必然失效（issue #420）。本仓库不为 Wayland 引入门户
//! GlobalShortcuts（GNOME 尚未原生落地，引入会增加合成器分裂的维护负担——见
//! `docs/issue-420-wayland-hotkey-research.md` 3.1 节），改走桌面环境快捷键 →
//! `listener-type --toggle-dictation` → tauri-plugin-single-instance 转发的 CLI 路径。
//! macOS / Windows 上仍走原生 hotkey 监听器，CLI 是补充而非替代。
//!
//! 解析约束：
//! - **不依赖 clap**。CLI surface 仍是少量 flag、无子命令，引入 clap 既增加二进制体积
//!   也带来「未知参数即 panic exit」的风险——GUI app 必须吃下未知参数照常起来，否则
//!   .desktop launcher 或发行版包装传 dragged-in 文件路径就直接崩。
//! - **未知参数静默忽略**。第一个能识别的 flag 即返回；其他参数（路径 / 自动注入的
//!   launcher 标志）不报错。
//! - **同一份解析复用**首次启动 + single-instance 回调两个入口，行为完全一致。

use std::path::PathBuf;

/// 桌面环境快捷键能给 Listener Type 触发的动作集合。
///
/// 与 modifier-only / combo 热键对齐 — 只覆盖「单次触发」语义，不含 push-to-talk
/// （桌面 OS 级快捷键大多只在 key-press 触发，不传 key-release，无法支持「按住说话」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliIntent {
    /// 等价于按一次主听写热键：Idle → 开始；Listening → 结束。
    ToggleDictation,
    /// 等价于按一次 QA 热键：toggle QA 浮窗显隐。
    ToggleQa,
    /// 等价于按 Esc：取消当前听写 session。
    CancelDictation,
    /// 调试 / 自动化入口：把本地 16k mono i16 WAV/PCM 当成嵌入式音频送入听写链路。
    SubmitEmbeddedAudioFile {
        path: PathBuf,
        format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
    },
    /// 调试 / 自动化入口：把本地 16k mono i16 WAV/PCM 拆成 VKA1 事件流送入听写链路。
    SubmitEmbeddedAudioStreamingFile {
        path: PathBuf,
        format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
    },
    /// 调试 / 自动化入口：订阅一次嵌入式 BLE notify，会话结束后送入听写链路。
    SubmitEmbeddedAudioBleOnce { timeout_ms: Option<u64> },
    /// 调试 / 自动化入口：订阅嵌入式 BLE notify，收到 audio_data 立刻送入 ASR。
    SubmitEmbeddedAudioBleStream { timeout_ms: Option<u64> },
    /// 调试 / 自动化入口：触发前台 BLE 状态探测，验证它能复用或等待后台 listener。
    ProbeEmbeddedAudioBleSubscription { timeout_ms: Option<u64> },
    /// 调试 / 自动化入口：只发一次嵌入式 BLE 录音停止控制，用于清理失败的硬件测试会话。
    SendEmbeddedAudioControlStop { timeout_ms: Option<u64> },
    /// 调试 / 自动化入口：读取嵌入式 BLE 音频服务的 readiness/capabilities 状态。
    ReadEmbeddedAudioBleStatus { timeout_ms: Option<u64> },
    /// 调试 / 自动化入口：只验证 Listener OTA v2 GATT 服务可达，不做版本升级判定。
    ProbeListenerOtaV2Gatt { timeout_ms: Option<u64> },
    /// 调试 / 自动化入口：扫描未配对 Listener 并触发 Windows 系统配对体验。
    PromptEmbeddedBlePairing { expected_name: Option<String> },
    /// 调试 / 自动化入口：只执行 Windows 配对体验，不先通过串口打开 recovery 窗口。
    PromptEmbeddedBlePairingOnly { expected_name: Option<String> },
    /// 调试 / 自动化入口：清理 Windows 里残留的 Listener 配对和 PnP 缓存。
    CleanupEmbeddedBlePairing { expected_name: Option<String> },
    /// 调试 / 自动化入口：校验固件 OTA 包，可选做 BLE preflight 或真实传输。
    FirmwareOta {
        manifest_path: PathBuf,
        firmware_path: PathBuf,
        preflight_only: bool,
        transfer: bool,
    },
    /// 调试 / 自动化入口：用 Type 后端的有线刷机实现校验或刷入 factory 固件包。
    WiredFirmware {
        package_path: PathBuf,
        port: Option<String>,
        baud: Option<u32>,
        action: WiredFirmwareCliAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiredFirmwareCliAction {
    Check,
    Flash,
    BootRepair,
}

/// 扫描 argv 找第一个能识别的 intent。未知参数静默忽略，绝不 panic。
///
/// `args` 通常是 `std::env::args().collect::<Vec<_>>()` 或 single-instance 回调里
/// 传入的 `Vec<String>`；两条路径走同一份解析。
pub fn parse_cli_intent<S: AsRef<str>>(args: &[S]) -> Option<CliIntent> {
    // 跳过 argv[0]（自身路径），逐项匹配。命中第一个就返回 —
    // 多个 flag 时取首个，避免出现"toggle + cancel"这种自相矛盾组合。
    let mut args = args.iter().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_ref() {
            "--toggle-dictation" => return Some(CliIntent::ToggleDictation),
            "--toggle-qa" => return Some(CliIntent::ToggleQa),
            "--cancel-dictation" | "--cancel" => return Some(CliIntent::CancelDictation),
            "--submit-embedded-audio" => {
                if let Some(path) = next_path_arg(&mut args) {
                    return Some(CliIntent::SubmitEmbeddedAudioFile { path, format: None });
                }
            }
            "--submit-embedded-audio-wav" => {
                if let Some(path) = next_path_arg(&mut args) {
                    return Some(CliIntent::SubmitEmbeddedAudioFile {
                        path,
                        format: Some(crate::embedded_audio::EmbeddedAudioInputFormat::Wav),
                    });
                }
            }
            "--submit-embedded-audio-pcm16le" => {
                if let Some(path) = next_path_arg(&mut args) {
                    return Some(CliIntent::SubmitEmbeddedAudioFile {
                        path,
                        format: Some(crate::embedded_audio::EmbeddedAudioInputFormat::Pcm16Le),
                    });
                }
            }
            "--submit-embedded-audio-stream" => {
                if let Some(path) = next_path_arg(&mut args) {
                    return Some(CliIntent::SubmitEmbeddedAudioStreamingFile {
                        path,
                        format: None,
                    });
                }
            }
            "--submit-embedded-audio-wav-stream" => {
                if let Some(path) = next_path_arg(&mut args) {
                    return Some(CliIntent::SubmitEmbeddedAudioStreamingFile {
                        path,
                        format: Some(crate::embedded_audio::EmbeddedAudioInputFormat::Wav),
                    });
                }
            }
            "--submit-embedded-audio-pcm16le-stream" => {
                if let Some(path) = next_path_arg(&mut args) {
                    return Some(CliIntent::SubmitEmbeddedAudioStreamingFile {
                        path,
                        format: Some(crate::embedded_audio::EmbeddedAudioInputFormat::Pcm16Le),
                    });
                }
            }
            "--submit-embedded-audio-ble-once" => {
                return Some(CliIntent::SubmitEmbeddedAudioBleOnce {
                    timeout_ms: next_u64_arg(&mut args),
                });
            }
            "--submit-embedded-audio-ble-stream" => {
                return Some(CliIntent::SubmitEmbeddedAudioBleStream {
                    timeout_ms: next_u64_arg(&mut args),
                });
            }
            "--probe-embedded-audio-ble-subscription" => {
                return Some(CliIntent::ProbeEmbeddedAudioBleSubscription {
                    timeout_ms: next_u64_arg(&mut args),
                });
            }
            "--send-embedded-audio-control-stop" => {
                return Some(CliIntent::SendEmbeddedAudioControlStop {
                    timeout_ms: next_u64_arg(&mut args),
                });
            }
            "--read-embedded-audio-ble-status" => {
                return Some(CliIntent::ReadEmbeddedAudioBleStatus {
                    timeout_ms: next_u64_arg(&mut args),
                });
            }
            "--probe-listener-ota-v2-gatt" => {
                return Some(CliIntent::ProbeListenerOtaV2Gatt {
                    timeout_ms: next_u64_arg(&mut args),
                });
            }
            "--prompt-embedded-ble-pairing" => {
                let expected_name = args
                    .peek()
                    .map(|value| value.as_ref())
                    .filter(|value| !value.starts_with("--"))
                    .map(ToOwned::to_owned);
                if expected_name.is_some() {
                    let _ = args.next();
                }
                return Some(CliIntent::PromptEmbeddedBlePairing { expected_name });
            }
            "--prompt-embedded-ble-pairing-only" => {
                let expected_name = args
                    .peek()
                    .map(|value| value.as_ref())
                    .filter(|value| !value.starts_with("--"))
                    .map(ToOwned::to_owned);
                if expected_name.is_some() {
                    let _ = args.next();
                }
                return Some(CliIntent::PromptEmbeddedBlePairingOnly { expected_name });
            }
            "--cleanup-embedded-ble-pairing" => {
                let expected_name = args
                    .peek()
                    .map(|value| value.as_ref())
                    .filter(|value| !value.starts_with("--"))
                    .map(ToOwned::to_owned);
                if expected_name.is_some() {
                    let _ = args.next();
                }
                return Some(CliIntent::CleanupEmbeddedBlePairing { expected_name });
            }
            "--firmware-ota-check" | "--firmware-ota-preflight" | "--firmware-ota-transfer" => {
                let mode = arg.as_ref();
                if let Some((manifest_path, firmware_path)) = next_ota_paths(&mut args) {
                    return Some(CliIntent::FirmwareOta {
                        manifest_path,
                        firmware_path,
                        preflight_only: mode == "--firmware-ota-preflight",
                        transfer: mode == "--firmware-ota-transfer",
                    });
                }
            }
            "--wired-firmware-check"
            | "--wired-firmware-flash"
            | "--wired-firmware-boot-repair"
            | "--wired-firmware-repair-bootloader" => {
                let action = match arg.as_ref() {
                    "--wired-firmware-check" => WiredFirmwareCliAction::Check,
                    "--wired-firmware-flash" => WiredFirmwareCliAction::Flash,
                    _ => WiredFirmwareCliAction::BootRepair,
                };
                if let Some((package_path, port, baud)) = next_wired_firmware_args(&mut args) {
                    return Some(CliIntent::WiredFirmware {
                        package_path,
                        port,
                        baud,
                        action,
                    });
                }
            }
            _ => {}
        }
    }
    None
}

fn next_path_arg<'a, S, I>(args: &mut std::iter::Peekable<I>) -> Option<PathBuf>
where
    S: AsRef<str> + 'a,
    I: Iterator<Item = &'a S>,
{
    let next: &str = (*args.peek()?).as_ref();
    if next.starts_with("--") {
        return None;
    }
    args.next().map(|value| PathBuf::from(value.as_ref()))
}

fn next_u64_arg<'a, S, I>(args: &mut std::iter::Peekable<I>) -> Option<u64>
where
    S: AsRef<str> + 'a,
    I: Iterator<Item = &'a S>,
{
    let next: &str = (*args.peek()?).as_ref();
    if next.starts_with("--") {
        return None;
    }
    let parsed = next.parse().ok()?;
    let _ = args.next();
    Some(parsed)
}

fn next_ota_paths<'a, S, I>(args: &mut std::iter::Peekable<I>) -> Option<(PathBuf, PathBuf)>
where
    S: AsRef<str> + 'a,
    I: Iterator<Item = &'a S>,
{
    let manifest_path = next_path_arg(args)?;
    let firmware_path = next_path_arg(args)?;
    Some((manifest_path, firmware_path))
}

fn next_wired_firmware_args<'a, S, I>(
    args: &mut std::iter::Peekable<I>,
) -> Option<(PathBuf, Option<String>, Option<u32>)>
where
    S: AsRef<str> + 'a,
    I: Iterator<Item = &'a S>,
{
    let package_path = next_path_arg(args)?;
    let Some(next) = args.peek() else {
        return Some((package_path, None, None));
    };
    let next = next.as_ref();
    if next.starts_with("--") {
        return Some((package_path, None, None));
    }
    let first = args.next()?.as_ref().to_string();
    if let Ok(baud) = first.parse::<u32>() {
        return Some((package_path, None, Some(baud)));
    }

    let port = Some(first);
    let baud = args.peek().and_then(|candidate| {
        let value = candidate.as_ref();
        if value.starts_with("--") {
            return None;
        }
        value.parse::<u32>().ok()
    });
    if baud.is_some() {
        let _ = args.next();
    }
    Some((package_path, port, baud))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_none_for_empty_argv() {
        let args: Vec<&str> = vec![];
        assert_eq!(parse_cli_intent(&args), None);
    }

    #[test]
    fn parse_returns_none_when_only_argv0() {
        // GUI 双击启动 / Tauri 默认启动场景：只有 argv[0]，没有 intent。
        let args = vec!["listener-type"];
        assert_eq!(parse_cli_intent(&args), None);
    }

    #[test]
    fn parse_recognizes_toggle_dictation() {
        let args = vec!["listener-type", "--toggle-dictation"];
        assert_eq!(parse_cli_intent(&args), Some(CliIntent::ToggleDictation));
    }

    #[test]
    fn parse_recognizes_toggle_qa() {
        let args = vec!["listener-type", "--toggle-qa"];
        assert_eq!(parse_cli_intent(&args), Some(CliIntent::ToggleQa));
    }

    #[test]
    fn parse_recognizes_cancel_dictation() {
        let args = vec!["listener-type", "--cancel-dictation"];
        assert_eq!(parse_cli_intent(&args), Some(CliIntent::CancelDictation));
    }

    #[test]
    fn parse_recognizes_embedded_audio_file_with_inferred_format() {
        let args = vec!["listener-type", "--submit-embedded-audio", "input.wav"];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::SubmitEmbeddedAudioFile {
                path: PathBuf::from("input.wav"),
                format: None,
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_audio_wav_file() {
        let args = vec!["listener-type", "--submit-embedded-audio-wav", "input.raw"];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::SubmitEmbeddedAudioFile {
                path: PathBuf::from("input.raw"),
                format: Some(crate::embedded_audio::EmbeddedAudioInputFormat::Wav),
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_audio_pcm_file() {
        let args = vec![
            "listener-type",
            "--submit-embedded-audio-pcm16le",
            "input.pcm",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::SubmitEmbeddedAudioFile {
                path: PathBuf::from("input.pcm"),
                format: Some(crate::embedded_audio::EmbeddedAudioInputFormat::Pcm16Le),
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_audio_streaming_file() {
        let args = vec![
            "listener-type",
            "--submit-embedded-audio-stream",
            "input.wav",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::SubmitEmbeddedAudioStreamingFile {
                path: PathBuf::from("input.wav"),
                format: None,
            })
        );
    }

    #[test]
    fn parse_ignores_embedded_audio_flag_without_path() {
        let args = vec!["listener-type", "--submit-embedded-audio"];
        assert_eq!(parse_cli_intent(&args), None);
    }

    #[test]
    fn parse_recognizes_embedded_ble_once_with_timeout() {
        let args = vec!["listener-type", "--submit-embedded-audio-ble-once", "90000"];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::SubmitEmbeddedAudioBleOnce {
                timeout_ms: Some(90_000),
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_ble_once_without_timeout() {
        let args = vec!["listener-type", "--submit-embedded-audio-ble-once"];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::SubmitEmbeddedAudioBleOnce { timeout_ms: None })
        );
    }

    #[test]
    fn parse_recognizes_embedded_ble_stream() {
        let args = vec![
            "listener-type",
            "--submit-embedded-audio-ble-stream",
            "90000",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::SubmitEmbeddedAudioBleStream {
                timeout_ms: Some(90_000),
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_ble_probe_with_timeout() {
        let args = vec![
            "listener-type",
            "--probe-embedded-audio-ble-subscription",
            "15000",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::ProbeEmbeddedAudioBleSubscription {
                timeout_ms: Some(15_000),
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_ble_probe_without_timeout() {
        let args = vec!["listener-type", "--probe-embedded-audio-ble-subscription"];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::ProbeEmbeddedAudioBleSubscription { timeout_ms: None })
        );
    }

    #[test]
    fn parse_recognizes_embedded_ble_control_stop_with_timeout() {
        let args = vec![
            "listener-type",
            "--send-embedded-audio-control-stop",
            "4000",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::SendEmbeddedAudioControlStop {
                timeout_ms: Some(4000),
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_ble_status_with_timeout() {
        let args = vec!["listener-type", "--read-embedded-audio-ble-status", "7000"];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::ReadEmbeddedAudioBleStatus {
                timeout_ms: Some(7000),
            })
        );
    }

    #[test]
    fn parse_recognizes_listener_ota_v2_gatt_probe_with_timeout() {
        let args = vec!["listener-type", "--probe-listener-ota-v2-gatt", "20000"];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::ProbeListenerOtaV2Gatt {
                timeout_ms: Some(20000),
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_ble_pairing_prompt_with_name() {
        let args = vec![
            "listener-type",
            "--prompt-embedded-ble-pairing",
            "listenerB",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::PromptEmbeddedBlePairing {
                expected_name: Some("listenerB".to_string()),
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_ble_pairing_only_prompt_with_name() {
        let args = vec![
            "listener-type",
            "--prompt-embedded-ble-pairing-only",
            "listenerB",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::PromptEmbeddedBlePairingOnly {
                expected_name: Some("listenerB".to_string()),
            })
        );
    }

    #[test]
    fn parse_recognizes_embedded_ble_cleanup_with_name() {
        let args = vec![
            "listener-type",
            "--cleanup-embedded-ble-pairing",
            "OfficeType01",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::CleanupEmbeddedBlePairing {
                expected_name: Some("OfficeType01".to_string()),
            })
        );
    }

    #[test]
    fn parse_recognizes_firmware_ota_check() {
        let args = vec![
            "listener-type",
            "--firmware-ota-check",
            "ota_manifest.json",
            "firmware_ota.bin",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::FirmwareOta {
                manifest_path: PathBuf::from("ota_manifest.json"),
                firmware_path: PathBuf::from("firmware_ota.bin"),
                preflight_only: false,
                transfer: false,
            })
        );
    }

    #[test]
    fn parse_recognizes_firmware_ota_preflight() {
        let args = vec![
            "listener-type",
            "--firmware-ota-preflight",
            "ota_manifest.json",
            "firmware_ota.bin",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::FirmwareOta {
                manifest_path: PathBuf::from("ota_manifest.json"),
                firmware_path: PathBuf::from("firmware_ota.bin"),
                preflight_only: true,
                transfer: false,
            })
        );
    }

    #[test]
    fn parse_recognizes_firmware_ota_transfer() {
        let args = vec![
            "listener-type",
            "--firmware-ota-transfer",
            "ota_manifest.json",
            "firmware_ota.bin",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::FirmwareOta {
                manifest_path: PathBuf::from("ota_manifest.json"),
                firmware_path: PathBuf::from("firmware_ota.bin"),
                preflight_only: false,
                transfer: true,
            })
        );
    }

    #[test]
    fn parse_ignores_firmware_ota_without_two_paths() {
        let args = vec!["listener-type", "--firmware-ota-check", "ota_manifest.json"];
        assert_eq!(parse_cli_intent(&args), None);
    }

    #[test]
    fn parse_recognizes_wired_firmware_check() {
        let args = vec!["listener-type", "--wired-firmware-check", "release.zip"];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::WiredFirmware {
                package_path: PathBuf::from("release.zip"),
                port: None,
                baud: None,
                action: WiredFirmwareCliAction::Check,
            })
        );
    }

    #[test]
    fn parse_recognizes_wired_firmware_flash_with_port_and_baud() {
        let args = vec![
            "listener-type",
            "--wired-firmware-flash",
            "release.zip",
            "COM10",
            "460800",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::WiredFirmware {
                package_path: PathBuf::from("release.zip"),
                port: Some("COM10".to_string()),
                baud: Some(460_800),
                action: WiredFirmwareCliAction::Flash,
            })
        );
    }

    #[test]
    fn parse_recognizes_wired_firmware_boot_repair_alias() {
        let args = vec![
            "listener-type",
            "--wired-firmware-repair-bootloader",
            "release.zip",
            "COM10",
        ];
        assert_eq!(
            parse_cli_intent(&args),
            Some(CliIntent::WiredFirmware {
                package_path: PathBuf::from("release.zip"),
                port: Some("COM10".to_string()),
                baud: None,
                action: WiredFirmwareCliAction::BootRepair,
            })
        );
    }

    #[test]
    fn parse_accepts_cancel_alias() {
        // --cancel 也接受（research doc 5 节里写成 --cancel；为兼容两种写法都收）。
        let args = vec!["listener-type", "--cancel"];
        assert_eq!(parse_cli_intent(&args), Some(CliIntent::CancelDictation));
    }

    #[test]
    fn parse_ignores_unknown_args() {
        // GUI app 必须吃下未知参数照常起来。
        let args = vec!["listener-type", "--unknown-flag", "/some/path"];
        assert_eq!(parse_cli_intent(&args), None);
    }

    #[test]
    fn parse_returns_first_matching_intent() {
        // 多个 flag 时取首个，确定行为。
        let args = vec!["listener-type", "--toggle-dictation", "--toggle-qa"];
        assert_eq!(parse_cli_intent(&args), Some(CliIntent::ToggleDictation));
    }

    #[test]
    fn parse_skips_argv0_even_if_it_looks_like_a_flag() {
        // argv[0] 是进程路径，永远跳过。即便构造出诡异的"argv[0]=--toggle-dictation"
        // 也不应被当作 intent —— skip(1) 已保证。
        let args = vec!["--toggle-dictation"];
        assert_eq!(parse_cli_intent(&args), None);
    }

    #[test]
    fn parse_finds_intent_among_unknown_args() {
        let args = vec![
            "listener-type",
            "/path/to/file",
            "--toggle-dictation",
            "extra",
        ];
        assert_eq!(parse_cli_intent(&args), Some(CliIntent::ToggleDictation));
    }
}
