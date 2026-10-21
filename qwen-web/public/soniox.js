// Soniox real-time engine: the browser streams PCM straight to Soniox with a single-use temporary key minted by our server.
// Sessions rotate make-before-break (scheduled ones at the next sentence end): audio switches to the new socket, the old one gets the end-of-stream frame and keeps
// delivering its final tokens until Soniox reports finished, so nothing spoken at the boundary is lost.
import { sonioxContext } from './glossary.js';
const ENDPOINT = 'wss://stt-rt.soniox.com/transcribe-websocket';
const ROTATE_MS = 30 * 60000, GRACE_MS = 60000, LAG_LIMIT = 6000, UPLOAD_LIMIT = 8000, STALL_MS = 15000, TAIL_MS = 10000, BUFFER_MS = 5000;
const due = '已满 30 分钟';
const FATAL = new Set([400, 401, 402, 429]);

function sonioxError(r) {
  const known = { 401: 'Soniox 密钥无效或已过期', 402: 'Soniox 余额或月度额度已用尽', 429: 'Soniox 并发或频率超限，请稍后再试' };
  const error = new Error(known[r.error_code] || `Soniox 错误 ${r.error_code}：${r.error_message || r.error_type || 'unknown'}`);
  error.fatal = FATAL.has(r.error_code);
  return error;
}

export async function startSoniox(o) {
  await o.audio.audioWorklet.addModule('./pcm-worklet.js');
  const rate = o.audio.sampleRate, bytesPerMs = rate * 2 / 1000;
  const node = new AudioWorkletNode(o.audio, 'pcm16');
  o.input.connect(node); node.connect(o.audio.destination);
  const conns = new Set();
  let closed = false, active = null, pending = null, failures = 0, rotations = 0, retryAt = 0, backlog = [], backlogBytes = 0;

  const send = (c, chunk) => { c.ws.send(chunk); c.sent += chunk.byteLength; };
  // Audio captured while no socket is open (startup, reconnect) is held for up to 5 s and flushed to the next session.
  node.port.onmessage = ({ data }) => {
    if (closed) return;
    if (active?.ws.readyState === WebSocket.OPEN) return send(active, data);
    backlog.push(data); backlogBytes += data.byteLength;
    while (backlogBytes > BUFFER_MS * bytesPerMs) backlogBytes -= backlog.shift().byteLength;
  };

  function config() {
    const cfg = {
      model: 'stt-rt-v5', audio_format: 'pcm_s16le', sample_rate: rate, num_channels: 1,
      enable_endpoint_detection: true, enable_language_identification: true, language_hints: ['zh', 'en'],
      translation: o.language === 'two_way' ? { type: 'two_way', language_a: 'zh', language_b: 'en' } : { type: 'one_way', target_language: o.language },
    };
    const recent = o.transcript.recent(400), glossary = sonioxContext(o.glossary(), o.language);
    if (recent || glossary) cfg.context = { ...glossary, ...(recent && { text: `Recent conversation: ${recent}` }) };
    return cfg;
  }
  function close(c) {
    if (c.dead) return;
    c.dead = true; clearTimeout(c.tail); conns.delete(c);
    try { c.ws.close(); } catch {}
  }
  function finish(c) {
    if (!c || c.dead || c.finishing) return;
    c.finishing = true;
    try { c.ws.send(''); } catch {}
    c.tail = setTimeout(() => close(c), TAIL_MS);
  }
  function lost(c, error) {
    const wasActive = c === active;
    close(c);
    if (closed || !wasActive) return;
    active = null;
    if (error.fatal) return o.fail(error);
    o.status(`连接中断，正在重连：${error.message}`, 'busy');
    rotate();
  }
  async function open() {
    const { api_key: key } = await o.api('soniox-key', '{}');
    if (closed) throw new Error('已停止');
    const ws = new WebSocket(ENDPOINT, ['soniox-api-key', key]);
    const c = { ws, sent: 0, lag: null, last: performance.now(), dead: false, finishing: false, cursor: o.transcript.cursor() };
    conns.add(c);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Soniox 连接超时')), 10000);
      ws.onopen = () => { clearTimeout(timer); resolve(); };
      ws.onerror = () => { clearTimeout(timer); reject(new Error('无法连接 Soniox，请检查网络')); };
    }).catch(error => { close(c); throw error; });
    ws.onerror = null;
    ws.send(JSON.stringify(config()));
    ws.onmessage = ({ data }) => {
      if (c.dead) return;
      let r; try { r = JSON.parse(data); } catch { return; }
      c.last = performance.now();
      if (r.error_code) return lost(c, sonioxError(r));
      // Backlog = audio already on the wire minus audio Soniox has processed (final + non-final).
      if (Number.isFinite(r.total_audio_proc_ms)) c.lag = Math.max(0, (c.sent - ws.bufferedAmount) / bytesPerMs - r.total_audio_proc_ms);
      if (o.transcript.accept(c.cursor, r)) o.render();
      if (r.finished) return close(c);
      // Scheduled switches wait for a sentence boundary so no utterance is split across two sessions.
      if (c === active && o.rotate && Date.now() >= retryAt && Date.now() - c.startedAt >= ROTATE_MS && r.tokens?.some(t => t.text === '<end>' && t.is_final)) rotate(due);
    };
    ws.onclose = () => { if (!c.dead) lost(c, new Error('Soniox 连接已断开')); };
    return c;
  }
  async function rotate(reason) {
    if (closed || pending) return;
    const old = active;
    if (old) o.status(`${reason} · 正在无缝切换会话…`, 'live');
    else if (!failures) o.status('正在连接 Soniox…', 'busy');
    try {
      pending = open();
      const c = await pending;
      pending = null;
      if (closed) return close(c);
      if (c.dead) throw new Error('新会话建立后立即断开');
      c.startedAt = Date.now(); active = c; failures = 0;
      for (const chunk of backlog) send(c, chunk);
      backlog = []; backlogBytes = 0;
      if (old) { finish(old); rotations++; }
      o.status('翻译中 · 正在发送音频', 'live');
    } catch (error) {
      pending = null;
      if (closed) return;
      if (error.fatal) return o.fail(error);
      if (active) { retryAt = Date.now() + 30000; return o.status(`会话切换失败，继续使用当前会话：${error.message}`, 'live'); }
      if (++failures >= 3) return o.fail(error);
      o.status(`连接失败，${2 * failures} 秒后重试（${failures}/3）：${error.message}`, 'busy');
      setTimeout(() => rotate(), 2000 * failures);
    }
  }
  const ticker = setInterval(() => {
    const c = active, upload = c ? c.ws.bufferedAmount / bytesPerMs : null;
    o.metrics({ lag: c?.lag ?? null, upload, age: c ? Date.now() - c.startedAt : 0, rotations });
    o.diag(`Soniox stt-rt-v5 · WebSocket ${c ? '已连接' : pending ? '连接中' : '—'} · ${rate} Hz PCM · 连接数 ${conns.size}`);
    if (!c || pending) return;
    if (performance.now() - c.last > STALL_MS) return lost(c, new Error('Soniox 超过 15 秒无响应'));
    if (!o.rotate || Date.now() < retryAt) return;
    // A congested uplink cannot be drained in time; drop that socket so captions return to real time.
    if (upload > UPLOAD_LIMIT) return lost(c, new Error(`上行积压 ${(upload / 1000).toFixed(1)} s，丢弃积压并重连`));
    if (c.lag > LAG_LIMIT) rotate(`延迟 ${(c.lag / 1000).toFixed(1)} s 过高`);
    else if (Date.now() - c.startedAt >= ROTATE_MS + GRACE_MS) rotate(due);
  }, 1000);
  rotate();
  return {
    stop() {
      closed = true; clearInterval(ticker);
      node.port.onmessage = null; node.disconnect();
      for (const c of [...conns]) c === active || c.finishing ? finish(c) : close(c);
      active = null;
    },
  };
}
