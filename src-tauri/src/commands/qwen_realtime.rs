// Qwen realtime WS bridge: China Qwen 3.8 (default) and Singapore legacy.
// PCM16 / 16 kHz / mono input, text-only output, server-managed turns.
// Modern text/lifecycle events retain item IDs for bilingual frontend alignment.
// Legacy translation snapshots retain the existing Transcript contract.
// Audio is bounded and held until session.updated; restart/stop cancel without finalization.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use http::Request;
use serde::{Deserialize, Serialize};
#[cfg(feature = "tauri-host")]
use std::collections::HashMap;
#[cfg(not(feature = "tauri-host"))]
use std::sync::mpsc::Sender as Channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(feature = "tauri-host")]
use tauri::ipc::Channel;
#[cfg(feature = "tauri-host")]
use tauri::{AppHandle, Manager, State};
#[cfg(feature = "tauri-host")]
use tokio::sync::Semaphore;
use tokio::sync::{mpsc, OwnedSemaphorePermit};
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{connect_async_with_config, tungstenite::Message};

const QWEN_REALTIME_URL: &str =
    "wss://dashscope-intl.aliyuncs.com/api-ws/v1/realtime?model=qwen3-livetranslate-flash-realtime";
const QIANWEN_REALTIME_URL: &str =
    "wss://maas.qianwenaiapi.com/api-ws/v1/realtime?model=qwen3.8-livetranslate-flash-realtime";
// Three seconds of PCM16, 16 kHz mono; cap both bytes and tiny/empty packets.
pub const MAX_QUEUED_AUDIO_BYTES: usize = 16_000 * 2 * 3;
const IO_TIMEOUT: Duration = Duration::from_secs(3);

fn default_service() -> String {
    "qianwen".into()
}

fn service_endpoint(service: &str) -> Result<(&'static str, &'static str), String> {
    match service {
        "qianwen" => Ok((QIANWEN_REALTIME_URL, "maas.qianwenaiapi.com")),
        "dashscope_intl" => Ok((QWEN_REALTIME_URL, "dashscope-intl.aliyuncs.com")),
        _ => Err("不支持的千问服务，请选择中国站或新加坡站".into()),
    }
}

#[derive(Debug, Deserialize)]
pub struct QwenRealtimeConfig {
    pub api_key: String,
    /// BCP-47-ish code (e.g. "en", "ja"). "auto" or empty → fallback "en".
    pub source_language: String,
    /// BCP-47-ish code (e.g. "vi"). Sent as `translation.language`.
    pub target_language: String,
    #[serde(default = "default_service")]
    pub service: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QwenEvent {
    Restarted {
        generation: u64,
    },
    Realtime {
        event: serde_json::Value,
    },
    Status {
        state: String,
        message: Option<String>,
    },
    Transcript {
        text: String,
        is_final: bool,
    },
    Error {
        code: String,
        message: String,
    },
    Closed {
        reason: String,
    },
}

#[cfg(feature = "tauri-host")]
struct Session {
    audio_tx: mpsc::Sender<(Vec<u8>, OwnedSemaphorePermit)>,
    audio_budget: Arc<Semaphore>,
    stop_tx: mpsc::Sender<()>,
    generation: Arc<Mutex<u64>>,
}

#[cfg(feature = "tauri-host")]
#[derive(Default)]
pub struct QwenState {
    sessions: Mutex<HashMap<u64, Session>>,
    next_id: Mutex<u64>,
}

#[cfg(feature = "tauri-host")]
impl QwenState {
    pub fn capture_generation(&self, id: u64) -> Result<Arc<Mutex<u64>>, String> {
        self.sessions
            .lock()
            .unwrap()
            .get(&id)
            .map(|session| session.generation.clone())
            .ok_or_else(|| "千问会话已结束".into())
    }
}

#[cfg(feature = "tauri-host")]
#[tauri::command]
pub async fn qwen_realtime_start(
    config: QwenRealtimeConfig,
    on_event: Channel<QwenEvent>,
    state: State<'_, QwenState>,
    app: AppHandle,
) -> Result<u64, String> {
    service_endpoint(&config.service)?;
    if config.api_key.trim().is_empty() {
        return Err("请填写所选千问服务的 API 密钥".into());
    }

    let session_id = {
        let mut id = state.next_id.lock().unwrap();
        *id += 1;
        *id
    };

    let (audio_tx, audio_rx) = mpsc::channel(256);
    let (stop_tx, stop_rx) = mpsc::channel(1);
    let generation = Arc::new(Mutex::new(0));

    let session = Session {
        audio_tx,
        audio_budget: Arc::new(Semaphore::new(MAX_QUEUED_AUDIO_BYTES)),
        stop_tx,
        generation: generation.clone(),
    };
    state.sessions.lock().unwrap().insert(session_id, session);

    let event_ch = on_event.clone();

    tokio::spawn(async move {
        let _ = event_ch.send(QwenEvent::Status {
            state: "connecting".into(),
            message: None,
        });

        if let Err(e) =
            run_session_generation(config, audio_rx, stop_rx, event_ch.clone(), generation).await
        {
            let _ = event_ch.send(QwenEvent::Error {
                code: "session_failed".into(),
                message: e,
            });
        }

        app.state::<QwenState>()
            .sessions
            .lock()
            .unwrap()
            .remove(&session_id);
        let _ = event_ch.send(QwenEvent::Closed {
            reason: "session_ended".into(),
        });
    });

    Ok(session_id)
}

#[cfg(feature = "tauri-host")]
#[tauri::command]
pub async fn qwen_realtime_send_audio(
    session_id: u64,
    pcm: Vec<u8>,
    generation: Option<u64>,
    state: State<'_, QwenState>,
) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions
        .get(&session_id)
        .ok_or_else(|| "千问实时会话不存在或已结束".to_string())?;

    let current_generation = session.generation.lock().unwrap();
    if generation.unwrap_or(0) != *current_generation {
        return Ok(());
    }
    if pcm.is_empty() {
        return Ok(());
    }
    if pcm.len() > MAX_QUEUED_AUDIO_BYTES {
        return Err("音频积压超过 3 秒，请稍后重试或重新连接".into());
    }
    let permit = session
        .audio_budget
        .clone()
        .try_acquire_many_owned(pcm.len() as u32)
        .map_err(|_| "音频积压超过 3 秒，请稍后重试或重新连接".to_string())?;
    session
        .audio_tx
        .try_send((pcm, permit))
        .map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => "音频队列已满，请稍后重试或重新连接".to_string(),
            mpsc::error::TrySendError::Closed(_) => "千问实时会话已结束".to_string(),
        })?;
    Ok(())
}

#[cfg(feature = "tauri-host")]
#[tauri::command]
pub async fn qwen_realtime_stop(
    session_id: u64,
    state: State<'_, QwenState>,
) -> Result<(), String> {
    let mut sessions = state.sessions.lock().unwrap();
    if let Some(session) = sessions.remove(&session_id) {
        let _ = session.stop_tx.try_send(());
    }
    Ok(())
}

pub async fn run_session(
    cfg: QwenRealtimeConfig,
    audio_rx: mpsc::Receiver<(Vec<u8>, OwnedSemaphorePermit)>,
    stop_rx: mpsc::Receiver<()>,
    event_ch: Channel<QwenEvent>,
) -> Result<(), String> {
    run_session_generation(cfg, audio_rx, stop_rx, event_ch, Arc::new(Mutex::new(0))).await
}

pub(crate) async fn run_session_generation(
    cfg: QwenRealtimeConfig,
    audio_rx: mpsc::Receiver<(Vec<u8>, OwnedSemaphorePermit)>,
    stop_rx: mpsc::Receiver<()>,
    event_ch: Channel<QwenEvent>,
    generation: Arc<Mutex<u64>>,
) -> Result<(), String> {
    let (url, host) = service_endpoint(&cfg.service)?;
    let request = Request::builder()
        .uri(url)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .map_err(|_| "无法创建千问连接请求，请检查 API 密钥".to_string())?;

    run_reconnecting(
        cfg,
        audio_rx,
        stop_rx,
        event_ch,
        generation,
        request,
        Duration::from_secs(300),
    )
    .await
}

async fn run_reconnecting(
    cfg: QwenRealtimeConfig,
    mut audio_rx: mpsc::Receiver<(Vec<u8>, OwnedSemaphorePermit)>,
    mut stop_rx: mpsc::Receiver<()>,
    event_ch: Channel<QwenEvent>,
    generation: Arc<Mutex<u64>>,
    request: Request<()>,
    interval: Duration,
) -> Result<(), String> {
    let mut restarted = false;
    loop {
        // Cancellation surrounds connect, configuration, reads AND pending sends.
        // Dropping this future drops the old socket; no finish/close-input request.
        tokio::select! {
            biased;
            _ = stop_rx.recv() => return Ok(()),
            _ = tokio::time::sleep(interval) => {},
            result = async {
                let (socket, _) = timeout(Duration::from_secs(15),
                    connect_async_with_config(request.clone(), None, true)).await
                    .map_err(|_| "千问连接超时".to_string())?
                    .map_err(|_| "千问连接失败，请检查所选服务、API 密钥及网络".to_string())?;
                if restarted { while audio_rx.try_recv().is_ok() {} }
                connected(&cfg, &mut audio_rx, &event_ch, socket).await
            } => return result,
        }
        let next = {
            let mut current = generation.lock().unwrap();
            *current += 1;
            while audio_rx.try_recv().is_ok() {}
            *current
        };
        let _ = event_ch.send(QwenEvent::Restarted { generation: next });
        restarted = true;
    }
}

async fn connected(
    cfg: &QwenRealtimeConfig,
    audio_rx: &mut mpsc::Receiver<(Vec<u8>, OwnedSemaphorePermit)>,
    event_ch: &Channel<QwenEvent>,
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<(), String> {
    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    let session_update = build_session_update(cfg);
    timeout(
        IO_TIMEOUT,
        ws_sink.send(Message::Text(session_update.into())),
    )
    .await
    .map_err(|_| "发送千问会话配置超时".to_string())?
    .map_err(|_| "发送千问会话配置失败".to_string())?;

    let modern = cfg.service == "qianwen";
    let mut ready = false;
    let ack_deadline = Instant::now() + Duration::from_secs(15);

    // Guard: response.text.done can fire twice for one response when both text
    // and audio_transcript streams complete. With modalities=["text"] this
    // shouldn't happen, but keep the guard — cheap insurance, matches mobile.
    let mut last_done_response_id: Option<String> = None;

    loop {
        tokio::select! {
            biased;

            _ = tokio::time::sleep_until(ack_deadline), if !ready => {
                return Err("等待千问会话确认超时".into());
            }

            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let value: serde_json::Value = match serde_json::from_str(&text) {
                            Ok(value) => value,
                            Err(_) => continue,
                        };
                        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if kind == "session.updated" && !ready {
                            ready = true;
                            let _ = event_ch.send(QwenEvent::Status {
                                state: "ready".into(), message: None,
                            });
                        }
                        let finished = kind == "session.finished";
                        let failed = kind == "error";
                        if let Some(event) = map_server_event(value, modern, &mut last_done_response_id) {
                            let _ = event_ch.send(event);
                        }
                        if failed || finished {
                            return Err("千问服务异常结束，尾句可能不完整".into());
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Close(_))) | None => {
                        return Err("千问实时连接已关闭".into());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => return Err("千问实时连接中断".into()),
                }
            }

            Some((pcm, _permit)) = audio_rx.recv(), if ready => {
                let evt = serde_json::json!({
                    "type": "input_audio_buffer.append",
                    "audio": B64.encode(&pcm),
                });
                timeout(IO_TIMEOUT, ws_sink.send(Message::Text(evt.to_string().into())))
                    .await.map_err(|_| "发送千问音频超时".to_string())?
                    .map_err(|_| "发送千问音频失败".to_string())?;
            }
        }
    }
}

fn build_session_update(cfg: &QwenRealtimeConfig) -> String {
    // Live Flash rejects "auto" for source. Empty / "auto" → "en" fallback
    // (mirrors mobile client onopen handler).
    let source = if cfg.source_language.is_empty() || cfg.source_language == "auto" {
        "en"
    } else {
        cfg.source_language.as_str()
    };

    let session = if cfg.service == "qianwen" {
        serde_json::json!({
            "output_modalities": ["text"],
            "translation": { "language": cfg.target_language },
        })
    } else {
        serde_json::json!({
            "modalities": ["text"],
            "input_audio_format": "pcm",
            "input_audio_transcription": { "language": source },
            "translation": { "language": cfg.target_language },
            // VAD is server-side — manual commits rejected on Live Flash.
            "turn_detection": serde_json::Value::Null,
        })
    };

    serde_json::json!({
        "type": "session.update",
        "session": session,
    })
    .to_string()
}

fn map_server_event(
    mut value: serde_json::Value,
    modern: bool,
    last_done_response_id: &mut Option<String>,
) -> Option<QwenEvent> {
    let evt_type = value.get("type")?.as_str()?;
    if evt_type == "error" {
        return Some(QwenEvent::Error {
            code: value
                .pointer("/error/code")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            // Provider error messages may echo request content or credentials.
            message: "千问服务返回错误，请检查所选服务、API 密钥和语言设置".into(),
        });
    }
    if modern {
        if is_realtime_text_event(evt_type) {
            strip_audio_payloads(&mut value);
            return Some(QwenEvent::Realtime { event: value });
        }
        return None;
    }

    match evt_type {
        "session.created" | "session.updated" | "response.created" | "response.done" => {}

        "response.text.text" => {
            // Live Flash streams via text (committed) + stash (pending).
            // Full provisional snapshot = text + stash. Emit on every tick.
            let committed = value.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let stash = value.get("stash").and_then(|v| v.as_str()).unwrap_or("");
            let snapshot = format!("{}{}", committed, stash);
            if !snapshot.is_empty() {
                return Some(QwenEvent::Transcript {
                    text: snapshot,
                    is_final: false,
                });
            }
        }

        "response.text.done" => {
            let response_id = value
                .get("response_id")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("item_id").and_then(|v| v.as_str()))
                .map(|s| s.to_string());

            if let (Some(rid), Some(last)) = (response_id.as_ref(), last_done_response_id.as_ref())
            {
                if rid == last {
                    return None;
                }
            }
            *last_done_response_id = response_id;

            let t = value.get("text").and_then(|v| v.as_str());
            if let Some(t) = t {
                return Some(QwenEvent::Transcript {
                    text: t.into(),
                    is_final: true,
                });
            }
        }

        _ => {}
    }
    None
}

fn is_realtime_text_event(kind: &str) -> bool {
    // Audio transcription is text; response.audio.delta is base64 and is not.
    !kind.starts_with("response.audio.")
        && (kind.starts_with("conversation.")
            || kind.starts_with("response.")
            || kind.starts_with("input_audio_buffer.")
            || kind.starts_with("session."))
}

fn strip_audio_payloads(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            fields.remove("audio");
            for child in fields.values_mut() {
                strip_audio_payloads(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                strip_audio_payloads(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(service: Option<&str>) -> QwenRealtimeConfig {
        let mut value = json!({
            "api_key": "test-placeholder",
            "source_language": "auto",
            "target_language": "zh"
        });
        if let Some(service) = service {
            value["service"] = json!(service);
        }
        serde_json::from_value(value).unwrap()
    }

    #[cfg(not(feature = "tauri-host"))]
    #[tokio::test]
    async fn restart_discards_old_audio_and_events_and_stop_never_finishes_input() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        for service in ["qianwen", "dashscope_intl"] {
            timeout(Duration::from_secs(5), async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let request = format!("ws://{}", listener.local_addr().unwrap()).into_client_request().unwrap();
                let (audio, audio_rx) = mpsc::channel(16);
                let budget = Arc::new(tokio::sync::Semaphore::new(100));
                audio.send((vec![1, 2], budget.clone().acquire_many_owned(2).await.unwrap())).await.unwrap();
                let (stop, stop_rx) = mpsc::channel(1);
                let (events, rx) = std::sync::mpsc::channel();
                let generation = Arc::new(Mutex::new(0));
                let task = tokio::spawn(run_reconnecting(config(Some(service)), audio_rx, stop_rx,
                    events, generation.clone(), request, Duration::from_millis(250)));
                let mut old = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0).await.unwrap();
                assert_eq!(serde_json::from_str::<serde_json::Value>(old.next().await.unwrap().unwrap().to_text().unwrap()).unwrap()["type"], "session.update");
                // No acknowledgement: pre-restart audio must never leave the queue.
                let mut new = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0).await.unwrap();
                assert_eq!(serde_json::from_str::<serde_json::Value>(new.next().await.unwrap().unwrap().to_text().unwrap()).unwrap()["type"], "session.update");
                assert_eq!(*generation.lock().unwrap(), 1);
                assert_eq!(budget.available_permits(), 100);
                assert!(matches!(rx.try_recv().unwrap(), QwenEvent::Restarted { generation: 1 }));
                // Try a late result on the canceled socket. It must never reach consumers.
                let _ = old.send(Message::Text(json!({"type":"response.text.done","item_id":"same","text":"OLD"}).to_string().into())).await;
                match old.next().await {
                    Some(Ok(Message::Text(frame))) => panic!("unexpected old-session frame: {frame}"),
                    _ => {},
                }
                new.send(Message::Text(json!({"type":"session.updated"}).to_string().into())).await.unwrap();
                audio.send((vec![7, 8], budget.clone().acquire_many_owned(2).await.unwrap())).await.unwrap();
                let frame = new.next().await.unwrap().unwrap();
                let frame: serde_json::Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                assert_eq!(frame["type"], "input_audio_buffer.append");
                assert_eq!(B64.decode(frame["audio"].as_str().unwrap()).unwrap(), [7, 8]);
                new.send(Message::Text(json!({"type":"response.text.done","response_id":"same","item_id":"same","text":"NEW"}).to_string().into())).await.unwrap();
                let mut saw_new = false;
                while !saw_new {
                    for event in rx.try_iter() {
                        let json = serde_json::to_string(&event).unwrap();
                        assert!(!json.contains("OLD"));
                        saw_new |= json.contains("NEW");
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                stop.send(()).await.unwrap();
                timeout(Duration::from_millis(100), task).await.unwrap().unwrap().unwrap();
                // EOF/reset is immediate. In particular, there is no session.finish.
                assert!(!matches!(new.next().await, Some(Ok(Message::Text(_)))));
                assert_eq!(budget.available_permits(), 100);
            }).await.unwrap();
        }
    }

    #[cfg(not(feature = "tauri-host"))]
    #[tokio::test]
    async fn stop_cancels_stalled_websocket_handshake() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let request = format!("ws://{}", listener.local_addr().unwrap())
            .into_client_request()
            .unwrap();
        let (_audio, audio_rx) = mpsc::channel(1);
        let (stop, stop_rx) = mpsc::channel(1);
        let (events, _rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(run_reconnecting(
            config(None),
            audio_rx,
            stop_rx,
            events,
            Arc::new(Mutex::new(0)),
            request,
            Duration::from_secs(300),
        ));
        let _stalled_peer = listener.accept().await.unwrap();
        stop.send(()).await.unwrap();
        timeout(Duration::from_millis(100), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[cfg(not(feature = "tauri-host"))]
    #[tokio::test]
    async fn restart_cancels_a_blocked_socket_write() {
        use std::{future::Future, task::Poll};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let request = format!("ws://{}", listener.local_addr().unwrap())
            .into_client_request()
            .unwrap();
        const FRAME_BYTES: u32 = 6400; // Normal 200 ms PCM16/16 kHz batch.
        let (audio, audio_rx) = mpsc::channel(1);
        let budget = Arc::new(tokio::sync::Semaphore::new(FRAME_BYTES as usize));
        let (stop, stop_rx) = mpsc::channel(1);
        let (events, rx) = std::sync::mpsc::channel();
        let generation = Arc::new(Mutex::new(0));
        let mut session = Box::pin(run_reconnecting(
            config(None),
            audio_rx,
            stop_rx,
            events,
            generation.clone(),
            request,
            Duration::from_secs(300),
        ));
        let peer = async {
            let stream = listener.accept().await.unwrap().0;
            // Constrain the actual accepted socket, not an assumed inherited
            // listener option. Windows may otherwise buffer the entire old 8 MiB fixture.
            let socket = socket2::SockRef::from(&stream);
            socket.set_recv_buffer_size(4096).unwrap();
            assert!(socket.recv_buffer_size().unwrap() <= 64 * 1024);
            let mut old = tokio_tungstenite::accept_async(stream).await.unwrap();
            old.next().await.unwrap().unwrap();
            old.send(Message::Text(
                json!({"type":"session.updated"}).to_string().into(),
            ))
            .await
            .unwrap();
            old
        };
        let _old = tokio::select! {
            result = &mut session => panic!("session ended during handshake: {result:?}"),
            old = timeout(Duration::from_secs(5), peer) => old.unwrap(),
        };
        // Drive the real connection until its acknowledgement is processed.
        timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                assert!(session.as_mut().poll(cx).is_pending());
                if rx.try_iter().any(
                    |event| matches!(event, QwenEvent::Status { state, .. } if state == "ready"),
                ) {
                    Poll::Ready(())
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }),
        )
        .await
        .unwrap();

        // Time cannot expire while establishing backpressure. Drive the real
        // future directly: an empty channel AND a retained permit after Pending
        // prove it is suspended inside send, not just waiting to be scheduled.
        tokio::time::pause();
        let mut blocked = false;
        for _ in 0..4096 {
            // Safety cap, not an assumed kernel buffer size.
            let permit = budget.clone().try_acquire_many_owned(FRAME_BYTES).unwrap();
            audio
                .try_send((vec![0; FRAME_BYTES as usize], permit))
                .unwrap();
            // Exclude cooperative scheduling yields as a source of Pending.
            assert!(futures_util::poll!(tokio::task::unconstrained(session.as_mut())).is_pending());
            assert_eq!(
                audio.capacity(),
                1,
                "the session must have consumed the queued frame"
            );
            if budget.available_permits() == 0 {
                blocked = true;
                break;
            }
        }
        assert!(
            blocked,
            "peer must exert backpressure before testing cancellation"
        );
        assert_eq!(*generation.lock().unwrap(), 0);
        tokio::time::advance(Duration::from_secs(300)).await;
        assert!(futures_util::poll!(session.as_mut()).is_pending());
        assert_eq!(
            *generation.lock().unwrap(),
            1,
            "rotation must win over the send timeout"
        );
        assert_eq!(
            budget.available_permits(),
            FRAME_BYTES as usize,
            "canceling the pending write must release its permit"
        );
        tokio::time::resume();
        let replacement = async {
            let mut new = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap();
            let update = new.next().await.unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(update.to_text().unwrap()).unwrap()
                    ["type"],
                "session.update"
            );
        };
        tokio::select! {
            result = &mut session => panic!("session ended instead of reconnecting: {result:?}"),
            result = timeout(Duration::from_secs(5), replacement) => result.unwrap(),
        }
        stop.send(()).await.unwrap();
        timeout(Duration::from_millis(100), session)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn profiles_and_default_schema() {
        let cfg = config(None);
        assert_eq!(cfg.service, "qianwen");
        assert_eq!(service_endpoint(&cfg.service).unwrap(), (
            "wss://maas.qianwenaiapi.com/api-ws/v1/realtime?model=qwen3.8-livetranslate-flash-realtime",
            "maas.qianwenaiapi.com"
        ));
        let update: serde_json::Value = serde_json::from_str(&build_session_update(&cfg)).unwrap();
        assert_eq!(
            update,
            json!({"type": "session.update", "session": {
                "output_modalities": ["text"], "translation": {"language": "zh"}
            }})
        );
        assert!(service_endpoint("").is_err());
        assert!(service_endpoint("dashscope").is_err());
        assert!(service_endpoint("Qianwen").is_err());
    }

    #[test]
    fn legacy_profile_retains_schema_and_source_fallback() {
        let mut cfg = config(Some("dashscope_intl"));
        assert_eq!(service_endpoint(&cfg.service).unwrap(), (
            "wss://dashscope-intl.aliyuncs.com/api-ws/v1/realtime?model=qwen3-livetranslate-flash-realtime",
            "dashscope-intl.aliyuncs.com"
        ));
        for source in ["auto", "", "ja"] {
            cfg.source_language = source.into();
            let update: serde_json::Value =
                serde_json::from_str(&build_session_update(&cfg)).unwrap();
            assert_eq!(
                update["session"],
                json!({
                    "modalities": ["text"], "input_audio_format": "pcm",
                    "input_audio_transcription": {"language": if source == "ja" {"ja"} else {"en"}},
                    "translation": {"language": "zh"}, "turn_detection": null
                })
            );
        }
    }

    #[test]
    fn modern_events_preserve_alignment_and_deltas() {
        for kind in [
            "conversation.item.created",
            "conversation.item.input_audio_transcription.delta",
            "conversation.item.input_audio_transcription.completed",
            "response.created",
            "response.text.delta",
            "response.text.done",
            "response.output_item.done",
            "response.done",
            "session.updated",
            "session.finished",
        ] {
            let value = json!({"type": kind, "item_id": "input-1", "response_id": "response-1",
                "previous_item_id": "input-1", "item": {"id": "assistant-1"},
                "delta": "你好", "text": "你好", "transcript": "hello"});
            let event = map_server_event(value.clone(), true, &mut None).unwrap();
            assert_eq!(
                serde_json::to_value(event).unwrap(),
                json!({"type": "realtime", "event": value})
            );
        }
    }

    #[test]
    fn audio_payloads_are_not_forwarded() {
        assert!(map_server_event(
            json!({"type": "response.audio.delta", "delta": "base64"}),
            true,
            &mut None
        )
        .is_none());
        let event = map_server_event(
            json!({"type": "conversation.item.created",
                "previous_item_id": "input-1", "item": {"id": "assistant-1",
                "content": [{"type": "input_audio", "audio": "base64", "transcript": "hello"}]}
            }),
            true,
            &mut None,
        )
        .unwrap();
        let value = serde_json::to_value(event).unwrap();
        assert!(value["event"]["item"]["content"][0].get("audio").is_none());
        assert_eq!(value["event"]["item"]["content"][0]["transcript"], "hello");
        assert_eq!(value["event"]["previous_item_id"], "input-1");
    }

    #[test]
    fn legacy_snapshots_and_duplicate_finals() {
        let mut last = None;
        let event = map_server_event(
            json!({"type": "response.text.text", "text": "a", "stash": "b"}),
            false,
            &mut last,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({"type": "transcript", "text": "ab", "is_final": false})
        );
        let done = json!({"type": "response.text.done", "response_id": "r1", "text": "ab"});
        assert!(map_server_event(done.clone(), false, &mut last).is_some());
        assert!(map_server_event(done, false, &mut last).is_none());
    }

    #[test]
    fn errors_do_not_echo_provider_messages() {
        let event = map_server_event(
            json!({"type": "error", "error": {
                "code": "invalid_api_key", "message": "sensitive request content"
            }}),
            true,
            &mut None,
        )
        .unwrap();
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "error");
        assert_eq!(value["code"], "invalid_api_key");
        assert!(!value.to_string().contains("sensitive request content"));
    }
}
