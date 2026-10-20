import { Transcript, SonioxTranscript } from './transcript.js';
import { startQwen } from './qwen.js';
import { startSoniox } from './soniox.js';
const $ = id => document.getElementById(id);
const DIRECTIONS = {
  qwen: [['zh', '英文 → 中文'], ['en', '中文 → 英文']],
  soniox: [['two_way', '中英双向 · 自动识别切换'], ['zh', '任意语言 → 中文'], ['en', '任意语言 → 英文']],
};
const IDLE_DIAG = { qwen: 'SDP — · ICE — · DataChannel — · 模型 —', soniox: 'Soniox — · WebSocket —' };
let csrf = '', available = { qwen: true, soniox: false }, capture = null, session = null, generation = 0;
let audio = null, input = null, analyser = null, monitor = null, animation = 0, popup = null;
const prefs = JSON.parse(localStorage.getItem('qwen-view') || '{}');
let engine = prefs.engine === 'soniox' ? 'soniox' : 'qwen';
let transcript = engine === 'soniox' ? new SonioxTranscript() : new Transcript();
const status = (text, state = 'idle') => { $('status').textContent = text; $('status').dataset.state = state; };
const error = text => $('error').textContent = text;
const savePrefs = () => localStorage.setItem('qwen-view', JSON.stringify(prefs));
const busy = value => {
  for (const id of ['start', 'test', 'source', 'device', 'devices', 'language', 'reconnect']) $(id).disabled = value;
  for (const b of document.querySelectorAll('[data-engine]')) b.disabled = value || !available[b.dataset.engine];
  $('stop').disabled = !value;
};
function publish() {
  popup?.postMessage({ type: 'qwen-director', text: transcript.text(false), font: +$('font').value, width: +$('width').value }, location.origin);
}
// Auto-follow the newest line unless the reader scrolled up; offer a jump-back button instead.
const panes = ['original', 'translated'], pinned = { original: true, translated: true };
const showFollow = () => $('follow').hidden = panes.every(id => pinned[id]);
function render() {
  for (const [id, source] of [['original', true], ['translated', false]]) {
    const el = $(id);
    el.replaceChildren(...transcript.entries(source).filter(x => x.text).map(({ text, lang }) => {
      const p = Object.assign(document.createElement('p'), { textContent: text });
      if (lang) p.dataset.lang = lang.toUpperCase();
      return p;
    }));
    if (pinned[id]) el.scrollTop = el.scrollHeight;
  }
  publish();
}
for (const id of panes) $(id).addEventListener('scroll', () => {
  const el = $(id);
  pinned[id] = el.scrollHeight - el.scrollTop - el.clientHeight < 40; showFollow();
}, { passive: true });
$('follow').onclick = () => { for (const id of panes) { pinned[id] = true; $(id).scrollTop = $(id).scrollHeight; } showFollow(); };
let size = Math.max(16, Math.min(56, prefs.size || 26));
function applyView(view = prefs.view || 'both') {
  prefs.view = view; prefs.size = size; savePrefs();
  document.querySelector('.transcripts').dataset.view = view;
  document.documentElement.style.setProperty('--size', size + 'px');
  for (const b of document.querySelectorAll('.segmented [data-view]')) b.setAttribute('aria-pressed', b.dataset.view === view);
  for (const id of panes) if (pinned[id]) $(id).scrollTop = $(id).scrollHeight;
}
for (const b of document.querySelectorAll('.segmented [data-view]')) b.onclick = () => applyView(b.dataset.view);
$('smaller').onclick = () => { size = Math.max(16, size - 2); applyView(prefs.view); };
$('larger').onclick = () => { size = Math.min(56, size + 2); applyView(prefs.view); };
applyView();
$('error').onclick = () => error('');

function selectEngine(next) {
  if (next !== engine) {
    engine = next; prefs.engine = next; savePrefs();
    transcript = next === 'soniox' ? new SonioxTranscript() : new Transcript();
    render();
  }
  for (const b of document.querySelectorAll('[data-engine]')) b.setAttribute('aria-pressed', b.dataset.engine === engine);
  $('language').replaceChildren(...DIRECTIONS[engine].map(([value, label]) => new Option(label, value)));
  const saved = prefs[`language_${engine}`];
  if (DIRECTIONS[engine].some(([value]) => value === saved)) $('language').value = saved;
  $('rotateHint').textContent = engine === 'soniox' ? '30 分钟或延迟 > 6 s 时切换；上行积压 > 8 s 丢弃重连' : '5 分钟或延迟 > 8 s 时切换';
  $('engineHint').textContent = engine === 'soniox' ? 'Soniox stt-rt-v5 · 浏览器直连 · 临时密钥' : 'Qwen 3.8 LiveTranslate · WebRTC';
  $('diagnostics').textContent = IDLE_DIAG[engine];
  $('metrics').hidden = true;
}
for (const b of document.querySelectorAll('[data-engine]')) b.onclick = () => selectEngine(b.dataset.engine);
$('language').onchange = () => { prefs[`language_${engine}`] = $('language').value; savePrefs(); };

async function api(path, body, type = 'application/json', signal) {
  const response = await fetch(`./api/${path}`, { method: body === undefined ? 'GET' : 'POST', headers: { 'Content-Type': type, 'X-CSRF-Token': csrf }, body, signal });
  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `请求失败 (${response.status})`);
  }
  return type === 'application/sdp' ? response.text() : response.json();
}
function signedIn(result) {
  csrf = result?.csrf || '';
  available = { qwen: true, soniox: !!result?.engines?.soniox };
  if (engine === 'soniox' && !available.soniox) engine = 'qwen', transcript = new Transcript();
  for (const b of document.querySelectorAll('[data-engine]')) b.title = available[b.dataset.engine] ? '' : '服务器未配置该引擎';
  $('workspace').hidden = !csrf; $('logout').hidden = !csrf; $('login').hidden = !!csrf;
  if (csrf) { selectEngine(engine); initializeDevices(); }
}
$('loginForm').onsubmit = async event => {
  event.preventDefault(); error('');
  try { const result = await api('login', JSON.stringify({ password: $('password').value })); $('password').value = ''; signedIn(result); }
  catch (e) { error(e.message); }
};
$('logout').onclick = async () => { stop(); try { await api('logout', '{}'); signedIn(null); } catch (e) { error(e.message); } };
api('session').then(signedIn).catch(() => signedIn(null));

async function enumerate() {
  const selected = $('device').value;
  const devices = await navigator.mediaDevices.enumerateDevices();
  $('device').replaceChildren(new Option('默认麦克风', ''));
  for (const device of devices.filter(d => d.kind === 'audioinput')) $('device').add(new Option(device.label || `麦克风 ${$('device').length}`, device.deviceId));
  if ([...$('device').options].some(o => o.value === selected)) $('device').value = selected;
}
async function initializeDevices() {
  error(''); const ticket = ++generation;
  busy(true); status('初始化音频 · 请允许麦克风权限，正在识别设备…', 'busy');
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    stream.getTracks().forEach(t => t.stop());
    if (ticket !== generation) return;
    await enumerate();
    if (ticket === generation) status('音频设备已识别 · 麦克风已释放，可选择设备后开始');
  } catch (e) {
    if (ticket !== generation) return;
    status('麦克风尚未授权 · 可点 ↻ 重试或选择共享音频', 'error');
    error(`无法访问麦克风：${e.name}。请在浏览器站点权限中允许麦克风，再点击设备旁的 ↻。`);
  } finally {
    if (ticket === generation) { busy(false); $('source').onchange(); }
  }
}
$('devices').onclick = initializeDevices;
navigator.mediaDevices?.addEventListener('devicechange', () => enumerate().catch(() => {}));
$('source').onchange = () => { const mic = $('source').value === 'mic'; $('device').disabled = !mic; $('sourceHint').hidden = mic; };

// Soniox receives PCM from this graph, so it runs at 16 kHz to keep the uplink at 32 KB/s; Qwen uses the native rate.
function meter(stream, sampleRate) {
  audio = new AudioContext(sampleRate ? { sampleRate } : undefined);
  input = audio.createMediaStreamSource(stream);
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
const seconds = ms => ms == null ? '—' : `${(ms / 1000).toFixed(1)} s`;
function metrics({ lag, upload, age, rotations }) {
  const m = $('metrics');
  m.hidden = false;
  m.dataset.level = lag > 4000 || upload > 3000 ? 'bad' : lag > 2000 || upload > 1000 ? 'warn' : 'ok';
  const mins = Math.floor(age / 60000), secs = String(Math.floor(age / 1000) % 60).padStart(2, '0');
  m.textContent = `延迟 ${seconds(lag)}${upload == null ? '' : ` · 上行 ${seconds(upload)}`} · 会话 ${mins}:${secs} · 切换 ${rotations}`;
}
// Stop releases devices at once; open sessions still deliver their final sentence in the background.
function stop(message = '已停止 · 设备已释放；最后一句仍会补全', state = 'idle') {
  generation++;
  session?.stop(); session = null;
  capture?.getTracks().forEach(t => t.stop()); capture = null;
  cancelAnimationFrame(animation); audio?.close().catch(() => {}); audio = null; input = null; analyser = null; monitor = null;
  $('wave').getContext('2d').clearRect(0, 0, 720, 64); $('level').textContent = '静音 · −∞ dB';
  $('diagnostics').textContent = IDLE_DIAG[engine]; $('metrics').hidden = true;
  busy(false); $('source').onchange(); status(message, state);
}
$('stop').onclick = () => stop();
function fail(e, ticket) { if (ticket !== generation) return; stop('连接停止 · 请检查错误后重试', 'error'); error(e.message); }
async function start(localOnly) {
  const ticket = ++generation, alive = () => ticket === generation;
  busy(true); error(''); status('等待音频权限…', 'busy');
  try {
    const stream = $('source').value === 'display'
      ? await navigator.mediaDevices.getDisplayMedia({ video: true, audio: true })
      : await navigator.mediaDevices.getUserMedia({ audio: { deviceId: $('device').value ? { exact: $('device').value } : undefined, echoCancellation: false, noiseSuppression: false, autoGainControl: false } });
    if (!alive()) { stream.getTracks().forEach(t => t.stop()); return; }
    capture = stream;
    if (!stream.getAudioTracks().length) throw new Error('分享来源没有音轨。请重新选择标签页并勾选「分享音频」；部分系统/浏览器不支持系统音频。');
    for (const track of stream.getTracks()) track.onended = () => { if (alive()) stop('音频来源已结束 · 设备已释放'); };
    const audioOnly = new MediaStream(stream.getAudioTracks());
    meter(audioOnly, engine === 'soniox' && !localOnly ? 16000 : undefined);
    await enumerate();
    if (!alive()) return;
    if (localOnly) return status('本地试音中 · 不连接模型，不调用 API', 'test');
    const options = {
      stream: audioOnly, audio, input, api, transcript, render,
      language: $('language').value, rotate: $('reconnect').checked,
      status: (text, state) => alive() && status(text, state),
      diag: text => { if (alive()) $('diagnostics').textContent = text; },
      metrics: value => alive() && metrics(value),
      fail: e => fail(e, ticket),
    };
    const started = engine === 'soniox' ? await startSoniox(options) : startQwen(options);
    if (alive()) session = started; else started.stop();
  } catch (e) { fail(e.name === 'NotAllowedError' ? new Error('音频权限被拒绝或分享已取消。请允许麦克风/重新分享后再试。') : e, ticket); }
}
$('start').onclick = () => start(false);
$('test').onclick = () => start(true);
$('clear').onclick = () => { transcript.clear(); render(); };
$('director').onclick = () => {
  popup = window.open('./director.html', 'qwen-director', 'popup,width=1100,height=420');
  if (!popup) error('导播弹窗被浏览器拦截，请允许此网站弹窗后重试。');
  else { error(''); popup.focus(); publish(); }
};
window.addEventListener('message', event => { if (event.origin === location.origin && event.source === popup && event.data?.type === 'qwen-director-ready') publish(); });
for (const id of ['font', 'width']) $(id).oninput = () => { $(`${id}Value`).textContent = $(id).value + (id === 'font' ? ' px' : '%'); publish(); };
window.addEventListener('pagehide', () => stop());
