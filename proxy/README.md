# DMIT 实时翻译中转

运行链路：导播 Windows 客户端 → DMIT 上的 Caddy（HTTPS/WSS）→ Node.js 中转 → Soniox / OpenAI。Qwen 不经过中转。**Cloudflare 如用于域名管理，只开 DNS（灰云），不要开启橙云、Worker 路由或 Tunnel，否则客户端不再直接走 DMIT 的优化线路。** DMIT 机房、套餐及实际电信/联通/移动路由仍需现场验证。

只开放 `/soniox`、`/openai` 两条固定 WebSocket 路由和不含敏感信息的 `/healthz` 健康检查。不提供任意地址转发。文本保持文本、音频保持二进制、空文本结束帧保持为空；不攒句、不修改字幕。上游异常会关闭客户端连接，沿用客户端重连逻辑；不能补回断线期间丢失的音频。

Soniox API Key 随首条消息转发；OpenAI API Key 随 Authorization 请求头转发。两者不内置在安装包、不保存在服务器配置中。另设团队令牌鉴权：Soniox 使用 WebSocket 子协议头，OpenAI 使用独立请求头，均不放入 URL、不传给服务商。中转运营方能接触密钥和音频，仅使用团队控制的服务器。客户端本地配置文件为明文，勿共享。

## DMIT 部署

部署前确认服务器 IP、SSH 身份、部署目录和域名，检查现有网站及端口。以下步骤会修改线上资源；没有目标服务器授权时只做本地测试。

1. 准备 Linux DMIT 主机，安装 Docker Engine 和 Compose 插件。确认出站能建立 Soniox/OpenAI 的 TLS/WebSocket 连接，入站 TCP 80/443 可用；保持现有 SSH 访问规则，不覆盖其他防火墙配置。
2. 为独立子域名创建指向 DMIT 的 **DNS-only A 记录**。仅在 IPv6 路由也已验证时添加 AAAA。域名不得绑定冲突的 CDN/Worker 路由。
3. 将 `proxy/` 复制到服务器独立目录（如 `/opt/translator-relay`），不要覆盖已有项目。复制 `.env.example` 为 `.env`，权限设为 `600`。填写域名 `RELAY_DOMAIN`；通过密码管理器生成 32 字节随机数的十六进制表示（64 字符），填为 `RELAY_TOKEN`。不要使用示例/测试令牌，不要放服务商 API Key。
4. 在该目录执行 `docker compose config --quiet` 和 `docker compose up -d --build`。Caddy 自动申请并续期证书；DNS 与 80/443 必须能完成证书验证。证书存储在持久卷中，不要随意删除。
5. 执行 `docker compose ps`，确认 relay 健康、Caddy 运行。访问 `https://您的域名/healthz` 应返回 `ok`；无令牌的 WebSocket 请求应被拒绝。健康检查仅说明中转存活，不保证服务商密钥/网络有效。
6. 在客户端填写 `https://您的域名`、团队令牌及相应 Soniox/OpenAI API Key。地址不要带 `/soniox`、`/openai` 或参数。保存后点击服务商旁“测试”：Soniox 发送一秒静音，OpenAI 建立无输入音频的翻译会话，不读取麦克风。
7. 在实际导播电脑分别验证中译英、英译中、长时间运行/静音、断线重连、停止收尾，并测电信/联通/移动到该 DMIT 节点的实际延迟。orb 实测不等于现场网络已验证。

**如服务器已有 Nginx/Caddy 占用 80/443，不要停止或覆盖它。** 复用现有入口添加独立域名的 WebSocket 反代，只把 relay 端口发布到 `127.0.0.1:8787`；不可把未加密的 8787 端口开放到公网。现有反代须支持 Upgrade、禁用响应缓冲、设置足够长的读超时。仓库 Compose 默认不向宿主机发布 relay 端口。

## 运维

- Node 服务无请求/音频/字幕日志；Caddy 不启用访问日志，并从错误日志中移除 Authorization、Cookie 和团队令牌所在的头。不要开启全量请求或 WebSocket 调试日志。
- 固定上限 64 条活动或建连中的会话，1 MiB 单消息/发送队列上限，15 秒上游握手超时，30 秒心跳。过载关闭连接而不是无限占用内存或静默丢帧；凭据仍需保密。
- 更新 `.env` 中的团队令牌后执行 `docker compose up -d --force-recreate relay`，会断开现有会话。更新客户端令牌，再重连。无需更换个人服务商 API Key。
- `docker compose stop` 停止服务并保留证书；不要使用 `down -v` 删除证书卷。更新前保存旧版本，回滚只替换本目录代码并重建 relay，不修改其他网站。
- 客户端地址留空恢复直连；配置中转后失败不会自动绕过。不自动跨供应商切换。

## 本地验证

Node.js 22+，执行 `npm ci && npm test`。测试以真实本地 WebSocket 验证鉴权、固定上游、头部隔离、文本/二进制/空帧、尾句先于关闭、异常断开和重新连接；不使用真实 API Key，不部署资源。启动时必须设置有效 `RELAY_TOKEN`，`npm start` 默认仅监听 `127.0.0.1:8787`，生产外部 TLS 由 Caddy 提供。
