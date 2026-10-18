# Windows 简体中文安装包

本分支提供 Windows x64 的简体中文 NSIS 安装包，不包含自动更新签名或代码签名。

## 在 GitHub Actions 中构建

1. 将修改保存到自己的分支；要在 Actions 页面显示手动运行入口，工作流文件需存在于默认分支。
2. 打开 **Actions → Windows 简体中文安装包 → Run workflow**，选择需要构建的分支。
3. 等待成功，在该次运行的 **Artifacts** 中下载 `MyTranslator-windows-x64-zh-CN-unsigned`。
4. 解压下载的文件，运行其中的 `.exe` 安装程序。

此工作流仅手动触发，只上传保留 14 天的构建产物，不创建 GitHub Release，不需要配置 API Key、证书或签名密钥。原有 `release.yml` 不属于此构建流程；不要为此流程推送版本标签以免触发旧发布任务。

## 本机 Windows 构建

安装 Node.js 22、Rust stable（MSVC 工具链）、Visual Studio Build Tools 的“使用 C++ 的桌面开发”及 Windows SDK，并确保已安装 WebView2 Runtime。在仓库根目录运行：

```powershell
npm ci
npm run build -- --bundles nsis --config src-tauri/tauri.windows.conf.json
```

安装包位于 `src-tauri/target/release/bundle/nsis/`。首次构建需要联网下载 npm/Rust 依赖及打包工具。Windows 覆盖配置选择 `SimpChinese` 安装界面、仅生成 NSIS，并关闭 `createUpdaterArtifacts`，因此无需 `TAURI_SIGNING_PRIVATE_KEY`。Tauri 在 Windows 上也会自动合并该平台配置。

## API Key 与隐私

安装包不附带开发者的 API Key。启动后，请在设置中自行填写所选服务需要的个人 API Key；费用、额度和权限由自己的服务账号承担。不要将密钥提交到仓库或填写到工作流文件中。Windows 构建不会读取 `.env` 中的 `GOOGLE_FREE_TTS_KEY`，也会清空编译环境中的该密钥；Google Free TTS 同样需要在设置中输入自己的有效密钥，否则不可用。

## 中英双语与导播字幕

在“设置 → 翻译引擎 → 中英翻译快捷设置”选择“英文 → 中文”或“中文 → 英文”，然后保存。默认新配置为英文转中文；已有配置不会被覆盖。也可选择双向翻译，并将语言 A/B 设为英语和中文（Soniox）。

主界面点击“导播字幕”打开独立窗口，只显示译文，不显示原文，纯黑背景、白色文字；主界面仍保留双语。可拖到第二屏，由导播软件捕获此窗口的客户区；按 F 或双击全屏，按 + / − 调整字号。主窗口和导播窗口均显示增量字幕，不等待整句定稿；导播窗口每 100 毫秒同步一次当前文本，定稿替换临时文本，临时识别结果可能被服务修正。没有译文时保持黑屏，不回退显示原文。主界面清除字幕后输出也会清除。输出不包含设置或密钥。主窗口关闭或停止发送数据超过 5 秒后输出清空。

支持以下实时翻译服务：

- **Soniox `stt-rt-v5`**：原文和译文增量字幕，支持中英双向模式。
- **中国站 Qwen `qwen3.8-livetranslate-flash-realtime`**：`maas.qianwenaiapi.com`，原文和译文增量字幕；自动识别源语言，使用设置中的目标语言。停止后最多等待 15 秒接收尾句再保存。
- **新加坡旧版 Qwen `qwen3-livetranslate-flash-realtime`**：`dashscope-intl.aliyuncs.com`，仅译文增量字幕，需明确指定源语言。请在千问服务选择器中选择对应站点，填写该站点有效的密钥；不能假定密钥跨站通用。
- **OpenAI `gpt-realtime-translate`**：原文和译文连续增量字幕，不依赖句末事件。默认静音；服务端仍生成音频并计费，不等于纯文本计费模式。停止后发送 `session.close`，接收尾部字幕再保存。

Soniox 已有断线自动重连；此版不自动跨供应商切换，以免字幕能力和费用发生未经选择的变化。断网期间不能保证音频补发；恢复连接不等于补齐遗漏内容。字幕收尾时暂时禁用开始按钮，避免旧会话的尾句进入新会话。

API Key 保存在当前 Windows 用户的本地配置文件中，并非加密保险箱；不要共享配置文件或在录屏时显示密钥。

## Soniox / OpenAI 网络中转

在“设置 → 翻译引擎”填写团队中转域名和团队令牌。两种服务共用入口，仍使用各自 API Key；Qwen 不变。地址留空则直连，填写后失败不会自动绕过中转。服务商旁的“测试”按钮也经过配置的中转，验证实际实时接口，不再用模型列表请求代替 OpenAI 翻译连接测试。

管理员部署说明见 [实时翻译中转](../proxy/README.md)。仓库未预设线上域名或团队令牌；部署和实际导播网络测试完成后才可作为生产入口使用。

## 手动更新与安全提示

此中文定制版已移除上游更新地址和公钥。启动检查与“检查更新”均只显示禁用提示，不发起外部更新检查，也不下载或安装上游版本。更新时请从自己信任的本分支构建记录下载新安装包，备份需要保留的设置与记录后手动安装。

**这是未签名的程序。** Windows SmartScreen 或杀毒软件可能提示未知发布者或阻止运行。请先核实源码、构建分支和产物来源；不确定时不要运行，不要关闭系统安全防护。构建成功不等于已在真实 Windows 设备验证：安装、启动、音频采集和服务连接仍需在 Windows 上测试。
