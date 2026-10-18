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

## 手动更新与安全提示

此中文定制版已移除上游更新地址和公钥。启动检查与“检查更新”均只显示禁用提示，不发起外部更新检查，也不下载或安装上游版本。更新时请从自己信任的本分支构建记录下载新安装包，备份需要保留的设置与记录后手动安装。

**这是未签名的程序。** Windows SmartScreen 或杀毒软件可能提示未知发布者或阻止运行。请先核实源码、构建分支和产物来源；不确定时不要运行，不要关闭系统安全防护。构建成功不等于已在真实 Windows 设备验证：安装、启动、音频采集和服务连接仍需在 Windows 上测试。
