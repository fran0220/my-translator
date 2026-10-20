export class Transcript {
  constructor() { this.items = new Map(); this.seen = new Set(); }
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
  parts(source) { return [...this.items.values()].filter(x => x.source === source).map(x => x.text); }
  text(source) { return this.parts(source).join('\n'); }
}
