import { createServer } from 'node:http';
import { createHash, timingSafeEqual } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import WebSocket, { WebSocketServer } from 'ws';

const upstreams = {
    '/soniox': 'wss://stt-rt.soniox.com/transcribe-websocket',
    '/openai': 'wss://api.openai.com/v1/realtime/translations?model=gpt-realtime-translate',
};
const tokenPattern = /^[A-Za-z0-9_-]{32,128}$/;
const MAX_BYTES = 1024 * 1024;
const digest = token => createHash('sha256').update(token).digest();

export function createRelay({ token, connectUpstream = (url, options) => new WebSocket(url, options) }) {
    if (!tokenPattern.test(token || '')) throw new Error('RELAY_TOKEN must contain 32–128 URL-safe characters');
    const expected = digest(token);
    const connections = new Set();
    const wsServer = new WebSocketServer({
        noServer: true, clientTracking: false, perMessageDeflate: false, maxPayload: MAX_BYTES,
        handleProtocols: protocols => protocols.has('translator-v1') ? 'translator-v1' : false,
    });
    const server = createServer({ maxHeaderSize: 16384, headersTimeout: 10000, requestTimeout: 10000 }, (req, res) => {
        const healthy = req.method === 'GET' && req.url === '/healthz';
        res.writeHead(healthy ? 200 : 404, { 'Content-Type': 'text/plain', 'Cache-Control': 'no-store' });
        res.end(healthy ? 'ok' : 'Not found');
    });
    server.maxConnections = 256;
    server.on('clientError', (_error, socket) => socket.destroy());
    server.on('upgrade', (req, socket, head) => {
        const reject = code => socket.end(`HTTP/1.1 ${code} Rejected\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
        if (req.method !== 'GET' || !Object.hasOwn(upstreams, req.url)) return reject(404);
        const soniox = req.url === '/soniox';
        const protocols = (req.headers['sec-websocket-protocol'] || '').split(',').map(p => p.trim());
        const supplied = soniox && protocols.includes('translator-v1')
            ? protocols.find(p => p.startsWith('auth.'))?.slice(5)
            : !soniox ? req.headers['x-translator-token'] : null;
        if (!tokenPattern.test(supplied || '') || !timingSafeEqual(digest(supplied), expected)) return reject(401);
        const authorization = req.headers.authorization || '';
        if (!soniox && !/^Bearer \S+$/.test(authorization)) return reject(401);
        if (connections.size >= 64) return reject(503);

        // No cookies, relay tokens, forwarded IPs, or arbitrary client headers reach the provider.
        const upstream = connectUpstream(upstreams[req.url], {
            headers: soniox ? {} : { Authorization: authorization },
            handshakeTimeout: 15000, perMessageDeflate: false, maxPayload: MAX_BYTES,
            followRedirects: false,
        });
        let client, heartbeat, closing = false;
        const close = (code = 1011, reason = 'Relay connection ended') => {
            if (closing) return;
            closing = true;
            clearInterval(heartbeat);
            const closeCode = code === 1005 ? 1000 : code === 1006 ? 1011 : code;
            for (const peer of [client, upstream]) {
                if (peer?.readyState === WebSocket.OPEN) peer.close(closeCode, reason);
                else if (peer?.readyState === WebSocket.CONNECTING) peer.terminate();
            }
            const timer = setTimeout(() => { client?.terminate(); upstream.terminate(); }, 5000);
            timer.unref();
            // Keep capacity reserved until both TCP connections are actually closed.
            const release = () => {
                if ((!client || client.readyState === WebSocket.CLOSED) && upstream.readyState === WebSocket.CLOSED) {
                    clearTimeout(timer);
                    connections.delete(stop);
                }
            };
            client?.once('close', release);
            upstream.once('close', release);
            release();
        };
        const stop = () => { if (!client) socket.destroy(); close(1012, 'Relay restarting'); };
        connections.add(stop);
        const abandoned = () => close();
        socket.once('close', abandoned);
        socket.on('error', abandoned);
        upstream.on('error', () => { if (!client) reject(502); close(); });
        upstream.on('close', (code) => { if (!client) reject(502); close(code); });
        upstream.once('open', () => {
            if (closing || socket.destroyed) return close();
            wsServer.handleUpgrade(req, socket, head, downstream => {
                socket.removeListener('close', abandoned);
                socket.removeListener('error', abandoned);
                client = downstream;
                client.on('error', () => close());
                client.on('close', code => close(code));
                const forward = (from, to) => from.on('message', (data, isBinary) => {
                    if (to.readyState !== WebSocket.OPEN) return close();
                    if (to.bufferedAmount + data.length > MAX_BYTES) return close(1013, 'Relay backpressure');
                    // Text stays text (including Soniox's empty final frame); audio stays binary.
                    to.send(data, { binary: isBinary }, error => { if (error) close(); });
                });
                forward(client, upstream);
                forward(upstream, client);
                let clientAlive = true, upstreamAlive = true;
                client.on('pong', () => { clientAlive = true; });
                upstream.on('pong', () => { upstreamAlive = true; });
                heartbeat = setInterval(() => {
                    if (!clientAlive || !upstreamAlive) return close(1011, 'Relay heartbeat timeout');
                    clientAlive = upstreamAlive = false;
                    if (client.readyState === WebSocket.OPEN) client.ping();
                    if (upstream.readyState === WebSocket.OPEN) upstream.ping();
                }, 30000);
                heartbeat.unref();
            });
        });
    });
    server.on('close', () => { for (const stop of connections) stop(); wsServer.close(); });
    return server;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    const server = createRelay({ token: process.env.RELAY_TOKEN });
    server.listen(Number(process.env.PORT || 8787), process.env.HOST || '127.0.0.1');
    process.once('SIGTERM', () => server.close());
    process.once('SIGINT', () => server.close());
}
