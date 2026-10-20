const frame = document.querySelector('#frame'), text = document.querySelector('#text');
// CSS pixel line-height keeps the clipping boundary exactly on complete visual lines.
let state = { text: '', font: 48, width: 92 };
function render() {
  const font = Math.min(state.font, innerHeight * .88 / 1.35);
  const line = font * 1.35;
  const lines = Math.max(1, Math.min(3, Math.floor(innerHeight * .88 / line)));
  Object.assign(document.body.style, { background: '#000', color: '#fff' });
  Object.assign(frame.style, { position: 'absolute', left: `${(100 - state.width) / 2}%`, bottom: '6%', width: `${state.width}%`, height: `${line * lines}px`, overflow: 'hidden' });
  Object.assign(text.style, { position: 'absolute', bottom: '0', width: '100%', fontSize: `${font}px`, lineHeight: `${line}px`, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', textAlign: 'center' });
  text.textContent = state.text;
}
window.addEventListener('message', event => {
  if (event.origin !== location.origin || event.source !== window.opener || event.data?.type !== 'qwen-director') return;
  state = { text: String(event.data.text || ''), font: Math.max(20, Math.min(120, Number(event.data.font) || 48)), width: Math.max(35, Math.min(100, Number(event.data.width) || 92)) };
  render();
});
window.opener?.postMessage({ type: 'qwen-director-ready' }, location.origin);
window.addEventListener('resize', render);
render();
