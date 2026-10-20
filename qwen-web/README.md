# Qwen WebRTC translation

Independent Node 22+ service, no build step or npm dependencies. It does not change the native app or existing relay.

## Deployment

Run `node server.mjs` from this directory (or `node qwen-web/server.mjs` from the repository). Required server environment: `DASHSCOPE_API_KEY`, `QWEN_PASSWORD`, `PUBLIC_ORIGIN` (exact HTTPS origin, no trailing slash). `PORT` defaults to 8788 and binds only 127.0.0.1. Put secrets in a root-owned 0600 EnvironmentFile outside the repository; never put them in client configuration. The requested internal-test password is 0000. No secrets are logged.

App prefix is `/qwen/`. On the dedicated hostname, redirect `/` to `/qwen/`; proxy `/qwen/` unchanged to `http://127.0.0.1:8788`. Nginx must overwrite `X-Real-IP` with `$remote_addr`, limit body to 64k, and use proxy read timeout 30s. Health: `GET /qwen/healthz`. No WebSocket upgrade is needed: audio flows from browser to Qwen directly. Preserve all unrelated vhosts/routes. No DNS changes are needed for the authorized `trans.local.jingao.club` deployment.

Example location (inside the dedicated TLS vhost):

```nginx
location = / { return 302 /qwen/; }
location /qwen/ {
    client_max_body_size 64k;
    proxy_pass http://127.0.0.1:8788;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header Host $host;
    proxy_read_timeout 30s;
}
```

Login sessions are in memory, expire after eight hours, and reset on service restart. Cookies are HttpOnly/Secure/SameSite=Strict. All mutations require exact Origin, authenticated calls require a session-bound CSRF token. Limits: login 10/IP/min and 100/global/min; SDP 8/session/min, 16/IP/min, 60/global/hour and four concurrent exchanges. A fixed Qwen 3.8 endpoint/model is used; no user-selected upstream. SDP timeout 20 seconds. These quotas limit session creation, not the lifetime/billing of an already established peer connection; the weak shared password is not suitable for public production use.

## Browser behavior

Use current desktop Chrome/Edge over HTTPS. Microphone selection and local waveform/monitoring work without contacting the model. Sharing requires an explicit browser picker and an audio-capable source with audio enabled. No silent system/WASAPI capture is possible. Shared video stays local; only the audio track is attached to Qwen after session.updated. Both client oai-events and server txt channels are handled.

Enabled-by-default five-minute hard reconnect closes the old peer immediately, creates a fresh model session without history, and preserves displayed text. It may drop speech at the boundary; it does not guarantee latency improvement. Stop releases all captured tracks immediately, without waiting for a final sentence. Source/translation text is retained only in page memory (up to 300 content parts), not persisted server-side. The director popup displays the bottom three visual lines, automatically reflowed by browser layout at any width/font size.

`npm test` tests security contracts and transcript assembly with a stub upstream; it does not claim cloud verification. Real browser/cloud results are reported separately. No TTS or alternate provider is included.
