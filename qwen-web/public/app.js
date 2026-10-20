import { Transcript } from './transcript.js';
const $ = id => document.getElementById(id);
let csrf = '', capture = null, connection = null, generation = 0, audio = null, analyser = null, monitor = null, animation = 0, popup = null;
let transcript = new Transcript();
const status = text => $('status').textContent = text;
const error = text => $('error').textContent = text;
const busy = value => {
  for (const id of ['start', 'test', 'source', 'device', 'devices', 'language', 'reconnect']) $(id).disabled = value;
  $('stop').disabled = !value;
};
function publish() {
  popup?.postMessage({ type: 'qwen-director', text: transcript.text(false), font: +$('font').value, width: +$('width').value }, location.origin);
}
function render() {
  for (const [id, source] of [['original', true], ['translated', false]]) {
    $(id).textContent = transcript.text(source);
    $(id).scrollTop = $(id).scrollHeight;
  }
  publish();
}
async function api(path, body, type = 'application/json', signal) {
  const response = await fetch(`./api/${path}`, { method: body === undefined ? 'GET' : 'POST', headers: { 'Content-Type': type, 'X-CSRF-Token': csrf }, body, signal });
  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `请求失败 (${response.status})`);
  }
  return type === 'application/sdp' ? response.text() : response.json();
}
function signedIn(value) {
  csrf = value || '';
  $('workspace').hidden = !csrf; $('logout').hidden = !csrf; $('login').hidden = !!csrf;
  if (csrf) initializeDevices();
}
$('loginForm').onsubmit = async event => {
  event.preventDefault(); error('');
  try { const result = await api('login', JSON.stringify({ password: $('password').value })); $('password').value = ''; signedIn(result.csrf); }
  catch (e) { error(e.message); }
};
$('logout').onclick = async () => { stop(); try { await api('logout', '{}'); signedIn(''); } catch (e) { error(e.message); } };
api('session').then(result => signedIn(result.csrf)).catch(() => signedIn(''));

async function enumerate() {
  const selected = $('device').value;
  const devices = await navigator.mediaDevices.enumerateDevices();
  $('device').replaceChildren(new Option('默认麦克风', ''));
  for (const device of devices.filter(d => d.kind === 'audioinput')) $('device').add(new Option(device.label || `麦克风 ${$('device').length}`, device.deviceId));
  if ([...$('device').options].some(o => o.value === selected)) $('device').value = selected;
}
async function initializeDevices() {
  error(''); const ticket = ++generation;
  busy(true); status('初始化音频 · 请允许麦克风权限，正在识别设备…');
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    stream.getTracks().forEach(t => t.stop());
    if (ticket !== generation) return;
    await enumerate();
    if (ticket === generation) status('音频设备已识别 · 麦克风已释放，可选择设备后开始');
  } catch (e) {
    if (ticket !== generation) return;
    status('麦克风尚未授权 · 可重试授权或选择共享音频');
    error(`无法访问麦克风：${e.name}。请在浏览器站点权限中允许麦克风，再点击「刷新设备 / 授权」。`);
  } finally {
    if (ticket === generation) { busy(false); $('source').onchange(); }
  }
}
$('devices').onclick = initializeDevices;
navigator.mediaDevices?.addEventListener('devicechange', () => enumerate().catch(() => {}));
$('source').onchange = () => $('device').disabled = $('source').value !== 'mic';

function meter(stream) {
  audio = new AudioContext();
  const input = audio.createMediaStreamSource(stream);
  analyser = audio.createAnalyser(); analyser.fftSize = 1024;
  input.connect(analyser);
  monitor = audio.createGain(); monitor.gain.value = $('monitor').checked ? 1 : 0;
  input.connect(monitor); monitor.connect(audio.destination);
  const data = new Float32Array(analyser.fftSize), canvas = $('wave'), ctx = canvas.getContext('2d');
  function draw() {
    analyser.getFloatTimeDomainData(data);
    const rms = Math.sqrt(data.reduce((sum, v) => sum + v * v, 0) / data.length);
    $('level').textContent = rms > .00001 ? `${Math.round(20 * Math.log10(rms))} dB` : '静音 · −∞ dB';
    ctx.clearRect(0, 0, canvas.width, canvas.height); ctx.strokeStyle = '#a5e5d2'; ctx.lineWidth = 2; ctx.beginPath();
    data.forEach((v, i) => { const x = i * canvas.width / data.length, y = 32 + v * 30; i ? ctx.lineTo(x, y) : ctx.moveTo(x, y); });
    ctx.stroke(); animation = requestAnimationFrame(draw);
  }
  audio.resume().catch(() => {}); draw();
}
$('monitor').onchange = () => { if (monitor) monitor.gain.value = $('monitor').checked ? 1 : 0; };
function closeConnection() {
  const old = connection; connection = null;
  if (!old) return;
  clearTimeout(old.timeout); clearTimeout(old.reconnect);
  old.abort.abort(); old.pc.close();
}
function stop(message = '已停止 · 设备已释放；字幕保留') {
  generation++; closeConnection();
  capture?.getTracks().forEach(t => t.stop()); capture = null;
  cancelAnimationFrame(animation); audio?.close().catch(() => {}); audio = null; analyser = null; monitor = null;
  $('wave').getContext('2d').clearRect(0, 0, 720, 64); $('level').textContent = '静音 · −∞ dB';
  $('diagnostics').textContent = 'SDP — · ICE closed · DataChannel closed · 模型已断开';
  busy(false); $('source').onchange(); status(message);
}
$('stop').onclick = () => stop();
function fail(e, ticket) { if (ticket !== generation) return; stop('连接停止 · 请检查错误后重试'); error(e.message); }
async function start(localOnly) {
  const ticket = ++generation; busy(true); error(''); status('等待音频权限…');
  try {
    const stream = $('source').value === 'display'
      ? await navigator.mediaDevices.getDisplayMedia({ video: true, audio: true })
      : await navigator.mediaDevices.getUserMedia({ audio: { deviceId: $('device').value ? { exact: $('device').value } : undefined, echoCancellation: false, noiseSuppression: false, autoGainControl: false } });
    if (ticket !== generation) { stream.getTracks().forEach(t => t.stop()); return; }
    capture = stream;
    if (!stream.getAudioTracks().length) throw new Error('分享来源没有音轨。请重新选择标签页并勾选「分享音频」；部分系统/浏览器不支持系统音频。');
    for (const track of stream.getTracks()) track.onended = () => { if (ticket === generation) stop('音频来源已结束 · 设备已释放'); };
    meter(new MediaStream(stream.getAudioTracks()));
    await enumerate();
    if (ticket !== generation) return;
    if (localOnly) status('本地试音 · 不连接 Qwen，不调用 API');
    else await connect(ticket);
  } catch (e) { fail(e.name === 'NotAllowedError' ? new Error('音频权限被拒绝或分享已取消。请允许麦克风/重新分享后再试。') : e, ticket); }
}
$('start').onclick = () => start(false);
$('test').onclick = () => start(true);
async function connect(ticket) {
  closeConnection();
  const pc = new RTCPeerConnection({ iceServers: [] });
  const c = { pc, abort: new AbortController(), created: false, updated: false, ready: false, channel: null, channels: new Set(), sdp: '等待' };
  connection = c;
  const alive = () => connection === c && generation === ticket;
  const diagnose = () => { if (alive()) $('diagnostics').textContent = `SDP ${c.sdp} · ICE ${pc.iceConnectionState} · DataChannel ${[...c.channels].map(x => x.label + ':' + x.readyState).join(', ') || '—'} · 模型 ${c.ready ? 'ready' : '等待'}`; };
  const init = () => {
    if (!alive() || !c.created || c.updated || pc.connectionState !== 'connected' || c.channel?.readyState !== 'open') return;
    c.updated = true;
    c.channel.send(JSON.stringify({ type: 'session.update', event_id: crypto.randomUUID(), session: { output_modalities: ['text'], translation: { language: $('language').value } } }));
    status('WebRTC 已连接 · 等待模型配置确认');
  };
  const sender = pc.addTransceiver('audio', { direction: 'sendrecv' }).sender;
  const bind = channel => {
    c.channels.add(channel);
    channel.onopen = () => { diagnose(); init(); };
    channel.onclose = () => { if (alive() && channel === c.channel) fail(new Error('字幕通道已关闭，请重新开始'), ticket); };
    channel.onerror = () => { if (alive()) fail(new Error('WebRTC 数据通道出错'), ticket); };
    channel.onmessage = async ({ data }) => {
      if (!alive()) return;
      try {
        const event = JSON.parse(data);
        if (event.type === 'session.created') { c.created = true; c.channel = channel; init(); }
        else if (event.type === 'session.updated' && c.updated && !c.ready) {
          if (event.session?.output_modalities?.includes('audio')) throw new Error('上游未接受仅文本配置，已停止采音');
          await sender.replaceTrack(capture.getAudioTracks()[0]);
          if (!alive()) return;
          c.ready = true; clearTimeout(c.timeout); status('翻译中 · 正在发送音频'); diagnose();
          if ($('reconnect').checked) c.reconnect = setTimeout(() => { if (alive()) { status('5 分钟到达 · 硬重连中（不补历史）'); connect(ticket).catch(e => fail(e, ticket)); } }, 300000);
        } else if (event.type === 'error' || event.type === 'conversation.item.input_audio_transcription.failed') {
          throw new Error(`Qwen 模型错误：${event.error?.code || event.error?.type || 'unknown'}`);
        } else if (event.type === 'response.done' && ['failed', 'incomplete'].includes(event.response?.status)) {
          throw new Error(`Qwen 翻译未完成：${event.response.status}`);
        } else if (transcript.accept(event)) render();
      } catch (e) { fail(e, ticket); }
    };
    diagnose();
  };
  pc.ondatachannel = ({ channel }) => bind(channel);
  bind(pc.createDataChannel('oai-events'));
  pc.onconnectionstatechange = () => {
    if (!alive()) return;
    diagnose(); init();
    if (['failed', 'disconnected'].includes(pc.connectionState)) fail(new Error(`WebRTC ${pc.connectionState}：网络可能阻止 UDP/ICE，请重试或换网络`), ticket);
  };
  pc.oniceconnectionstatechange = diagnose;
  c.timeout = setTimeout(() => { if (alive()) fail(new Error('WebRTC / 模型初始化超时。请检查网络与 Qwen 服务可用性。'), ticket); }, 45000);
  status('正在建立 WebRTC…');
  await pc.setLocalDescription(await pc.createOffer());
  await new Promise((resolve, reject) => {
    const finish = error => { clearTimeout(timer); pc.removeEventListener('icegatheringstatechange', check); c.abort.signal.removeEventListener('abort', cancel); error ? reject(error) : resolve(); };
    const check = () => { if (pc.iceGatheringState === 'complete') finish(); };
    const cancel = () => finish(new Error('已取消连接'));
    const timer = setTimeout(() => finish(new Error('ICE 收集超时')), 15000);
    pc.addEventListener('icegatheringstatechange', check); c.abort.signal.addEventListener('abort', cancel, { once: true }); check();
  });
  if (!alive()) return;
  const sdp = await api('sdp', pc.localDescription.sdp, 'application/sdp', c.abort.signal);
  if (!alive()) return;
  c.sdp = '成功'; diagnose();
  await pc.setRemoteDescription({ type: 'answer', sdp: sdp.trim().replace(/\r?\n/g, '\r\n') + '\r\n' });
}
$('clear').onclick = () => { transcript = new Transcript(); render(); };
$('director').onclick = () => {
  popup = window.open('./director.html', 'qwen-director', 'popup,width=1100,height=420');
  if (!popup) error('导播弹窗被浏览器拦截，请允许此网站弹窗后重试。');
  else { error(''); popup.focus(); publish(); }
};
window.addEventListener('message', event => { if (event.origin === location.origin && event.source === popup && event.data?.type === 'qwen-director-ready') publish(); });
for (const id of ['font', 'width']) $(id).oninput = () => { $(`${id}Value`).textContent = $(id).value + (id === 'font' ? ' px' : '%'); publish(); };
window.addEventListener('pagehide', () => stop());
