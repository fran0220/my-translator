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
| Soniox | `soniox.rs` 移植 JS 的配置、双向语言、词表/上下文、token final/provisional、keepalive、180 秒先建后断轮换与上下文携带、空二进制停止。 |
| 采音 | 复用现有 CPAL / WASAPI / ScreenCaptureKit、重采样；专用线程创建和释放采集。混合来源叠加 PCM，不把同时音频串接成双倍时长。 |
| 故障切换 | 首选服务优先，其余已配置服务作为备用，每家最多两次；停止优先，保留尾句，不向备用服务重放过期音频。双向仅 Soniox，不静默变为单向。 |
| 设置与代理 | 中文原生按钮/输入框、三家密钥、千问站点、HTTPS/WSS 中转与令牌；复用设置数据结构，未展示的旧字段保留。 |
| 导播 | 独立原生窗口；仅译文，黑底白字；GPUI 实际字体 shaping/wrapping 后裁切到最后三行，随宽度和字号重排。 |

主界面最近显示 40 个已完成段落，内存保留最近 1000 段。停止与切换会保留未完成字幕。
字幕导播是当前迁移范围；原 Tauri 版的多家朗读/TTS、历史会话管理、自动更新和本地离线翻译
尚未迁移到原生界面，不能把此入口当作这些功能已全部替代的正式发行版。
OpenAI 会生成音频但当前原生界面不播放；这不减少服务端音频费用。

## 凭据边界

普通构建的预置 JSON 永远为 `{}`，不读取环境中的 API key 或 `.env`。
显式 `--features private-team` 才允许构建脚本读取团队构建环境；本次未运行此模式。
敏感值只写入受限权限的 `OUT_DIR`，不打印到 Cargo 指令或日志，也不写入源码。
私有程序本身包含可提取凭据，不能公开分发。普通运行仍沿用现有本机设置文件，不能分享此文件。

## 验证边界

已运行的检查和截图以本线程最终交接为准。离线回放验证真实 GPUI 渲染及字幕模型，
不等于实时 WebSocket、真实音频设备或 Windows GUI 测试。Windows 验证由主线程协调，
本线程未创建 Windows 线程、未改动 Windows checkout、未推送或发布。
