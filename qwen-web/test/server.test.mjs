import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from '../server.mjs';
import { parseGlossary, qwenPhrases, sonioxContext } from '../public/glossary.js';
import { Transcript, SonioxTranscript } from '../public/transcript.js';

test('authentication, CSRF, fixed upstream, body limits and SDP quotas', async t => {
  let calls = 0;
  const server = createServer({ key: 'test-only-secret', password: '0000', origin: 'https://test.example', fetcher: async (url, options) => {
    calls++;
    assert.equal(url, 'https://maas.qianwenaiapi.com/api/v1/webrtc/realtime?model=qwen3.8-livetranslate-flash-realtime');
    assert.equal(options.headers.Authorization, 'Bearer test-only-secret');
    return new Response('v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n');
  } });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  t.after(() => server.close());
  const url = `http://127.0.0.1:${server.address().port}/qwen/api/`;
  const request = (path, body, headers = {}) => fetch(url + path, { method: 'POST', headers: { Origin: 'https://test.example', ...headers }, body });
  assert.equal((await request('sdp', 'v=0')).status, 401);
  assert.equal((await request('login', '{"password":"0000"}', { Origin: 'https://evil.example' })).status, 403);
  assert.equal((await request('login', '{"password":"wrong"}')).status, 401);
  const login = await request('login', '{"password":"0000"}');
  const cookie = login.headers.get('set-cookie');
  for (const flag of ['HttpOnly', 'Secure', 'SameSite=Strict']) assert.ok(cookie.includes(flag));
  const { csrf } = await login.json();
  const headers = { Cookie: cookie.split(';')[0], 'X-CSRF-Token': csrf, 'Content-Type': 'application/sdp' };
  assert.equal((await request('sdp', 'v=0', { Cookie: headers.Cookie })).status, 401);
  assert.equal((await request('sdp', 'v=0', headers)).status, 400);
  assert.equal((await request('sdp', 'v=0\r\nm=audio ' + 'x'.repeat(66000), headers)).status, 413);
  for (let i = 0; i < 6; i++) assert.equal((await request('sdp', 'v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n', headers)).status, 200);
  assert.equal((await request('sdp', 'v=0\r\nm=audio 9', headers)).status, 429);
  assert.equal(calls, 6);
  assert.equal((await request('logout', '{}', headers)).status, 200);
  assert.equal((await request('sdp', 'v=0', headers)).status, 401);
  for (let i = 0; i < 8; i++) assert.equal((await request('login', '{"password":"wrong"}')).status, 401);
  assert.equal((await request('login', '{"password":"0000"}')).status, 429);
});
test('incremental source/translation preserve separate items, deduplicate and finalize', () => {
  const t = new Transcript();
  const first = { type: 'response.text.delta', event_id: 'one', item_id: 'translated', delta: '你好' };
  t.accept(first); t.accept(first);
  t.accept({ ...first, event_id: 'two', delta: '世界' });
  t.accept({ type: 'conversation.item.input_audio_transcription.delta', item_id: 'source', delta: 'Hello world' });
  t.accept({ type: 'response.text.done', item_id: 'translated', text: '你好，世界。' });
  t.accept({ ...first, event_id: 'three', item_id: 'next', delta: '再见' });
  assert.equal(t.text(false), '你好，世界。\n再见');
  assert.equal(t.text(true), 'Hello world');
});

test('Soniox temporary keys: authenticated only, long-lived key stays server-side, quota enforced', async t => {
  const bodies = [];
  const server = createServer({ key: 'qwen-secret', sonioxKey: 'soniox-secret', password: '0000', origin: 'https://test.example', fetcher: async (url, options) => {
    assert.equal(url, 'https://api.soniox.com/v1/auth/temporary-api-key');
    assert.equal(options.headers.Authorization, 'Bearer soniox-secret');
    bodies.push(JSON.parse(options.body));
    return Response.json({ api_key: 'snx_temp_abc', expires_at: '2026-01-01T00:00:00Z' }, { status: 201 });
  } });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  t.after(() => server.close());
  const url = `http://127.0.0.1:${server.address().port}/qwen/api/`;
  const request = (path, body, headers = {}) => fetch(url + path, { method: 'POST', headers: { Origin: 'https://test.example', ...headers }, body });
  assert.equal((await request('soniox-key', '{}')).status, 401);
  const login = await request('login', '{"password":"0000"}');
  const { csrf, engines } = await login.json();
  assert.deepEqual(engines, { qwen: true, soniox: true });
  const headers = { Cookie: login.headers.get('set-cookie').split(';')[0], 'X-CSRF-Token': csrf };
  const minted = await request('soniox-key', '{}', headers);
  const text = await minted.text();
  assert.equal(minted.status, 200);
  assert.equal(JSON.parse(text).api_key, 'snx_temp_abc');
  assert.ok(!text.includes('soniox-secret'));
  assert.equal(bodies[0].usage_type, 'transcribe_websocket');
  assert.equal(bodies[0].single_use, true);
  assert.ok(bodies[0].expires_in_seconds <= 60);
  for (let i = 1; i < 12; i++) assert.equal((await request('soniox-key', '{}', headers)).status, 200);
  assert.equal((await request('soniox-key', '{}', headers)).status, 429);
  const page = await fetch(url.replace('api/', ''));
  assert.match(page.headers.get('content-security-policy'), /connect-src 'self' wss:\/\/stt-rt\.soniox\.com;/);
});
test('Soniox engine is reported unavailable without a server key', async t => {
  const server = createServer({ key: 'qwen-secret', sonioxKey: '', password: '0000', origin: 'https://test.example', fetcher: async () => assert.fail('must not call upstream') });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  t.after(() => server.close());
  const url = `http://127.0.0.1:${server.address().port}/qwen/api/`;
  const login = await fetch(url + 'login', { method: 'POST', headers: { Origin: 'https://test.example' }, body: '{"password":"0000"}' });
  const { csrf, engines } = await login.json();
  assert.equal(engines.soniox, false);
  const res = await fetch(url + 'soniox-key', { method: 'POST', headers: { Origin: 'https://test.example', Cookie: login.headers.get('set-cookie').split(';')[0], 'X-CSRF-Token': csrf }, body: '{}' });
  assert.equal(res.status, 503);
});
test('Soniox utterances: endpoint split, late translation, provisional replacement, interleaved sockets, clear', () => {
  const t = new SonioxTranscript();
  const tok = (text, extra = {}) => ({ text, is_final: true, translation_status: 'original', language: 'en', ...extra });
  const a = t.cursor(), b = t.cursor();
  t.accept(a, { tokens: [tok('Hello'), tok(' there', { is_final: false }), tok('你好', { translation_status: 'translation', is_final: false, language: 'zh' })] });
  assert.deepEqual(t.parts(true), ['Hello there']);
  assert.deepEqual(t.parts(false), ['你好']);
  // Non-final tokens are replaced by the next response, not appended.
  t.accept(a, { tokens: [tok(' there.'), tok('<end>'), tok('你好。', { translation_status: 'translation', language: 'zh' })] });
  assert.deepEqual(t.parts(true), ['Hello there.']);
  assert.deepEqual(t.parts(false), ['你好。']);
  // The replacement socket opens a new utterance while the old one is still draining its translation.
  t.accept(b, { tokens: [tok('我们开始吧', { language: 'zh' })] });
  t.accept(a, { tokens: [tok('！', { translation_status: 'translation', language: 'zh' })] });
  t.accept(b, { tokens: [tok("Let's begin", { translation_status: 'translation' })] });
  assert.deepEqual(t.parts(true), ['Hello there.', '我们开始吧']);
  assert.deepEqual(t.parts(false), ['你好。！', "Let's begin"]);
  assert.deepEqual(t.entries(true).map(x => x.lang), ['en', 'zh']);
  assert.equal(t.text(false), "你好。！\nLet's begin");
  assert.match(t.recent(6), /我们开始吧$/);
  t.accept(b, { tokens: [tok('<end>'), tok('Good evening.'), tok(' 大家晚上好。', { translation_status: 'translation' }), tok(' 今天 见', { translation_status: 'translation' })] });
  assert.equal(t.parts(false).at(-1), '大家晚上好。今天见');
  assert.equal(t.parts(true).at(-1), 'Good evening.');
  t.clear();
  t.accept(a, { tokens: [tok('late', { translation_status: 'translation' })] });
  t.accept(b, { tokens: [tok('Next')] });
  assert.deepEqual(t.parts(true), ['Next']);
  assert.deepEqual(t.parts(false), ['late']);
});

test('glossary orients each pair by target language for Qwen and Soniox', () => {
  const pairs = parseGlossary('亦庄 = E-town\nAI＝人工智能\n\nnot a pair\n大兴 -> Daxing');
  assert.deepEqual(pairs, [['亦庄', 'E-town'], ['AI', '人工智能'], ['大兴', 'Daxing']]);
  assert.deepEqual(qwenPhrases(pairs, 'en'), { 亦庄: 'E-town', 人工智能: 'AI', 大兴: 'Daxing' });
  assert.deepEqual(qwenPhrases(pairs, 'zh'), { 'E-town': '亦庄', AI: '人工智能', Daxing: '大兴' });
  assert.equal(qwenPhrases([], 'zh'), null);
  const ctx = sonioxContext([['亦庄', 'E-town']], 'two_way');
  assert.deepEqual(ctx.terms, ['亦庄', 'E-town']);
  assert.deepEqual(ctx.translation_terms, [{ source: 'E-town', target: '亦庄' }, { source: '亦庄', target: 'E-town' }]);
  assert.deepEqual(sonioxContext([['亦庄', 'E-town']], 'en').translation_terms, [{ source: '亦庄', target: 'E-town' }]);
});
