// Qwen realtime WS bridge: China Qwen 3.8 (default) and Singapore legacy.
// PCM16 / 16 kHz / mono input, text-only output, server-managed turns.
// Modern text/lifecycle events retain item IDs for bilingual frontend alignment.
// Legacy translation snapshots retain the existing Transcript contract.
// Audio is bounded and held until session.updated; stop drains session.finish.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use http::Request;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{connect_async_with_config, tungstenite::Message};

const QWEN_REALTIME_URL: &str =
    "wss://dashscope-intl.aliyuncs.com/api-ws/v1/realtime?model=qwen3-livetranslate-flash-realtime";
const QIANWEN_REALTIME_URL: &str =
    "wss://maas.qianwenaiapi.com/api-ws/v1/realtime?model=qwen3.8-livetranslate-flash-realtime";
// Three seconds of PCM16, 16 kHz mono; cap both bytes and tiny/empty packets.
const MAX_QUEUED_AUDIO_BYTES: usize = 16_000 * 2 * 3;
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

struct Session {
    audio_tx: mpsc::Sender<(Vec<u8>, OwnedSemaphorePermit)>,
    audio_budget: Arc<Semaphore>,
    stop_tx: mpsc::Sender<()>,
}

#[derive(Default)]
pub struct QwenState {
    sessions: Mutex<HashMap<u64, Session>>,
    next_id: Mutex<u64>,
}

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

    let session = Session {
        audio_tx,
        audio_budget: Arc::new(Semaphore::new(MAX_QUEUED_AUDIO_BYTES)),
        stop_tx,
    };
    state.sessions.lock().unwrap().insert(session_id, session);

    let event_ch = on_event.clone();

    tokio::spawn(async move {
        let _ = event_ch.send(QwenEvent::Status {
            state: "connecting".into(),
            message: None,
        });

        if let Err(e) = run_session(config, audio_rx, stop_rx, event_ch.clone()).await {
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

#[tauri::command]
pub async fn qwen_realtime_send_audio(
    session_id: u64,
    pcm: Vec<u8>,
    state: State<'_, QwenState>,
) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions
        .get(&session_id)
        .ok_or_else(|| "千问实时会话不存在或已结束".to_string())?;

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

async fn run_session(
    cfg: QwenRealtimeConfig,
    mut audio_rx: mpsc::Receiver<(Vec<u8>, OwnedSemaphorePermit)>,
    mut stop_rx: mpsc::Receiver<()>,
    event_ch: Channel<QwenEvent>,
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

    let (ws_stream, _) = tokio::select! {
        _ = stop_rx.recv() => return Ok(()),
        result = timeout(Duration::from_secs(15), connect_async_with_config(request, None, true)) => {
            result.map_err(|_| "千问连接超时".to_string())?
                .map_err(|_| "千问连接失败，请检查所选服务、API 密钥及网络".to_string())?
        }
    };

    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    let session_update = build_session_update(&cfg);
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
    let mut drain_deadline = None;

    // Guard: response.text.done can fire twice for one response when both text
    // and audio_transcript streams complete. With modalities=["text"] this
    // shouldn't happen, but keep the guard — cheap insurance, matches mobile.
    let mut last_done_response_id: Option<String> = None;

    loop {
        tokio::select! {
            biased;

            _ = stop_rx.recv(), if drain_deadline.is_none() => {
                // Do not submit queued microphone data after stop. Already sent
                // audio is finalized by the server, without manual commits.
                audio_rx.close();
                while audio_rx.try_recv().is_ok() {}
                let deadline = Instant::now() + Duration::from_secs(15);
                drain_deadline = Some(deadline);
                tokio::time::timeout_at(deadline, ws_sink.send(Message::Text(
                    serde_json::json!({"type": "session.finish"}).to_string().into()
                ))).await.map_err(|_| "结束千问会话超时".to_string())?
                    .map_err(|_| "结束千问会话失败".to_string())?;
            }

            _ = tokio::time::sleep_until(drain_deadline.unwrap_or(ack_deadline)),
                if drain_deadline.is_some() || !ready => {
                if drain_deadline.is_none() {
                    return Err("等待千问会话确认超时".into());
                }
                return Err("千问收尾超时，已显示的字幕会保留，尾句可能不完整".into());
            }

            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let value: serde_json::Value = match serde_json::from_str(&text) {
                            Ok(value) => value,
                            Err(_) => continue,
                        };
                        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if kind == "session.updated" && !ready && drain_deadline.is_none() {
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
                        if finished || failed { break; }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => return Err("千问实时连接中断".into()),
                    None => break,
                }
            }

            Some((pcm, _permit)) = audio_rx.recv(), if ready && drain_deadline.is_none() => {
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

    // A stalled close handshake must not retain the session indefinitely.
    let _ = timeout(Duration::from_millis(100), ws_sink.close()).await;
    Ok(())
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
