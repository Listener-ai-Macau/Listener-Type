# File Traceability

Every tracked code/config/script file must be listed here. Regenerate with `npm run check:traceability -- --write`.

| File | Responsibility | Verification |
| --- | --- | --- |
| `index.html` | Core app | Build and targeted tests |
| `package-lock.json` | Build and release config | Build, updater and audit scripts |
| `package.json` | Build and release config | Build, updater and audit scripts |
| `scripts/build-mac.sh` | Automation and audits | Node/PowerShell script tests |
| `scripts/check-brand-residue.mjs` | Automation and audits | Node/PowerShell script tests |
| `scripts/check-cloud-services.mjs` | Automation and audits | Node/PowerShell script tests |
| `scripts/check-doc-inheritance.mjs` | Automation and audits | Node/PowerShell script tests |
| `scripts/check-hotkey-injection.mjs` | Global hotkeys | Hotkey tests and manual smoke |
| `scripts/check-hotkey-recorder.mjs` | Global hotkeys | Hotkey tests and manual smoke |
| `scripts/check-tauri-info.mjs` | Automation and audits | Node/PowerShell script tests |
| `scripts/check-traceability.mjs` | Automation and audits | Node/PowerShell script tests |
| `scripts/check-window-hotkey-fallback.mjs` | Global hotkeys | Hotkey tests and manual smoke |
| `scripts/windows-build-gnu.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-capsule-lifecycle-smoke.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-capsule-watch.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-clipboard-consumer-timing-smoke.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-cold-start.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-hotkey-injection-smoke.ps1` | Global hotkeys | Hotkey tests and manual smoke |
| `scripts/windows-hotkey-os-hook-smoke.ps1` | Global hotkeys | Hotkey tests and manual smoke |
| `scripts/windows-ime-build.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-ime-install-smoke.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-ime-register.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-ime-unregister.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-listener-type-lifecycle-e2e.py` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-microphone-privacy-smoke.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-open-dev.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-package-msvc.cmd` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-package-msvc.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-package-msvc.test.mjs` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-preflight.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-real-asr-insertion-smoke.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-real-regression.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-runtime-smoke.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-smoke-suite.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-startup-lifecycle-contract.test.mjs` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-terminal-clipboard-restore-smoke.ps1` | Automation and audits | Node/PowerShell script tests |
| `scripts/windows-ui-config.test.mjs` | Automation and audits | Node/PowerShell script tests |
| `scripts/write-updater-manifest.mjs` | Automation and audits | Node/PowerShell script tests |
| `scripts/write-updater-manifest.test.mjs` | Automation and audits | Node/PowerShell script tests |
| `src-tauri/Cargo.toml` | Build and release config | Build, updater and audit scripts |
| `src-tauri/Entitlements.plist` | Core app | Build and targeted tests |
| `src-tauri/Info.plist` | Core app | Build and targeted tests |
| `src-tauri/backend-tests/Cargo.toml` | Build and release config | Build, updater and audit scripts |
| `src-tauri/backend-tests/tests/backend_rust.rs` | Core app | Build and targeted tests |
| `src-tauri/src/asr/bailian.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/frame.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/cache.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/download.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/foundry.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/foundry_native.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/foundry_provider.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/foundry_runtime.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/local_provider.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/mod.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/models.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/qwen_engine.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/qwen_ffi.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/local/test_run.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/mod.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/volcengine.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/wav.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/asr/whisper.rs` | ASR providers | Provider tests, dictation smoke |
| `src-tauri/src/audio_mute.rs` | Core app | Build and targeted tests |
| `src-tauri/src/cli.rs` | Core app | Build and targeted tests |
| `src-tauri/src/combo_hotkey.rs` | Global hotkeys | Hotkey tests and manual smoke |
| `src-tauri/src/commands.rs` | Core app | Build and targeted tests |
| `src-tauri/src/coordinator.rs` | Core app | Build and targeted tests |
| `src-tauri/src/coordinator/dictation.rs` | Core app | Build and targeted tests |
| `src-tauri/src/coordinator/qa.rs` | Core app | Build and targeted tests |
| `src-tauri/src/coordinator/resources.rs` | Core app | Build and targeted tests |
| `src-tauri/src/coordinator_state.rs` | Core app | Build and targeted tests |
| `src-tauri/src/correction.rs` | Core app | Build and targeted tests |
| `src-tauri/src/embedded_audio.rs` | Core app | Build and targeted tests |
| `src-tauri/src/embedded_ble.rs` | Core app | Build and targeted tests |
| `src-tauri/src/global_hotkey_runtime.rs` | Global hotkeys | Hotkey tests and manual smoke |
| `src-tauri/src/hotkey.rs` | Global hotkeys | Hotkey tests and manual smoke |
| `src-tauri/src/insertion.rs` | Core app | Build and targeted tests |
| `src-tauri/src/lib.rs` | Core app | Build and targeted tests |
| `src-tauri/src/llm_gemini.rs` | Polish providers | Rust unit tests, provider smoke |
| `src-tauri/src/main.rs` | Core app | Build and targeted tests |
| `src-tauri/src/permissions.rs` | Core app | Build and targeted tests |
| `src-tauri/src/persistence.rs` | Local data and style packs | Rust unit tests, local import/export smoke |
| `src-tauri/src/polish.rs` | Polish providers | Rust unit tests, provider smoke |
| `src-tauri/src/qa_hotkey.rs` | Global hotkeys | Hotkey tests and manual smoke |
| `src-tauri/src/recorder.rs` | Core app | Build and targeted tests |
| `src-tauri/src/selection.rs` | Core app | Build and targeted tests |
| `src-tauri/src/shortcut_binding.rs` | Core app | Build and targeted tests |
| `src-tauri/src/types.rs` | Core app | Build and targeted tests |
| `src-tauri/src/unicode_keystroke.rs` | Core app | Build and targeted tests |
| `src-tauri/src/windows_ime_ipc.rs` | Windows IME insertion | Windows static and runtime smoke |
| `src-tauri/src/windows_ime_profile.rs` | Windows IME insertion | Windows static and runtime smoke |
| `src-tauri/src/windows_ime_protocol.rs` | Windows IME insertion | Windows static and runtime smoke |
| `src-tauri/src/windows_ime_session.rs` | Windows IME insertion | Windows static and runtime smoke |
| `src-tauri/tauri.conf.json` | Build and release config | Build, updater and audit scripts |
| `tools/embedded_audio_replay/Cargo.lock` | Embedded BLE audio | VKA1 replay smoke and seeded ASR accuracy |
| `tools/embedded_audio_replay/Cargo.toml` | Embedded BLE audio | VKA1 replay smoke and seeded ASR accuracy |
| `tools/embedded_audio_replay/generate_tts_fixtures.ps1` | Embedded BLE audio | Seeded TTS fixture generation |
| `tools/embedded_audio_replay/measure_asr_accuracy.ps1` | Embedded BLE audio | Seeded ASR accuracy / CER report |
| `tools/embedded_audio_replay/run_ble_stream_smoke.ps1` | Embedded BLE audio | Serial-triggered BLE streaming smoke |
| `tools/embedded_audio_replay/src/main.rs` | Embedded BLE audio | VKA1 replay smoke |
| `tools/volcengine_asr_probe/Cargo.lock` | ASR providers | Volcengine ASR smoke and cargo check |
| `tools/volcengine_asr_probe/Cargo.toml` | ASR providers | Volcengine ASR smoke and cargo check |
| `tools/volcengine_asr_probe/src/asr/mod.rs` | ASR providers | Volcengine ASR smoke and cargo check |
| `tools/volcengine_asr_probe/src/main.rs` | ASR providers | Volcengine ASR smoke and cargo check |
| `src-tauri/vendor/qwen-asr/Makefile` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/asr_regression.py` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/download_model.sh` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/main.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/python_simple_implementation.py` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr.h` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_audio.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_audio.h` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_decoder.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_encoder.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_kernels.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_kernels.h` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_kernels_avx.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_kernels_generic.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_kernels_impl.h` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_kernels_neon.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_safetensors.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_safetensors.h` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_tokenizer.c` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/vendor/qwen-asr/qwen_asr_tokenizer.h` | Vendored local ASR engine | Cargo check and local ASR smoke |
| `src-tauri/wix/listener-type-ime.wxs` | Windows IME insertion | Windows static and runtime smoke |
| `src/App.tsx` | Core app | Build and targeted tests |
| `src/components/AutoUpdate.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/AutoUpdateGate.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/Capsule.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/FloatingShell.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/Icon.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/MarketplaceModal.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/Onboarding.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/SavedToast.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/SettingsModal.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/ShortcutRecorder.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/WindowChrome.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/ui/Row.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/ui/SegSimple.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/ui/SelectLite.tsx` | React UI | TypeScript build and visual smoke |
| `src/components/ui/SwitchLite.tsx` | React UI | TypeScript build and visual smoke |
| `src/i18n/en.ts` | Localization | TypeScript build |
| `src/i18n/index.ts` | Localization | TypeScript build |
| `src/i18n/ja.ts` | Localization | TypeScript build |
| `src/i18n/ko.ts` | Localization | TypeScript build |
| `src/i18n/zh-CN.ts` | Localization | TypeScript build |
| `src/i18n/zh-TW.ts` | Localization | TypeScript build |
| `src/lib/appVersion.ts` | Core app | Build and targeted tests |
| `src/lib/capsuleLayout.test.ts` | Core app | Build and targeted tests |
| `src/lib/capsuleLayout.ts` | Core app | Build and targeted tests |
| `src/lib/fontScale.ts` | Core app | Build and targeted tests |
| `src/lib/hotkey.ts` | Global hotkeys | Hotkey tests and manual smoke |
| `src/lib/hotkeyMigration.ts` | Global hotkeys | Hotkey tests and manual smoke |
| `src/lib/hotkeyRecorder.test.ts` | Global hotkeys | Hotkey tests and manual smoke |
| `src/lib/hotkeyRecorder.ts` | Global hotkeys | Hotkey tests and manual smoke |
| `src/lib/ipc.ts` | Core app | Build and targeted tests |
| `src/lib/localAsr.ts` | Core app | Build and targeted tests |
| `src/lib/mockData.ts` | Core app | Build and targeted tests |
| `src/lib/providerSetup.test.ts` | Core app | Build and targeted tests |
| `src/lib/providerSetup.ts` | Core app | Build and targeted tests |
| `src/lib/qaMarkdown.test.ts` | Core app | Build and targeted tests |
| `src/lib/qaMarkdown.ts` | Core app | Build and targeted tests |
| `src/lib/savedEvent.ts` | Core app | Build and targeted tests |
| `src/lib/stylePrefs.test.ts` | Core app | Build and targeted tests |
| `src/lib/stylePrefs.ts` | Core app | Build and targeted tests |
| `src/lib/types.ts` | Core app | Build and targeted tests |
| `src/lib/vocab-presets.json` | Core app | Build and targeted tests |
| `src/lib/vocabPresets.ts` | Core app | Build and targeted tests |
| `src/lib/windowHotkeyFallback.test.ts` | Core app | Build and targeted tests |
| `src/lib/windowHotkeyFallback.ts` | Core app | Build and targeted tests |
| `src/main.tsx` | Core app | Build and targeted tests |
| `src/pages/History.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/LocalAsr.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/Marketplace.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/Overview.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/QaPanel.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/SelectionAsk.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/Settings.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/Style.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/Translation.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/Vocab.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/_atoms.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/settings/AboutUpdateControl.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/settings/AdvancedSection.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/settings/EmbeddedBleStatusPanel.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/settings/LanguageSection.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/settings/PermissionsSection.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/settings/ShortcutsSection.tsx` | React UI | TypeScript build and visual smoke |
| `src/pages/settings/shared.tsx` | React UI | TypeScript build and visual smoke |
| `src/state/HotkeySettingsContext.tsx` | Core app | Build and targeted tests |
| `src/state/useAppState.ts` | Core app | Build and targeted tests |
| `src/styles/global.css` | Design system | Visual smoke |
| `src/styles/tokens.css` | Design system | Visual smoke |
| `src/types/tauri-plugin-autostart.d.ts` | Core app | Build and targeted tests |
| `src/vite-env.d.ts` | Core app | Build and targeted tests |
| `tsconfig.json` | Core app | Build and targeted tests |
| `tsconfig.node.json` | Core app | Build and targeted tests |
| `vite.config.ts` | Core app | Build and targeted tests |
| `windows-ime/ListenerTypeIme.sln` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/ListenerTypeIme.vcxproj` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/ListenerTypeIme.def` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/class_factory.cpp` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/class_factory.h` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/dllmain.cpp` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/edit_session.cpp` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/edit_session.h` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/guids.h` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/ipc_client.cpp` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/ipc_client.h` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/registry.cpp` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/registry.h` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/resource.rc` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/text_service.cpp` | Windows IME insertion | Windows static and runtime smoke |
| `windows-ime/src/text_service.h` | Windows IME insertion | Windows static and runtime smoke |
