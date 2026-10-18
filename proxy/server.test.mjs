import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import WebSocket, { WebSocketServer } from 'ws';
import { createRelay } from './server.js';
import { getRealtimeProxy } from '../src/js/realtime-proxy.js';

const token = 'test-only-team-token-0123456789abcdef';

async function setup(t) {
    const provider = new WebSocketServer({port: 0, host: '127.0.0.1'});
    await once(provider, 'listening');
    provider.on('connection', (ws, req) => {
        ws.on('error', () => {});
        ws.send(JSON.stringify({path: req.url, headers: req.headers}));
        ws.on('message', (data, binary) => {
            if (!binary && data.toString() === 'abort') return ws.terminate();
            ws.send(data, {binary});
            if (!binary && data.toString() === 'final') ws.close(1000, 'done');
        });
    });
    const relay = createRelay({token, connectUpstream: (url, options) => {
        const target = new URL(url);
        assert.ok(['stt-rt.soniox.com', 'api.openai.com'].includes(target.hostname));
        return new WebSocket(`ws://127.0.0.1:${provider.address().port}${target.pathname}${target.search}`, options);
    }});
    relay.listen(0, '127.0.0.1');
    await once(relay, 'listening');
    t.after(() => { relay.close(); for (const ws of provider.clients) ws.terminate(); provider.close(); });
    return `ws://127.0.0.1:${relay.address().port}`;
}

function client(url, protocols, options) {
    const ws = new WebSocket(url, protocols, options);
    const frames = [], waiters = [];
    ws.on('message', (data, binary) => {
        const frame = {data, binary};
        if (waiters.length) waiters.shift()(frame); else frames.push(frame);
    });
    return {ws, next: () => frames.length ? Promise.resolve(frames.shift()) : new Promise(r => waiters.push(r))};
}

test('authentication and fixed routes reject requests without opening a relay', {timeout: 5000}, async t => {
    const base = await setup(t);
    for (const [path, headers, status] of [
        ['/soniox', {}, 401],
        ['/openai', {'X-Translator-Token': 'x'.repeat(32)}, 401],
        ['/openai', {'X-Translator-Token': token}, 401],
        ['/arbitrary-host', {}, 404],
        ['/soniox?target=wss://evil.example', {}, 404],
    ]) {
        const ws = new WebSocket(base + path, {headers});
        ws.on('error', () => {});
        const code = await new Promise(resolve => ws.once('unexpected-response', (_req, res) => {
            res.resume(); resolve(res.statusCode); ws.terminate();
        }));
        assert.equal(code, status);
    }
    assert.equal(await (await fetch(base.replace('ws:', 'http:') + '/healthz')).text(), 'ok');
    assert.throws(() => createRelay({token: ''}));
});

test('both routes preserve binary audio, incremental text, empty final frames and tail-before-close', {timeout: 5000}, async t => {
    const base = await setup(t);
    for (const provider of ['soniox', 'openai']) {
        const {ws, next} = client(base + '/' + provider,
            provider === 'soniox' ? ['translator-v1', `auth.${token}`] : [],
            {headers: {Cookie: 'private-cookie', Authorization: 'Bearer provider-test-key', 'X-Translator-Token': token}});
        await once(ws, 'open');
        assert.equal(ws.protocol, provider === 'soniox' ? 'translator-v1' : '');
        const metadata = JSON.parse((await next()).data.toString());
        assert.equal(metadata.path, provider === 'soniox' ? '/transcribe-websocket' : '/v1/realtime/translations?model=gpt-realtime-translate');
        assert.equal(metadata.headers.authorization, provider === 'openai' ? 'Bearer provider-test-key' : undefined);
        for (const header of ['cookie', 'x-translator-token', 'sec-websocket-protocol']) assert.equal(metadata.headers[header], undefined);
        for (const value of ['增量字幕', Buffer.from([0, 127, 255, 23]), '']) {
            ws.send(value);
            const frame = await next();
            assert.equal(frame.binary, typeof value !== 'string');
            assert.deepEqual(frame.data, Buffer.from(value));
        }
        const closed = once(ws, 'close');
        ws.send('final');
        assert.equal((await next()).data.toString(), 'final');
        assert.equal((await closed)[0], 1000);
    }
});

test('upstream loss closes client and allows a fresh connection', {timeout: 5000}, async t => {
    const base = await setup(t);
    for (let attempt = 0; attempt < 2; attempt++) {
        const {ws, next} = client(base + '/soniox', ['translator-v1', `auth.${token}`]);
        await once(ws, 'open');
        await next();
        const closed = once(ws, 'close');
        ws.send('abort');
        assert.equal((await closed)[0], 1011);
    }
});

test('client relay validation keeps direct mode explicit and rejects credential-bearing addresses', () => {
    assert.equal(getRealtimeProxy('', token), null);
    assert.deepEqual(getRealtimeProxy('https://relay.test:8443/', token), {url: 'wss://relay.test:8443/', token});
    for (const address of ['http://relay.test', 'ws://relay.test', 'https://user:pass@relay.test',
        'https://relay.test/openai', 'https://relay.test/?key=private', 'https://relay.test/#secret']) {
        assert.throws(() => getRealtimeProxy(address, token));
    }
    for (const invalid of ['', 'x'.repeat(31), 'x'.repeat(129), 'bad token'.repeat(8)]) {
        assert.throws(() => getRealtimeProxy('https://relay.test', invalid));
    }
});
