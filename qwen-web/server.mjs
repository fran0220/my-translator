import http from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const upstream = 'https://maas.qianwenaiapi.com/api/v1/webrtc/realtime?model=qwen3.8-livetranslate-flash-realtime';
export function createServer({ key = process.env.DASHSCOPE_API_KEY, password = process.env.QWEN_PASSWORD, origin = process.env.PUBLIC_ORIGIN, fetcher = fetch } = {}) {
  if (!key || !password || !origin) throw new Error('Required server environment is missing');
  const sessions = new Map(), limits = new Map();
  let pending = 0;
  const limit = (id, max, window) => {
    const now = Date.now();
    let entry = limits.get(id);
    if (!entry || entry.until < now) limits.set(id, entry = { count: 0, until: now + window });
    return ++entry.count <= max;
  };
  const cleanup = setInterval(() => {
    const now = Date.now();
    for (const [id, value] of sessions) if (value.until < now) sessions.delete(id);
    for (const [id, value] of limits) if (value.until < now) limits.delete(id);
  }, 60000).unref();
  const server = http.createServer(async (req, res) => {
    const send = (code, body, type = 'application/json') => {
      res.writeHead(code, { 'Content-Type': type, 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'no-referrer', 'Content-Security-Policy': "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; media-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'" });
      res.end(type === 'application/json' ? JSON.stringify(body) : body);
    };
    try {
      const path = new URL(req.url, origin).pathname;
      if (req.method === 'GET' && path === '/qwen/healthz') return send(200, { ok: true });
      const id = req.headers.cookie?.match(/(?:^|;\s*)qwen_session=([a-f0-9]{64})(?:;|$)/)?.[1];
      const session = sessions.get(id);
      const authed = session && session.until > Date.now();
      const ip = req.headers['x-real-ip'] || req.socket.remoteAddress;
      if (req.method === 'POST') {
        if (req.headers.origin !== origin) return send(403, { error: 'Origin rejected' });
        if (path !== '/qwen/api/login' && (!authed || req.headers['x-csrf-token'] !== session.csrf)) return send(401, { error: '请重新登录' });
        if (path === '/qwen/api/login' && (!limit(`login:${ip}`, 10, 60000) || !limit('login:global', 100, 60000))) return send(429, { error: '登录过于频繁，请稍后再试' });
        if (path === '/qwen/api/sdp' && (!limit(`sdp:${id}`, 8, 60000) || !limit(`sdp-ip:${ip}`, 16, 60000) || !limit('sdp:global', 60, 3600000) || pending >= 4)) return send(429, { error: '连接配额已达上限，请稍后重试' });
        const max = path === '/qwen/api/sdp' ? 65536 : 1024;
        let body = '', size = 0;
        for await (const chunk of req) {
          size += chunk.length;
          if (size > max) { send(413, { error: 'Request too large' }); return; }
          body += chunk;
        }
        if (path === '/qwen/api/login') {
          let value;
          try { value = JSON.parse(body).password; } catch { return send(400, { error: 'Invalid JSON' }); }
          const a = Buffer.from(typeof value === 'string' ? value : ''), b = Buffer.from(password);
          if (a.length !== b.length || !timingSafeEqual(a, b)) return send(401, { error: '密码错误' });
          if (id) sessions.delete(id);
          const token = randomBytes(32).toString('hex'), csrf = randomBytes(32).toString('hex');
          sessions.set(token, { csrf, until: Date.now() + 8 * 3600000 });
          res.setHeader('Set-Cookie', `qwen_session=${token}; Path=/qwen/; HttpOnly; Secure; SameSite=Strict; Max-Age=28800`);
          return send(200, { csrf });
        }
        if (path === '/qwen/api/logout') {
          sessions.delete(id);
          res.setHeader('Set-Cookie', 'qwen_session=; Path=/qwen/; HttpOnly; Secure; SameSite=Strict; Max-Age=0');
          return send(200, { ok: true });
        }
        if (path === '/qwen/api/sdp') {
          if (req.headers['content-type'] !== 'application/sdp' || !body.startsWith('v=0') || !/\nm=audio /.test(body)) return send(400, { error: 'Invalid audio SDP' });
          pending++;
          try {
            const response = await fetcher(upstream, { method: 'POST', headers: { Authorization: `Bearer ${key}`, 'Content-Type': 'application/sdp' }, body, signal: AbortSignal.timeout(20000), redirect: 'error' });
            if (!response.ok) { await response.body?.cancel(); return send(502, { error: `Qwen SDP 请求失败 (HTTP ${response.status})` }); }
            const answer = await response.text();
            if (answer.length > 262144 || !answer.startsWith('v=0')) return send(502, { error: 'Qwen 返回无效 SDP' });
            return send(200, answer, 'application/sdp');
          } finally { pending--; }
        }
        return send(404, { error: 'Not found' });
      }
      if (req.method !== 'GET') return send(405, { error: 'Method not allowed' });
      if (path === '/qwen/api/session') return authed ? send(200, { csrf: session.csrf }) : send(401, { error: '请登录' });
      const files = { '/qwen/': 'index.html', '/qwen/app.js': 'app.js', '/qwen/style.css': 'style.css', '/qwen/director.html': 'director.html', '/qwen/director.js': 'director.js', '/qwen/transcript.js': 'transcript.js' };
      const file = files[path];
      if (!file) return send(404, { error: 'Not found' });
      const type = file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html';
      send(200, await readFile(new URL(`public/${file}`, import.meta.url)), `${type}; charset=utf-8`);
    } catch {
      if (!res.headersSent) send(502, { error: '请求失败或超时，请重试' });
      else res.end();
    }
  });
  server.requestTimeout = 30000;
  server.headersTimeout = 10000;
  server.on('close', () => clearInterval(cleanup));
  return server;
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  createServer().listen(Number(process.env.PORT || 8788), '127.0.0.1', () => console.log('Qwen Web listening on loopback'));
}
