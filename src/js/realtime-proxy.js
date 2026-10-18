// Empty address keeps direct connections. Never silently bypass a configured relay.
export function getRealtimeProxy(address = '', token = '') {
    if (!address.trim()) return null;
    let url;
    try { url = new URL(address.trim()); } catch {
        throw new Error('中转地址必须是完整的 HTTPS 或 WSS 域名');
    }
    if (!['https:', 'wss:'].includes(url.protocol) || !url.hostname ||
        url.username || url.password || url.pathname !== '/' || url.search || url.hash) {
        throw new Error('中转地址只填写 HTTPS 或 WSS 域名，不含路径、密钥或参数');
    }
    if (!/^[A-Za-z0-9_-]{32,128}$/.test(token)) {
        throw new Error('团队令牌须为 32–128 位字母、数字、下划线或连字符');
    }
    url.protocol = 'wss:';
    return { url: url.href, token };
}
