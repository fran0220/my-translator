import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

let stopEvents = () => {};
const audioInvocations = [];
globalThis.window = {
    __TAURI__: { core: {
        Channel: class {},
        invoke: async (command, args) => {
            if (command === 'qwen_realtime_send_audio') audioInvocations.push(args);
            if (command.endsWith('_stop')) stopEvents();
            return 1;
        },
    } },
    addEventListener() {}, removeEventListener() {},
};
globalThis.document = { getElementById: () => null, addEventListener() {}, removeEventListener() {} };
const { QwenRealtimeClient } = await import('../src/js/qwen-realtime-client.js');
const { OpenAiRealtimeClient } = await import('../src/js/openai-realtime-client.js');
const { SonioxClient } = await import('../src/js/soniox.js');
const { TranscriptUI } = await import('../src/js/ui.js');
const { initBroadcastOutput } = await import('../src/js/broadcast-output.js');

test('Qwen displays deltas immediately, corrects finals and pairs by item ID, not arrival order', async () => {
    const client = new QwenRealtimeClient();
    let source = '', target = '';
    const segments = [];
    client.onSourceProvisional = text => { source = text; };
    client.onProvisional = text => { target = text; };
    client.onSegment = (...pair) => segments.push(pair);
    await client.connect({ apiKey: 'placeholder', targetLanguage: 'zh' });
    const emit = event => client.channel.onmessage({ type: 'realtime', event });
    emit({ type: 'response.text.delta', item_id: 't2', delta: '二' });
    assert.equal(target, '二');
    assert.equal(source, '');
    emit({ type: 'conversation.item.input_audio_transcription.delta', item_id: 's1', delta: 'won' });
    assert.equal(source, 'won');
    emit({ type: 'conversation.item.input_audio_transcription.completed', item_id: 's1', transcript: 'one' });
    emit({ type: 'conversation.item.input_audio_transcription.completed', item_id: 's2', transcript: 'two' });
    emit({ type: 'response.text.done', item_id: 't2', text: '第二' });
    assert.equal(segments.length, 0);
    emit({ type: 'conversation.item.created', item: { id: 't2', role: 'assistant' }, previous_item_id: 's2' });
    assert.deepEqual(segments, [['two', '第二']]);
    assert.equal(source, 'one');
    assert.equal(target, '');
    emit({ type: 'conversation.item.created', item: { id: 't1', role: 'assistant' }, previous_item_id: 's1' });
    emit({ type: 'response.text.done', item_id: 't1', text: '第一' });
    emit({ type: 'response.text.done', item_id: 't1', text: '第一' });
    client.channel.onmessage({ type: 'closed' });
    assert.deepEqual(segments, [['two', '第二'], ['one', '第一']]);
    assert.equal(source + target, '');
});

test('Qwen restart preserves displayed text, fences captured generations and cancels late callbacks', async () => {
    const client = new QwenRealtimeClient();
    const segments = [];
    client.onSegment = (...pair) => segments.push(pair);
    await client.connect({ apiKey: 'placeholder', targetLanguage: 'zh' });
    const oldChannel = client.channel;
    oldChannel.onmessage({ type: 'realtime', event: { type: 'response.text.delta', item_id: 'same', delta: '旧字幕' } });
    oldChannel.onmessage({ type: 'restarted', generation: 1 });
    assert.deepEqual(segments, [['', '旧字幕']]);
    audioInvocations.length = 0;
    await client.sendAudio(new Uint8Array([1, 2]).buffer, 0);
    await client.sendAudio(new Uint8Array([7, 8]).buffer, 1);
    assert.deepEqual(audioInvocations.map(call => [call.pcm, call.generation]), [[[7, 8], 1]]);
    oldChannel.onmessage({ type: 'realtime', event: { type: 'response.text.delta', item_id: 'same', delta: '新字幕' } });
    stopEvents = () => oldChannel.onmessage({ type: 'realtime', event: { type: 'response.text.done', item_id: 'same', text: '迟到结果' } });
    await client.disconnect();
    assert.deepEqual(segments, [['', '旧字幕'], ['', '新字幕']]);
    await client.connect({ apiKey: 'placeholder', targetLanguage: 'zh' });
    oldChannel.onmessage({ type: 'transcript', text: '旧通道', is_final: true });
    assert.equal(segments.length, 2);
    stopEvents = () => {};
});

test('legacy Qwen snapshots replace rather than append', async () => {
    const client = new QwenRealtimeClient();
    let target = '';
    const segments = [];
    client.onProvisional = text => { target = text; };
    client.onSegment = (...pair) => segments.push(pair);
    await client.connect({ apiKey: 'placeholder', service: 'dashscope_intl', targetLanguage: 'en' });
    client.channel.onmessage({ type: 'transcript', text: 'hel', is_final: false });
    client.channel.onmessage({ type: 'transcript', text: 'hello', is_final: false });
    assert.equal(target, 'hello');
    client.channel.onmessage({ type: 'transcript', text: 'Hello!', is_final: true });
    client.channel.onmessage({ type: 'closed' });
    assert.deepEqual(segments, [['', 'Hello!']]);
});

test('OpenAI append-only captions remain live without done; stop drains trailing text exactly once', async () => {
    const client = new OpenAiRealtimeClient();
    let source = '', target = '', closes = 0;
    const segments = [];
    client.onSourceProvisional = text => { source = text; };
    client.onProvisional = text => { target = text; };
    client.onSegment = (...pair) => segments.push(pair);
    client.onClosed = () => { closes++; };
    await client.connect({ apiKey: 'placeholder', targetLanguage: 'en' });
    const emit = (type, text) => client.channel.onmessage({ type, text, is_final: false });
    emit('transcript', 'Keep');
    emit('transcript', ' camera two');
    assert.equal(target, 'Keep camera two');
    assert.equal(source, '');
    emit('source_transcript', '保留二号机位');
    assert.equal(source, '保留二号机位');
    assert.equal(segments.length, 0);
    stopEvents = () => {
        emit('transcript', '.');
        client.channel.onmessage({ type: 'closed' });
        client.channel.onmessage({ type: 'closed' });
    };
    await client.disconnect();
    assert.deepEqual(segments, [['保留二号机位', 'Keep camera two.']]);
    assert.equal(source + target, '');
    assert.equal(closes, 1);
});

test('Soniox replaces target hypotheses and removes them when finalized', () => {
    let socket;
    globalThis.WebSocket = class { constructor() { socket = this; } };
    const client = new SonioxClient();
    let live = '', final = '';
    client.onTranslationProvisional = text => { live = text; };
    client.onTranslation = text => { final += text; };
    client.connect({ apiKey: 'placeholder' });
    const emit = (text, is_final) => socket.onmessage({ data: JSON.stringify({ tokens: [
        { text, is_final, translation_status: 'translation' },
    ] }) });
    emit('seat two', false);
    assert.equal(live, 'seat two');
    emit('camera two', false);
    assert.equal(live, 'camera two');
    emit('camera two', true);
    assert.equal(final, 'camera two');
    assert.equal(live, '');
});

test('Soniox rotation and manual disconnect terminate with empty text, not empty audio', t => {
    t.mock.timers.enable({ apis: ['setTimeout', 'setInterval'] });
    const sockets = [];
    const previous = globalThis.WebSocket;
    globalThis.WebSocket = class {
        static OPEN = 1;
        readyState = 1;
        frames = [];
        constructor() { sockets.push(this); }
        send(frame) { this.frames.push(frame); }
        close() { this.readyState = 3; }
    };
    const client = new SonioxClient();
    try {
        client.connect({ apiKey: 'placeholder' });
        sockets[0].onopen();
        const pcm = new Uint8Array([1, 2]).buffer;
        client.sendAudio(pcm);
        assert.equal(sockets[0].frames.at(-1), pcm);
        t.mock.timers.tick(180000);
        assert.equal(sockets.length, 2);
        sockets[1].onopen();
        assert.equal(sockets[0].frames.at(-1), '');
        client.disconnect();
        assert.equal(sockets[1].frames.at(-1), '');
    } finally {
        client.disconnect();
        globalThis.WebSocket = previous;
    }
});

test('broadcast includes pending text on the correct side, replaces it with finals, and clears every stream', async () => {
    const messages = [];
    globalThis.BroadcastChannel = class {
        postMessage(message) { messages.push(message); }
        close() {}
    };
    const ui = new TranscriptUI({ innerHTML: '' });
    const dispose = initBroadcastOutput(ui);
    const tick = () => new Promise(resolve => setTimeout(resolve, 120));
    try {
        for (const provider of ['soniox', 'qwen', 'openai', 'qwen-legacy']) {
            ui.provider = provider;
            ui.provisionalText = provider === 'soniox' ? 'camera' : '机位';
            ui.sourceProvisionalText = provider === 'qwen-legacy' ? '' : 'camera';
            ui.translationProvisionalText = provider === 'soniox' ? '机位' : '';
            await tick();
            assert.deepEqual(messages.at(-1), {
                type: 'snapshot', source: provider === 'qwen-legacy' ? '' : 'camera', translation: '机位',
            });
            ui.clear();
        }
        ui.segments = [{ status: 'translated', original: 'camera', translation: '机位' }];
        await tick();
        assert.equal(messages.at(-1).translation, '机位');
        ui.clear();
        await tick();
        assert.deepEqual(messages.at(-1), { type: 'snapshot', source: '', translation: '' });
    } finally { dispose(); }
});

test('broadcast window renders only incremental translation, never source, and clears on silence', () => {
    const elements = { source: { textContent: '' }, translation: { textContent: '' } };
    let channel, heartbeat;
    let now = 1000;
    runInNewContext(readFileSync(new URL('../src/js/broadcast.js', import.meta.url), 'utf8'), {
        document: { getElementById: id => elements[id], addEventListener() {} },
        window: { addEventListener() {} },
        Date: { now: () => now },
        setInterval: callback => { heartbeat = callback; },
        BroadcastChannel: class { constructor() { channel = this; } postMessage() {} },
    });
    const send = translation => channel.onmessage({ data: { type: 'snapshot', source: 'Do not show this original.', translation } });
    send('请保留');
    assert.equal(elements.translation.textContent, '请保留');
    send('请保留二号机位。');
    assert.equal(elements.translation.textContent, '请保留二号机位。');
    assert.equal(elements.source.textContent, '');
    send('');
    assert.equal(elements.translation.textContent, '');
    assert.equal(elements.source.textContent, '');
    send('Keep camera two.');
    channel.onmessage({ data: { type: 'clear' } });
    assert.equal(elements.translation.textContent, '');
    send('Keep camera two.');
    now += 5001;
    heartbeat();
    assert.equal(elements.translation.textContent, '');
});
