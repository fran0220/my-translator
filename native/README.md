# 原生实时翻译（GPUI Kit）

这是独立原生可执行程序，不启动 Tauri、不加载 HTML/JS，也不需要 WebView2。
现有 Tauri 紧急安装包、入口和发布流程保持不变。此分支只供审核整合；未发布。

## 上游与平台约束

准确依赖为 [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit)，固定 `gpui-kit =0.7.3`。
它正式包含 `gpui-component`，并非把用户指定的库替换成另一个库。
默认底层固定 `gpui-pre =0.3.9`，对应 Zed
`93d745668805029476e6d7fbb19652116fed51de`。

Windows 源码使用 Win32 HWND、Direct3D 11 和 DirectWrite；不依赖可选的 `gpui-webview`。
上游 Windows CI 执行无头测试，但真实渲染测试排除了 Windows。
运行要求 Windows 10+，支持所需 Direct3D 11 功能的显卡/驱动；RDP、虚拟机、中文 IME 和多屏 DPI 必须实机验证。

## Windows 构建与验收

在**独立目录**检出本次准确提交，不复用紧急 Tauri 包的私有源码、配置或产物。
安装 Rust MSVC 工具链（本次 Linux 使用 Rust 1.99.1）、Visual Studio 2022 C++ 桌面构建工具、
Windows SDK 和 CMake；SDK 的 `fxc.exe` 必须可用，或设置 `GPUI_FXC_PATH` 指向该文件。

```powershell
cargo test --manifest-path native/Cargo.toml --locked --no-default-features --lib
cargo build --manifest-path native/Cargo.toml --locked --release
.\native\target\release\translator-native.exe --replay native/tests/fixtures/broadcast.jsonl
```

回放模式不加载用户设置、不保存设置、不连接服务商。打开导播窗口，改变窗口宽度和字号，
观察最新最多三行**视觉行**，不是三个句子。然后退出回放，以普通方式启动，人工配置测试账户。
导播窗口没有内容控件；按住空白处拖动，`F11` 切换全屏，`Esc` 关闭，再从主窗口重新打开。
验收麦克风、WASAPI、混合采音、三家增量协议、千问双站点、双向 Soniox、断网故障切换、
代理鉴权、停止尾句、重复开始/停止、关闭/重开导播、Pinyin 输入、100%/150%/200% DPI。
真实服务端验收需另行提供授权测试凭据；本次没有使用生产密钥或声称完成这部分验收。

## Linux 开发与渲染

`.agents/setup` 安装构建与 X11 检查依赖；它只在本地提交，尚未配置远端后续 orb。

```sh
cargo test --manifest-path native/Cargo.toml --locked --no-default-features --lib
cargo build --manifest-path native/Cargo.toml --locked
native/target/debug/translator-native --replay native/tests/fixtures/broadcast.jsonl
```

本次 Debian 12 orb 的 llvmpipe Vulkan 能创建窗口但连最小 GPUI 程序也显示空白。
设置 `VK_ICD_FILENAMES=/nonexistent` 使 GPUI 选择同一上游支持的 OpenGL 后可实际渲染。
这只是该软件渲染环境的验证绕过，**不要加入 Windows 构建或产品默认设置**。
无显示器环境可用受监督的 `Xvfb :99 -screen 0 1280x1000x24 -ac` 与 `DISPLAY=:99 openbox`，
再运行 `DISPLAY=:99 VK_ICD_FILENAMES=/nonexistent XDG_RUNTIME_DIR=<0700临时目录>` 的回放进程。
Linux 尚未接入系统音频后端，使用麦克风；Windows 使用复用的 WASAPI 后端。

## 功能与协议归属

| 功能 | 实现与语义 |
| --- | --- |
| OpenAI | 直接编译原有 Rust `openai_realtime.rs`；24k 重采样、会话确认、增量追加、权威 final 替换、停止收尾。原生输入队列有界。 |
| 千问 | 直接编译原有 `qwen_realtime.rs`；中国 3.8 / 新加坡旧站协议保持；JS 中的 item-ID 关联、乱序最终事件、去重、快照替换已迁入 `transcript.rs`。 |
| Soniox | `soniox.rs` 移植 JS 的配置、双向语言、词表/上下文、token final/provisional、keepalive、180 秒先建后断轮换与上下文携带、空文本帧停止。 |
| 采音 | 复用现有 CPAL / WASAPI / ScreenCaptureKit、重采样；专用线程创建和释放采集。混合来源叠加 PCM，不把同时音频串接成双倍时长。 |
| 故障切换 | 首选服务优先，其余已配置服务作为备用，每家最多两次；停止优先，保留尾句，不向备用服务重放过期音频。双向仅 Soniox，不静默变为单向。 |
| 设置与代理 | 中文原生按钮/输入框、三家密钥、千问站点、HTTPS/WSS 中转与令牌；复用设置数据结构，未展示的旧字段保留。 |
| 导播 | 独立原生窗口；仅译文，黑底白字；GPUI 实际字体 shaping/wrapping 后裁切到最后三行，随宽度和字号重排。 |

主界面最近显示 40 个已完成段落，内存保留最近 1000 段。停止与切换会保留未完成字幕。
字幕导播是当前迁移范围；原 Tauri 版的多家朗读/TTS、历史会话管理、自动更新和本地离线翻译
尚未迁移到原生界面，不能把此入口当作这些功能已全部替代的正式发行版。
OpenAI 会生成音频但当前原生界面不播放；这不减少服务端音频费用。

## 轮换与停止收尾

Soniox 每 180 秒先建立替换连接。旧连接发送空文本帧后继续读取 final，收到 `finished: true`
才发布字幕边界、清理旧会话临时状态并使用新连接。新连接事件不会抢先混入旧字幕。
空二进制帧只是空音频，不结束输入；final token 或 `<end>` 也不等于会话结束确认。
旧连接最多等待 5 秒；期间新采集音频留在有界队列（约 3 秒），超时、断开或队列满
会明确报错并进入既有故障切换，而不是无限缓存或把失败伪装成成功。

停止先关闭采集，最多等待 1 秒接收异步采集线程的最后回调，再混合并发送不足 200ms 的尾块。
OpenAI 还会以静音补齐并排出重采样器的输入残留和滤波延迟。
OpenAI/Soniox 先发送已接收队列，再发送终止协议，保留最后服务端事件。
OpenAI/Soniox 收尾分别限时 10/5 秒；会话线程另有 16 秒网络等待上限，
超时或未收到最终确认的断开均报告尾句可能不完整。Tauri 的丢弃待发队列策略仍保留。
OpenAI 的 10 秒是包含待发输入和服务端输出的整体工程上限，不是官方 SLA；
收到 `session.closed` 立即返回，并不固定等待 10 秒。Tauri JS guard 为 11 秒，
给后端预算额外留 1 秒 IPC 余量；静音不停止服务端音频生成，也不免除收尾确认。

**千问例外：**每 300 秒取消旧 socket（包含尚未完成的握手、发送和读取），清除旧待发音频，
新建会话从最新音频开始。不发送 `session.finish`，不等 final，不补发历史音频。
首次连接和重连期间均持续丢弃输入并释放队列预算，直到收到 `session.updated`；
确认时也清除已有待发帧，只发送此后输入。不扩大 96KB 预算，也不重放等待期间的音频。
手动停止也立即取消，已显示文字保留；轮换会截断尚未返回的尾句，重连期间可能漏字。
原生采集暂存、服务队列与 Tauri 的 200ms 采集批次均有边界清理；Tauri IPC 携带采集时的
generation，拒收旧批次，避免 JS 延迟投递跨会话。旧连接事件不会交给新会话处理。
这只取消客户端旧工作，不能保证服务商全部延迟消失。没有 TTL、静音检测、双连接预热
或其他服务的新增定时重开策略。

核心测试包含本地 TCP WebSocket 服务端，不需要密钥：刻意让新连接事件早于旧 final，
验证旧 final → 边界 → 新字幕的顺序、轮换中停止、缺失 final 的超时与断开，
以及 OpenAI 在停止早于会话确认时仍逐字节发送排队音频和最后短块。
千问两站点另测加速轮换、丢弃旧音频/迟到事件、握手阻塞时停止，以及停止读取的对端导致
socket 写阻塞时仍能取消重开。测试限制对端接收缓冲，用正常 200ms 音频帧确认发送 future
实际 Pending 后推进虚拟时钟触发轮换，不依赖大帧或固定等待；产品仍维持 96KB 待发上限。
另覆盖异步采集最后回调及设备停止超时。这些测试不证明真实云服务或 Windows 音频驱动行为。

## 音频设备与本地试音

“音频设备 / 试音”使用 gpui-kit 的可搜索 Select、Progress 与真实 PCM 绘图。
输入和系统回放分别列出真实友好名称、Windows IMMDevice 端点 ID、默认标记、可用/禁用/未插入状态。
明确选定的 USB 麦克风、声卡或输出设备只按 ID 打开，不按名称匹配；同名设备不会混淆。
选择“系统默认”时，在每次开始采集时解析并锁定默认端点，运行中不会跟随默认设备变化。
选择保存于已有原生 Settings 的 `input_device_id` / `output_device_id`，不另建配置结构。

Windows 通过 WASAPI 共享模式采集输入和 render loopback。CPAL 0.15 的公共接口没有
稳定 Windows 端点 ID，因此保留 Tauri 的 CPAL 默认麦克风入口，但原生显式设备使用 WASAPI。
Linux 仅提供 CPAL 默认输入，不伪造可持久化 ID，也不声称支持 Windows 系统回放。

两路独立显示 PCM 波形、RMS/峰值 dBFS、设备已打开/收到音频/无输入/零 PCM 静音/削波；
支持读取 Windows 端点静音状态。静音表示设备静音或全零 PCM，不是语音活动检测。
服务连接与字幕已返回另外显示，避免把“设备打开”误当“翻译正常”。每两秒刷新设备；
选定端点离线或采集读取失败会显式告警并停止，不改采其他设备。重连设备后手动重新开始。

本地试音只创建音频采集，不创建服务商连接、不需要 key、不播放声音。试音与翻译互斥，
停止完成前禁止再次开始；采集中锁定设备选择。配置错误时开始/保存仍禁用，回放不读写配置。

### Windows 必须补做的验收

在独立目录构建准确提交，使用隔离的 `TRANSLATOR_NATIVE_CONFIG_DIR`；不要复制旧私有配置。
运行上文 native 测试，并运行 `cargo test --manifest-path src-tauri/Cargo.toml --locked --lib`、
`node --test scripts/realtime-captions.test.mjs`。本次 Linux 不能运行 Windows 硬件验收。

1. 两只同名 USB 输入及两个输出：核对 ID、默认/禁用/未插入标记；保存重启仍选原 ID。
2. 分别仅麦克风、仅系统回放、双路试音；对不同端点播放不同信号，确认选中且仅选中目标。
3. 切换默认端点不改当前采集；拔掉选中端点明确报错且不改采；插回后手动开始能恢复。
4. 静音、无数据、正常语音、削波分别显示正确；开机权限、独占占用、USB 移除都要检查。
5. 试音不访问翻译 API；运行中不能开始翻译/切换设备，停止后可反复试音或翻译。
6. Qwen 两站点真实运行跨越五分钟边界：旧会话取消、新音频继续、字幕保留、停止及时；
   不要求保留取消边界尾句，不把此测试解释成所有服务端延迟已消除。
7. 回归 Tauri 默认入口/双源采集、Qwen IPC 隔离、Soniox/OpenAI 故障切换和独立三行导播窗口。

## 凭据边界

普通构建的预置 JSON 永远为 `{}`，不读取环境中的 API key 或 `.env`。
显式 `--features private-team` 才允许构建脚本读取团队构建环境；本次未运行此模式。
敏感值只写入受限权限的 `OUT_DIR`，不打印到 Cargo 指令或日志，也不写入源码。
私有程序本身包含可提取凭据，不能公开分发。不能分享任何含密钥的本机设置文件。

原生版默认使用操作系统配置根目录下的 `com.personal.translator.native/settings.json`，
不会读取、迁移或修改 Tauri 的 `com.personal.translator` 配置。私有原生版在同一个原生目录下
使用 `settings-team.json`，也不继承 Tauri 私有配置。

原生独有的 `TRANSLATOR_NATIVE_CONFIG_DIR` 可显式指定配置目录，必须为**非空绝对路径**。
目录可尚不存在，首次保存时创建；只在启动时解析一次，读取和保存始终指向同一位置。
Windows 示例（不要复制旧 Tauri 配置或密钥）：

```powershell
$env:TRANSLATOR_NATIVE_CONFIG_DIR = 'C:\translator-native-test-config'
.\native\target\release\translator-native.exe
```

非法覆盖、路径不是目录、不可读或损坏的配置都会显示错误并禁止开始与保存，
不会回落到其他目录，也不会用默认配置覆盖坏文件；修复目录/文件后重启。
保存前会再次检查已有文件，使用同目录临时文件替换，避免写入失败截断原文件。
`--replay` 完全跳过配置路径解析、读取和保存，即使覆盖变量非法也能进行离线回放。
Tauri 的 `Settings::load/save` API、默认目录和容错行为不变。

## 验证边界

已运行的检查和截图以本线程最终交接为准。离线回放验证真实 GPUI 渲染及字幕模型，
不等于实时 WebSocket、真实音频设备或 Windows GUI 测试。Windows 验证由主线程协调，
本线程未创建 Windows 线程、未改动 Windows checkout、未推送或发布。
