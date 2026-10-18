/** Only finalized display-buffer text crosses this channel, never settings. */
const CHANNEL = 'my-translator-broadcast-v1';
const MAX_CHARS = 600;
let disposePrevious;

export function initBroadcastOutput(transcriptUI) {
    disposePrevious?.();
    let channel;
    let preview;
    let opening = false;
    let disposed = false;

    function report(message) {
        const status = document.getElementById('status-text');
        if (status) status.textContent = message;
        else window.alert(message);
        console.error(message);
    }

    function snapshot() {
        if (!channel || disposed) return;
        const segments = (transcriptUI.segments || []).slice(-20)
            .filter(segment => segment.status === 'original' || segment.status === 'translated');
        const recent = (field) => segments.map(segment =>
            typeof segment[field] === 'string' ? segment[field].slice(-MAX_CHARS) : ''
        ).filter(Boolean).join('\n').slice(-MAX_CHARS);
        channel.postMessage({ type: 'snapshot', source: recent('original'), translation: recent('translation') });
    }

    try {
        channel = new BroadcastChannel(CHANNEL);
        channel.onmessage = ({ data }) => {
            if (data?.type === 'request') snapshot();
            if (data?.type === 'error') report('无法进入全屏，请在导播窗口内重试。');
        };
    } catch {
        // Report only when the user tries to open output, not on startup.
    }

    async function open() {
        if (opening || disposed) return;
        if (!channel) {
            report('导播字幕需要支持 BroadcastChannel 的同源窗口。');
            return;
        }
        opening = true;
        try {
            const WebviewWindow = window.__TAURI__?.webviewWindow?.WebviewWindow;
            if (WebviewWindow) {
                let child = await WebviewWindow.getByLabel('broadcast');
                if (!child) {
                    child = new WebviewWindow('broadcast', {
                        url: 'broadcast.html', title: '导播字幕 — + / − 调整字号；F 全屏',
                        width: 1000, height: 600, minWidth: 320, minHeight: 200,
                        backgroundColor: '#000000', decorations: true, resizable: true,
                    });
                    await new Promise((resolve, reject) => {
                        const timer = setTimeout(() => reject(new Error('Window creation timed out')), 10000);
                        child.once('tauri://created', () => { clearTimeout(timer); resolve(); }).catch(reject);
                        child.once('tauri://error', () => { clearTimeout(timer); reject(new Error('Window creation failed')); }).catch(reject);
                    });
                }
                await child.show();
                await child.setFocus();
            } else {
                if (!preview || preview.closed) {
                    preview = window.open(new URL('broadcast.html', document.baseURI).href,
                        'broadcast', 'popup,width=1000,height=600');
                }
                if (!preview) throw new Error('Popup blocked');
                preview.focus();
            }
            snapshot();
        } catch {
            report('无法打开导播字幕。浏览器预览请允许弹窗，桌面端请检查窗口权限。');
        } finally {
            opening = false;
        }
    }

    // Delegation also supports a toolbar button inserted after initialization.
    const onClick = (event) => {
        if (event.target.closest?.('#btn-broadcast')) void open();
    };
    document.addEventListener('click', onClick);
    const timer = setInterval(snapshot, 500); // Full snapshots double as heartbeats; [] clears output.
    const onPageHide = () => channel?.postMessage({ type: 'clear' });
    window.addEventListener('pagehide', onPageHide);
    const dispose = () => {
        if (disposed) return;
        onPageHide();
        disposed = true;
        clearInterval(timer);
        document.removeEventListener('click', onClick);
        window.removeEventListener('pagehide', onPageHide);
        channel?.close();
    };
    disposePrevious = dispose;
    return dispose;
}
