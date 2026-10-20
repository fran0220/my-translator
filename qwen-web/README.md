# Web translation desk (Qwen · Soniox)

Independent Node 22+ service, no build step or npm dependencies. It does not change the native app or existing relay.

## Deployment

Run `node server.mjs` from this directory (or `node qwen-web/server.mjs` from the repository). Required server environment: `DASHSCOPE_API_KEY`, `QWEN_PASSWORD`, `PUBLIC_ORIGIN` (exact HTTPS origin, no trailing slash). Optional `SONIOX_API_KEY` enables the Soniox engine; the key needs the *Temporary API keys* and real-time STT permissions. `PORT` defaults to 8788 and binds only 127.0.0.1. Put secrets in a root-owned 0600 EnvironmentFile outside the repository; never put them in client configuration. The requested internal-test password is 0000. No secrets are logged.

App prefix is `/qwen/`. On the dedicated hostname, redirect `/` to `/qwen/`; proxy `/qwen/` unchanged to `http://127.0.0.1:8788`. Nginx must overwrite `X-Real-IP` with `$remote_addr`, limit body to 64k, and use proxy read timeout 30s. Health: `GET /qwen/healthz`. No WebSocket upgrade is needed: audio flows from the browser directly to Qwen (WebRTC) or Soniox (`wss://stt-rt.soniox.com`, allowed by the page CSP). Preserve all unrelated vhosts/routes. No DNS changes are needed for the authorized `trans.local.jingao.club` deployment.

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

Login sessions are in memory, expire after eight hours, and reset on service restart. Cookies are HttpOnly/Secure/SameSite=Strict. All mutations require exact Origin, authenticated calls require a session-bound CSRF token. Limits: login 10/IP/min and 100/global/min; SDP 8/session/min, 16/IP/min, 60/global/hour and four concurrent exchanges; Soniox temporary keys 12/session/min, 24/IP/min, 300/global/hour. Each Soniox key is single-use, must be used within 30 s and is bound to `client_reference_id=translator-web`; the long-lived key never reaches the browser. A fixed Qwen 3.8 endpoint/model is used; no user-selected upstream. SDP timeout 20 seconds. These quotas limit session creation, not the lifetime/billing of an already established peer connection; the weak shared password is not suitable for public production use.

## Browser behavior

Use current desktop Chrome/Edge over HTTPS. Microphone selection and local waveform/monitoring work without contacting the model. Sharing requires an explicit browser picker and an audio-capable source with audio enabled. No silent system/WASAPI capture is possible. Shared video stays local; only the audio track is attached to Qwen after session.updated. Both client oai-events and server txt channels are handled.

Engines: **Qwen 3.8 LiveTranslate** (WebRTC, one target language per session) and **Soniox stt-rt-v5** (browser WebSocket, 16 kHz PCM from an AudioWorklet). Soniox `中英双向` uses two-way translation: each utterance is translated into the other language, so bilingual speakers can switch freely. A single Qwen session cannot do this (same-language speech is returned in that language).

Session switching (enabled by default) is make-before-break: the new session is configured and receiving audio before the old one gets `session.finish` / the Soniox end frame; the old one keeps delivering its last sentence (up to 10 s). Scheduled switches (Qwen 5 min, Soniox 30 min) wait for the next sentence boundary, falling back after 60 s; a measured lag above 8 s (Qwen) / 6 s (Soniox) switches immediately. Latency is measured, not guessed: Qwen = wall time since audio attach minus the server's `audio_start_ms/audio_end_ms`; Soniox = audio on the wire minus `total_audio_proc_ms`, plus browser upload backlog (`bufferedAmount`). A Soniox uplink backlog above 8 s drops that socket and reconnects; 15 s without any Soniox response is treated as a stall. Stop releases all captured tracks immediately; open sessions still finish the current sentence. Source/translation text is retained only in page memory (up to 300 content parts), not persisted server-side. The director popup displays the bottom three visual lines, automatically reflowed by browser layout at any width/font size.

`npm test` tests security contracts (login/CSRF/quotas, SDP and Soniox key minting) and transcript assembly with stub upstreams; it does not claim cloud verification. Real browser/cloud results are reported separately. No TTS is included.
