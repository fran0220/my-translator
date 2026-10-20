// Qwen: events keyed by item id; source (ASR) and translation items are kept apart.
export class Transcript {
  constructor() { this.clear(); }
  clear() { this.items = new Map(); this.seen = new Set(); }
  accept(event) {
    if (event.event_id && this.seen.has(event.event_id)) return false;
    if (event.event_id) { this.seen.add(event.event_id); if (this.seen.size > 4000) this.seen.delete(this.seen.values().next().value); }
    const source = event.type?.startsWith('conversation.item.input_audio_transcription.');
    const target = event.type?.startsWith('response.text.');
    if (!source && !target) return false;
    const suffix = event.type.split('.').at(-1);
    if (!['delta', 'text', 'done', 'completed'].includes(suffix)) return false;
    const id = `${source ? 's' : 't'}:${event.item_id || event.response_id}:${event.content_index || 0}`;
    const old = this.items.get(id)?.text || '';
    const text = suffix === 'delta' ? old + (event.delta || '') : suffix === 'text' ? (event.text || '') + (event.stash || '') : (event.text ?? event.transcript ?? old);
    this.items.set(id, { source, text: text.slice(-20000) });
    if (this.items.size > 300) this.items.delete(this.items.keys().next().value);
    return true;
  }
  entries(source) { return [...this.items.values()].filter(x => x.source === source).map(x => ({ text: x.text })); }
  parts(source) { return this.entries(source).map(x => x.text); }
  text(source) { return this.parts(source).join('\n'); }
}

// Soniox streams Chinese translation in chunks joined by spaces; drop spaces between CJK characters/punctuation only.
const CJK = '\\u3000-\\u303f\\u3400-\\u9fff\\uff00-\\uffef';
const cjkSpace = new RegExp(`(?<=[${CJK}])\\s+(?=[${CJK}])`, 'g');

// Soniox: one segment per utterance (closed by the <end> endpoint token) holding its spoken text and translation.
// Every WebSocket owns a cursor, so a draining old connection and its replacement never write into each other's segments.
export class SonioxTranscript {
  constructor() { this.epoch = 0; this.clear(); }
  clear() { this.segments = []; this.epoch++; }
  cursor() { return { seg: null, ended: true, live: new Set(), epoch: this.epoch }; }
  #add() {
    const seg = { s: '', sp: '', t: '', tp: '', lang: '' };
    this.segments.push(seg);
    if (this.segments.length > 300) this.segments.shift();
    return seg;
  }
  accept(cur, response) {
    if (cur.epoch !== this.epoch) Object.assign(cur, this.cursor());
    for (const seg of cur.live) seg.sp = seg.tp = '';
    cur.live.clear();
    let changed = false;
    for (const token of response.tokens || []) {
      if (token.text === '<end>' || token.text === '<fin>') { if (token.is_final) cur.ended = true; continue; }
      if (!token.text) continue;
      const translation = token.translation_status === 'translation';
      // A new utterance starts with its first spoken token; late translation tokens still belong to the previous one.
      if (!cur.seg || (!translation && cur.ended)) { cur.seg = this.#add(); cur.ended = false; }
      const seg = cur.seg;
      if (!translation && !seg.lang && token.language) seg.lang = token.language;
      if (token.is_final) seg[translation ? 't' : 's'] += token.text;
      else { seg[translation ? 'tp' : 'sp'] += token.text; cur.live.add(seg); }
      changed = true;
    }
    return changed;
  }
  entries(source) {
    return this.segments.map(s => ({ text: (source ? s.s + s.sp : s.t + s.tp).replace(cjkSpace, '').trim(), lang: source ? s.lang : '' })).filter(x => x.text);
  }
  parts(source) { return this.entries(source).map(x => x.text); }
  text(source) { return this.parts(source).join('\n'); }
  recent(chars) { return this.segments.map(s => s.s).join(' ').replace(/\s+/g, ' ').trim().slice(-chars); }
}
