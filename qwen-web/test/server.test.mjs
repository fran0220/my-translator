import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from '../server.mjs';
import { Transcript } from '../public/transcript.js';

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
