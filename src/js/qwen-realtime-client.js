// Qwen LiveTranslate Flash realtime client (via Tauri Rust backend).
// China 3.8 emits deltas linked by item IDs; legacy Singapore emits snapshots.

const { invoke, Channel } = window.__TAURI__.core;

export class QwenRealtimeClient {
    constructor() {
        this.sessionId = null;
        this.channel = null;
        this.isConnected = false;

        this.onStatusChange = () => {};
        this.onSegment = () => {};
        this.onProvisional = () => {};
        this.onSourceProvisional = () => {};
        this.onError = () => {};
        this.onClosed = () => {};

        this._provisionalBuffer = '';
        this._sources = new Map();
        this._targets = new Map();
        this._completed = new Set();
        this._closed = false;
    }

    async connect(cfg) {
        this.channel = new Channel();
        this.channel.onmessage = (evt) => this._handleEvent(evt);

        try {
            this.sessionId = await invoke('qwen_realtime_start', {
                config: {
                    api_key: cfg.apiKey,
                    source_language: cfg.sourceLanguage || 'en',
                    target_language: cfg.targetLanguage,
                    service: cfg.service || 'qianwen',
                },
                onEvent: this.channel,
            });
            this.isConnected = true;
        } catch (err) {
            this.onError('connect_failed', String(err));
            throw err;
        }
    }

    async sendAudio(arrayBuffer) {
        if (!this.isConnected || this.sessionId == null) return;
        const bytes = Array.from(new Uint8Array(arrayBuffer));
        try {
            await invoke('qwen_realtime_send_audio', {
                sessionId: this.sessionId,
                pcm: bytes,
            });
        } catch (err) {
            console.warn('[Qwen Realtime] send audio failed:', err);
        }
    }

    flushPending() {
        const tgt = this._provisionalBuffer;
        this._provisionalBuffer = '';
        if (tgt) this.onSegment('', tgt);
        for (const target of this._targets.values()) {
            const source = this._sources.get(target.sourceId);
            if (source?.text || target.text) this.onSegment(source?.text || '', target.text);
            this._sources.delete(target.sourceId);
        }
        for (const source of this._sources.values()) {
            if (source.text) this.onSegment(source.text, '');
        }
        this._sources.clear();
        this._targets.clear();
        this.onSourceProvisional('');
        this.onProvisional('');
    }

    async disconnect() {
        if (!this.isConnected) return;
        this.isConnected = false;
        const closed = new Promise(resolve => { this._resolveClosed = resolve; });
        const timer = setTimeout(() => this._handleEvent({ type: 'closed', reason: 'drain_timeout' }), 16000);
        try {
            await invoke('qwen_realtime_stop', { sessionId: this.sessionId });
            await closed;
        } catch {}
        finally {
            clearTimeout(timer);
            this._handleEvent({ type: 'closed', reason: 'stopped' });
        }
    }

    _handleEvent(evt) {
        if (this._closed) return;
        switch (evt.type) {
            case 'realtime':
                this._handleRealtime(evt.event);
                break;
            case 'status':
                this.onStatusChange(evt.state, evt.message);
                break;
            case 'transcript':
                if (evt.is_final) {
                    this._provisionalBuffer = '';
                    // Live Flash exposes only translation — no source side.
                    this.onSegment('', evt.text);
                } else {
                    // Full snapshot (committed + stash). Assign, don't concat.
                    this._provisionalBuffer = evt.text;
                    this.onProvisional(this._provisionalBuffer);
                }
                break;
            case 'error':
                this.onError(evt.code, evt.message);
                break;
            case 'closed':
                this._closed = true;
                this.isConnected = false;
                this.flushPending();
                this._resolveClosed?.();
                this.onClosed(evt.reason);
                break;
        }
    }

    _handleRealtime(event) {
        const sourceEvent = ['conversation.item.input_audio_transcription.delta',
            'conversation.item.input_audio_transcription.completed'].includes(event.type);
        const targetEvent = event.type === 'response.text.delta' || event.type === 'response.text.done';
        // China also labels the input-audio item as assistant; it is not a translation.
        const linkEvent = event.type === 'conversation.item.created' && event.previous_item_id
            && event.item?.role === 'assistant'
            && !event.item.content?.some(part => part.type === 'input_audio');
        if (!sourceEvent && !targetEvent && !linkEvent) return;
        const id = linkEvent ? event.item.id : event.item_id;
        if (!id || this._completed.has(id)) return;
        const map = sourceEvent ? this._sources : this._targets;
        const item = map.get(id) || { text: '', final: false };
        map.set(id, item);
        if (linkEvent) item.sourceId = event.previous_item_id;
        else if (event.type.endsWith('.delta')) {
            if (!item.final) item.text += event.delta || '';
        } else {
            item.text = event.transcript ?? event.text ?? item.text;
            item.final = true;
        }

        for (const [targetId, target] of this._targets) {
            const source = this._sources.get(target.sourceId);
            if (!target.final || !source?.final) continue;
            this.onSegment(source.text, target.text);
            this._sources.delete(target.sourceId);
            this._targets.delete(targetId);
            this._completed.add(target.sourceId);
            this._completed.add(targetId);
        }
        // Remember recent finals to ignore duplicate server events without growing forever.
        while (this._completed.size > 256) this._completed.delete(this._completed.values().next().value);
        this.onSourceProvisional([...this._sources.values()].map(item => item.text).join(''));
        this.onProvisional([...this._targets.values()].map(item => item.text).join(''));
    }
}
