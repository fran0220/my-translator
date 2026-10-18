const source = document.getElementById('source');
const translation = document.getElementById('translation');
let lastHeartbeat = 0;
let fontSize = 40;
let channel;
let togglingFullscreen = false;

function render(original = '', translated = '') {
    source.textContent = original;
    translation.textContent = translated;
    // Always retain the newest lines when text exceeds the capture area.
    source.scrollTop = source.scrollHeight;
    translation.scrollTop = translation.scrollHeight;
}

try {
    channel = new BroadcastChannel('my-translator-broadcast-v1');
    channel.onmessage = ({ data }) => {
        if (data?.type === 'clear') {
            lastHeartbeat = 0;
            render();
        } else if (data?.type === 'snapshot' && typeof data.source === 'string'
            && typeof data.translation === 'string') {
            lastHeartbeat = Date.now();
            render(data.source.slice(-600), data.translation.slice(-600));
        }
    };
    channel.postMessage({ type: 'request' });
} catch {
    // Capture output stays black, including when messaging is unavailable.
}

setInterval(() => {
    if (lastHeartbeat && Date.now() - lastHeartbeat > 5000) {
        lastHeartbeat = 0;
        render();
    }
}, 500);

async function toggleFullscreen() {
    if (togglingFullscreen) return;
    togglingFullscreen = true;
    try {
        const current = window.__TAURI__?.window?.getCurrentWindow();
        if (current) await current.setFullscreen(!await current.isFullscreen());
        else if (document.fullscreenElement) await document.exitFullscreen();
        else await document.documentElement.requestFullscreen();
    } catch {
        channel?.postMessage({ type: 'error' });
    } finally {
        togglingFullscreen = false;
    }
}

document.addEventListener('dblclick', () => void toggleFullscreen());
document.addEventListener('keydown', (event) => {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (['+', '=', '-', '_'].includes(event.key)) {
        event.preventDefault();
        fontSize = Math.max(16, Math.min(120, fontSize + (['+', '='].includes(event.key) ? 4 : -4)));
        document.documentElement.style.setProperty('--caption-size', `${fontSize}px`);
        source.scrollTop = source.scrollHeight;
        translation.scrollTop = translation.scrollHeight;
    } else if (event.key.toLowerCase() === 'f' && !event.repeat) {
        event.preventDefault();
        void toggleFullscreen();
    }
});
window.addEventListener('resize', () => {
    source.scrollTop = source.scrollHeight;
    translation.scrollTop = translation.scrollHeight;
});
window.addEventListener('pageshow', () => channel?.postMessage({ type: 'request' }));
