# Listener v1.0.6-rc1 · Type build190 / Firmware37

| Download | File |
| --- | --- |
| **Windows app** | **[Download MSI](https://github.com/Listener-ai-Macau/Listener-Type/releases/download/v1.0.6-rc1/ListenerType_1.0.6-rc1_x64_en-US.msi)** |
| **ESP32-S3 firmware · USB or Bluetooth OTA** | **[Download firmware ZIP](https://github.com/Listener-ai-Macau/Listener-Firmware/releases/download/v1.0.6-rc1/ListenerFirmware_1.0.6-rc1_ESP32S3.zip)** |

## In this app update

More tools now looks and behaves like an expandable control, with a clear row, arrow and keyboard focus. The question page and question window explain in five interface languages that questions are spoken through the computer microphone; typed questions are not currently supported. These changes address current software feedback without adding a new service, model or question workflow. Native checks cover the disclosure and localized content/action layout; they do not qualify actual microphone questions.

Navigation now mounts the selected page in the same render as its active sidebar item. It no longer leaves the old, blurred page on screen while an exit timer runs. The new-page animation, guide Skip preference and guide reopening are retained. Affected checks cover actual React rendering and the matching private native MSI; these are local UI checks, not speech or Bluetooth speed measurements.

Opening Providers from the question window now works after silent startup when no main window exists. Windows first-window creation no longer blocks its UI callback; navigation is retained until the new renderer is ready. Existing-window restore remains unchanged. Exact private-native checks observed first Providers opening in0.515 seconds and restoring the same native window in0.157 seconds; these are local UI timings, not recognition or Bluetooth throughput.

All new QA recordings now use the same recognition-and-answering configuration-presence check as the UI before reading selected text or opening capture. Missing answering setup is reported immediately, including requests through the common shortcut entry, instead of waiting until speech has been recognized. The existing Windows system-vault refresh is preserved. This check does not authenticate services or establish network/quota health. Native proof uses synthetic configuration and rejects before capture; it does not qualify real speech or a physical global key.

Empty WAV files now have a specific empty-file message rather than incorrectly showing the60-second limit. Oversize and unreadable-file errors remain distinct, and choosing a valid file after an error permits retry. This fixes an original file-input classification defect; no recognition service, model or microphone path was changed. An independent software user completed one included11.5-second English sample transcription and Copy; this is file-tool evidence, not real-time or hardware qualification.

The question window starts at a compact 360 × 300 logical pixels, with resizable edges and a 320 × 260 minimum. Setup help and long answers scroll within the content area while recording and Close controls remain visible. The window no longer forces the previous 380 × 440 size.

The question window now has visible Open, Start question, Finish and ask, and Close controls. On Windows, showing and hiding use one native visibility owner, fixing a window that could remain visible after Close. Optional shortcuts are kept.

Before a question starts, Type shows separate configuration-presence indicators for Volcengine recognition and the active AI answering service. Missing setup links directly to Providers, with APP ID, Access Token and Resource ID named consistently. Configured means values are present; it does not establish authentication, network or quota health. The question window uses the computer microphone. It does not change ordinary Listener dictation or board key mappings.

Existing windows and retained setup errors now follow the interface-language choice. Missing service setup no longer shows Ready while Start is disabled. The history switch explains local question/answer storage and that cloud processing still sends data. Exact native checks cover missing-configuration handling, direct settings navigation, language changes and Close; actual speech-to-answer/selected-context remain unqualified. The previously unqualified first-main-window case is covered by this update’s scoped native check. Earlier file-cancellation, diagnosis, device-profile and input-source fixes are retained. Firmware37 is unchanged; no reflash is needed for this app update.

## Before using

US$49 is the hardware price. Type is free and open source. Cloud recognition and optional AI services require preparation and separate usage charges; this release does not provide a ready-to-use official cloud account or allowance. USB flashing and Bluetooth OTA use the same firmware ZIP. Optional local translation components are prepared separately; general translation quality remains limited.

RC1 is a prerelease. Strong background-speech separation, some physical/global-key/optional-resource scopes and long-term acceptance remain incomplete. AI software feedback is not hardware qualification or a real order.

## File identity

- Type source: `37fa866e7fd0d952ac185013a718a0f78cd10272` · `build-source/type190-20261025`
- MSI: 299401216 bytes · SHA256 `9e34027a4ef6df3c74794f458f90049477367a810e0169be135b9ade6730b7a5`
- Firmware37 source: `87ea79ec3d6e34a95d09b73e2a5848207a919f1c` · runtime `9d7fbbe67ed0223d`
- ZIP: 1956799 bytes · SHA256 `fea25f0bf8f0d20edbb1e0ac326a5f026a0145915384372bde5f58d29b528b84`

## 中文说明

“更多工具”改为明确的展开整行、箭头与键盘焦点；提问入口和浮窗的五种语言说明这是电脑麦克风语音提问，当前不能键入问题。独立189软件顾客已自然打开提问窗，但未实际说话或得到答案；本批限定原生界面检查不替代真实问答。没有新增服务、模型或问答流程，固件37不变。

导航选中项与正文现在同步切换，不再等旧页退出模糊动画；首次引导的跳过记忆和重新打开保留。受影响的实际React渲染与匹配原生MSI已核对；本批不改录音、服务凭据、模型、设备设置或固件，不把界面耗时称作语音／蓝牙提速。

静默启动、主窗口尚未创建时，从提问窗打开提供商设置不再卡住。首次窗口创建不阻塞Windows界面回调，页面目的保留至渲染器准备好；已存在窗口的恢复逻辑保持。匹配私有原生环境首次进入约0.515秒，再次恢复同一窗口约0.157秒；这是本机界面耗时，不是识别或蓝牙速度。实际语音到回答／选区／物理按键仍未验收通过。

问答的共用开始入口现在与界面使用同一套识别＋回答配置存在性检查，在读取选区／采音前拦下未准备好的请求，包括共用快捷键路径；缺回答配置不会等说完才报错。保留原Windows凭据刷新；这不是鉴权、网络或额度检查。已在匹配原生程序用合成配置验证缺回答提示和关闭，未冒充真实语音／物理全局键验收。

空WAV文件现在单独提示为空，不再误报超过60秒；超长、读取失败仍分别提示，重新选择有效文件可继续操作。修复的是录音文件入口原有的错误分类，不改识别服务、模型或麦克风路径。独立软件体验员完成了一次内置11.5秒英文样例转写及复制；这不等于实时语音或硬件验收通过。

提问窗默认从380×440缩为360×300，可调整大小、最小320×260；配置帮助与长回答在内容区滚动，固定开始／结束／关闭控件保持可见。已做匹配原生默认／最小尺寸、机械缩放及关闭重开检查，不等于真人拖拽、讲话／识别／回答通过。

问答可以直接用按钮打开、开始、结束并提问、关闭，不要求记住快捷键。Windows显示／隐藏统一原生入口，修复后台已关闭但窗口还在的问题。开始前显示识别与回答两项配置存在状态，缺配置可直达提供商，字段统一为APP ID／Access Token／Resource ID；已开浮窗和已出现的配置错误随主界面语言更新。缺配置时不再提示准备就绪，历史开关说明本机保存与云处理的边界。问答仍使用电脑麦克风，保留Listener日用录音来源与板上按键。原生已检查相关配置、跳转、语言和关闭；配置存在不等于鉴权／网络／额度有效，真实讲话与选区到回答仍未完整验证；首次创建主窗已完成本批限定环境的原生检查，不代表物理触发或全部设备页面通过。固件37不变，无需为本次软件修复重刷。
