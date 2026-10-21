// Qwen LiveTranslate over WebRTC. Audio flows browser → Qwen; our server only exchanges the SDP.
// Rotation is make-before-break: the replacement session is fully configured and receiving the track before the old one
// stops receiving audio and gets session.finish, so its last sentence is still delivered (up to 10 s) instead of dropped.
import { qwenPhrases } from './glossary.js';
const ROTATE_MS = 5 * 60000, GRACE_MS = 60000, LAG_LIMIT = 8000, TAIL_MS = 10000;

function gather(pc, signal) {
  return new Promise((resolve, reject) => {
    const finish = error => { clearTimeout(timer); pc.removeEventListener('icegatheringstatechange', check); signal.removeEventListener('abort', cancel); error ? reject(error) : resolve(); };
    const check = () => { if (pc.iceGatheringState === 'complete') finish(); };
    const cancel = () => finish(new Error('已取消连接'));
    const timer = setTimeout(() => finish(new Error('ICE 收集超时')), 15000);
    pc.addEventListener('icegatheringstatechange', check); signal.addEventListener('abort', cancel, { once: true }); check();
  });
}

export function startQwen(o) {
  const track = o.stream.getAudioTracks()[0];
  const conns = new Set();
  let closed = false, active = null, pending = null, failures = 0, rotations = 0, retryAt = 0;

  const diag = c => o.diag(`SDP ${c.sdp} · ICE ${c.pc.iceConnectionState} · DataChannel ${[...c.channels].map(x => x.label + ':' + x.readyState).join(', ') || '—'} · 模型 ${c.ready ? 'ready' : '等待'} · 会话数 ${conns.size}`);
  function close(c) {
    if (c.dead) return;
    c.dead = true; clearTimeout(c.timeout); clearTimeout(c.tail);
    c.abort.abort(); c.pc.close(); conns.delete(c);
  }
  function finish(c) {
    if (!c || c.dead || c.finishing) return;
    c.finishing = true;
    c.sender.replaceTrack(null).catch(() => {});
    try { c.channel.send(JSON.stringify({ type: 'session.finish', event_id: crypto.randomUUID() })); } catch {}
    c.tail = setTimeout(() => close(c), TAIL_MS);
  }
  function lost(c, error) {
    const wasActive = c === active;
    close(c);
    if (closed || !wasActive) return;
    active = null;
    o.status(`连接中断，正在重连：${error.message}`, 'busy');
    rotate();
  }
  function open() {
    const pc = new RTCPeerConnection({ iceServers: [] });
    const c = { pc, abort: new AbortController(), channels: new Set(), channel: null, sdp: '等待', created: false, updated: false, ready: false, dead: false, finishing: false, lag: null };
    c.sender = pc.addTransceiver('audio', { direction: 'sendrecv' }).sender;
    conns.add(c);
    let settle;
    const ready = new Promise((resolve, reject) => settle = { resolve, reject });
    const failed = error => {
      if (c.dead) return;
      if (c.ready) return lost(c, error);
      close(c); settle.reject(error);
    };
    const init = () => {
      if (c.dead || !c.created || c.updated || pc.connectionState !== 'connected' || c.channel?.readyState !== 'open') return;
      c.updated = true;
      const translation = { language: o.language }, phrases = qwenPhrases(o.glossary(), o.language);
      if (phrases) translation.corpus = { phrases };
      c.channel.send(JSON.stringify({ type: 'session.update', event_id: crypto.randomUUID(), session: { output_modalities: ['text'], translation } }));
    };
    const bind = channel => {
      c.channels.add(channel);
      channel.onopen = () => { if (!c.dead) { diag(c); init(); } };
      channel.onclose = () => { if (channel === c.channel) c.finishing ? close(c) : failed(new Error('字幕通道已关闭')); };
      channel.onerror = () => { if (!c.finishing) failed(new Error('WebRTC 数据通道出错')); };
      channel.onmessage = async ({ data }) => {
        if (c.dead) return;
        let event; try { event = JSON.parse(data); } catch { return; }
        try {
          if (event.type === 'session.created') { c.created = true; c.channel = channel; init(); }
          else if (event.type === 'session.updated' && c.updated && !c.ready) {
            if (event.session?.output_modalities?.includes('audio')) throw new Error('上游未接受仅文本配置');
            await c.sender.replaceTrack(track);
            if (c.dead) return;
            c.ready = true; c.readyAt = performance.now(); clearTimeout(c.timeout); diag(c); settle.resolve(c);
          } else if (event.type === 'session.finished') close(c);
          else if (event.type === 'input_audio_buffer.speech_started' || event.type === 'input_audio_buffer.speech_stopped') {
            // Server offsets are relative to the audio it received; wall time since the track was attached minus that offset
            // is how far the model lags behind live speech.
            const at = event.audio_end_ms ?? event.audio_start_ms;
            if (c.readyAt && Number.isFinite(at)) c.lag = Math.max(0, performance.now() - c.readyAt - at);
            if (event.type === 'input_audio_buffer.speech_stopped') boundary(c);
          } else if (event.type === 'conversation.item.input_audio_transcription.completed') { if (o.transcript.accept(event)) o.render(); boundary(c); }
          else if (event.type === 'error' || event.type === 'conversation.item.input_audio_transcription.failed') {
            throw new Error(`Qwen 模型错误：${event.error?.code || event.error?.type || 'unknown'}`);
          } else if (event.type === 'response.done' && ['failed', 'incomplete'].includes(event.response?.status)) {
            throw new Error(`Qwen 翻译未完成：${event.response.status}`);
          } else if (o.transcript.accept(event)) o.render();
        } catch (error) { failed(error); }
      };
      diag(c);
    };
    pc.ondatachannel = ({ channel }) => bind(channel);
    bind(pc.createDataChannel('oai-events'));
    pc.onconnectionstatechange = () => {
      if (c.dead) return;
      diag(c); init();
      if (['failed', 'disconnected'].includes(pc.connectionState) && !c.finishing) failed(new Error(`WebRTC ${pc.connectionState}：网络可能阻止 UDP/ICE`));
    };
    pc.oniceconnectionstatechange = () => { if (!c.dead) diag(c); };
    c.timeout = setTimeout(() => failed(new Error('WebRTC / 模型初始化超时')), 45000);
    (async () => {
      await pc.setLocalDescription(await pc.createOffer());
      await gather(pc, c.abort.signal);
      const sdp = await o.api('sdp', pc.localDescription.sdp, 'application/sdp', c.abort.signal);
      if (c.dead) return;
      c.sdp = '成功'; diag(c);
      await pc.setRemoteDescription({ type: 'answer', sdp: sdp.trim().replace(/\r?\n/g, '\r\n') + '\r\n' });
    })().catch(error => failed(error));
    return ready;
  }
  // Scheduled switches wait for a sentence boundary so no utterance is split across two sessions.
  function boundary(c) {
    if (c === active && o.rotate && !pending && Date.now() >= retryAt && Date.now() - c.startedAt >= ROTATE_MS) rotate('已满 5 分钟');
  }
  async function rotate(reason) {
    if (closed || pending) return;
    const old = active;
    if (old) o.status(`${reason} · 正在无缝切换会话…`, 'live');
    else if (!failures) o.status('正在建立 WebRTC…', 'busy');
    try {
      pending = open();
      const c = await pending;
      pending = null;
      if (closed) return close(c);
      if (c.dead) throw new Error('新会话建立后立即断开');
      c.startedAt = Date.now(); active = c; failures = 0;
      if (old) { finish(old); rotations++; }
      o.status('翻译中 · 正在发送音频', 'live');
    } catch (error) {
      pending = null;
      if (closed) return;
      if (active) { retryAt = Date.now() + 30000; return o.status(`会话切换失败，继续使用当前会话：${error.message}`, 'live'); }
      if (++failures >= 3) return o.fail(error);
      o.status(`连接失败，${2 * failures} 秒后重试（${failures}/3）：${error.message}`, 'busy');
      setTimeout(() => rotate(), 2000 * failures);
    }
  }
  const ticker = setInterval(() => {
    const c = active;
    o.metrics({ lag: c?.lag ?? null, upload: null, age: c ? Date.now() - c.startedAt : 0, rotations });
    if (!c || pending || !o.rotate || Date.now() < retryAt) return;
    if (c.lag > LAG_LIMIT) rotate(`延迟 ${(c.lag / 1000).toFixed(1)} s 过高`);
    else if (Date.now() - c.startedAt >= ROTATE_MS + GRACE_MS) rotate('已满 5 分钟');
  }, 1000);
  rotate();
  return {
    stop() {
      closed = true; clearInterval(ticker);
      for (const c of [...conns]) c.ready ? finish(c) : close(c);
      active = null;
    },
  };
}
