// OpenAI Realtime translate provider — backend WebSocket bridge.
//
// Browsers can't set Authorization headers on WebSockets, so we run the WS
// in Rust. Frontend sends 16kHz s16le PCM via Tauri commands; we resample
// to 24kHz, base64-encode, and forward to OpenAI. Server events are parsed
// and emitted to the frontend via a Tauri Channel.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use http::Request;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

use crate::audio::resampler::UpsamplerTo24k;

const OPENAI_REALTIME_URL: &str =
    "wss://api.openai.com/v1/realtime/translations?model=gpt-realtime-translate";

#[derive(Debug, Deserialize)]
pub struct OpenAiRealtimeConfig {
    pub api_key: String,
    pub source_language: String,
    pub target_language: String,
    pub voice: Option<String>,
    #[serde(default)]
    pub proxy_url: String,
    #[serde(default)]
    pub proxy_token: String,
    /// Playback preference only. This API generates audio even when muted;
    /// it does not support text-only output or the corresponding cost savings.
    #[serde(default = "default_true")]
    pub audio_output: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpenAiEvent {
    Status {
        state: String,
        message: Option<String>,
    },
    Transcript {
        text: String,
        is_final: bool,
    },
    SourceTranscript {
        text: String,
        is_final: bool,
    },
    AudioChunk {
        pcm_base64: String,
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
    audio_tx: mpsc::UnboundedSender<Vec<u8>>,
    stop_tx: mpsc::UnboundedSender<()>,
    upsampler: Mutex<UpsamplerTo24k>,
}

#[derive(Default)]
pub struct OpenAiState {
    sessions: Mutex<HashMap<u64, Session>>,
    next_id: Mutex<u64>,
}

#[tauri::command]
pub async fn openai_realtime_start(
    config: OpenAiRealtimeConfig,
    on_event: Channel<OpenAiEvent>,
    state: State<'_, OpenAiState>,
    app: AppHandle,
) -> Result<u64, String> {
    if config.api_key.trim().is_empty() {
        return Err("OpenAI API key is empty".into());
    }

    let session_id = {
        let mut id = state.next_id.lock().unwrap();
        *id += 1;
        *id
    };

    let upsampler = UpsamplerTo24k::new()?;

    let (audio_tx, audio_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (stop_tx, stop_rx) = mpsc::unbounded_channel::<()>();

    let session = Session {
        audio_tx,
        stop_tx,
        upsampler: Mutex::new(upsampler),
    };
    state.sessions.lock().unwrap().insert(session_id, session);

    let event_ch = on_event.clone();
    let cfg = OpenAiRealtimeConfig {
        api_key: config.api_key,
        source_language: config.source_language,
        target_language: config.target_language,
        voice: config.voice,
        proxy_url: config.proxy_url,
        proxy_token: config.proxy_token,
        audio_output: config.audio_output,
    };

    tokio::spawn(async move {
        let _ = event_ch.send(OpenAiEvent::Status {
            state: "connecting".into(),
            message: None,
        });

        if let Err(e) = run_session(cfg, audio_rx, stop_rx, event_ch.clone()).await {
            let _ = event_ch.send(OpenAiEvent::Error {
                code: "session_failed".into(),
                message: e,
            });
        }

        app.state::<OpenAiState>()
            .sessions
            .lock()
            .unwrap()
            .remove(&session_id);
        let _ = event_ch.send(OpenAiEvent::Closed {
            reason: "session_ended".into(),
        });
    });

    Ok(session_id)
}

#[tauri::command]
pub async fn openai_realtime_send_audio(
    session_id: u64,
    pcm: Vec<u8>,
    state: State<'_, OpenAiState>,
) -> Result<(), String> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions
        .get(&session_id)
        .ok_or_else(|| format!("Session {} not found", session_id))?;

    let upsampled = session.upsampler.lock().unwrap().push(&pcm)?;
    if !upsampled.is_empty() {
        session
            .audio_tx
            .send(upsampled)
            .map_err(|e| format!("send audio failed: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn openai_realtime_stop(
    session_id: u64,
    state: State<'_, OpenAiState>,
) -> Result<(), String> {
    let mut sessions = state.sessions.lock().unwrap();
    if let Some(session) = sessions.remove(&session_id) {
        let _ = session.stop_tx.send(());
    }
    Ok(())
}

fn connection_request(cfg: &OpenAiRealtimeConfig) -> Result<Request<()>, String> {
    let proxied = !cfg.proxy_url.trim().is_empty();
    let endpoint = if proxied {
        let mut url = reqwest::Url::parse(cfg.proxy_url.trim()).map_err(|_| "中转地址格式无效")?;
        if !matches!(url.scheme(), "https" | "wss")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("中转地址只允许 HTTPS/WSS 域名，不含路径、凭据或参数".into());
        }
        if !(32..=128).contains(&cfg.proxy_token.len())
            || !cfg
                .proxy_token
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err("团队中转令牌格式无效".into());
        }
        url.set_scheme("wss").map_err(|_| "中转地址协议无效")?;
        url.set_path("/openai");
        url.to_string()
    } else {
        OPENAI_REALTIME_URL.to_string()
    };
    let mut request = endpoint
        .into_client_request()
        .map_err(|_| "无法创建实时连接")?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", cfg.api_key)
            .parse()
            .map_err(|_| "OpenAI API 密钥格式无效")?,
    );
    if proxied {
        request.headers_mut().insert(
            "X-Translator-Token",
            cfg.proxy_token
                .parse()
                .map_err(|_| "团队中转令牌格式无效")?,
        );
    }
    Ok(request)
}

async fn run_session(
    cfg: OpenAiRealtimeConfig,
    mut audio_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    mut stop_rx: mpsc::UnboundedReceiver<()>,
    event_ch: Channel<OpenAiEvent>,
) -> Result<(), String> {
    let request = connection_request(&cfg)?;

    let (ws_stream, _) = timeout(Duration::from_secs(15), connect_async(request))
        .await
        .map_err(|_| "OpenAI 连接超时".to_string())?
        .map_err(|_| "OpenAI 连接失败，请检查密钥和网络".to_string())?;

    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    // Send session.update once connected
    let session_update = build_session_update(&cfg);
    ws_sink
        .send(Message::Text(session_update.into()))
        .await
        .map_err(|e| format!("send session.update: {}", e))?;

    let mut ready = false;
    let ack_deadline = Instant::now() + Duration::from_secs(15);
    let mut drain_deadline = None;

    loop {
        tokio::select! {
            biased;

            _ = stop_rx.recv(), if drain_deadline.is_none() => {
                audio_rx.close();
                while audio_rx.try_recv().is_ok() {}
                let deadline = Instant::now() + Duration::from_secs(3);
                drain_deadline = Some(deadline);
                tokio::time::timeout_at(deadline, ws_sink.send(Message::Text(
                    serde_json::json!({"type": "session.close"}).to_string().into()
                ))).await.map_err(|_| "结束 OpenAI 会话超时".to_string())?
                    .map_err(|_| "结束 OpenAI 会话失败".to_string())?;
            }

            _ = tokio::time::sleep_until(drain_deadline.unwrap_or(ack_deadline)),
                if drain_deadline.is_some() || !ready => {
                if drain_deadline.is_none() { return Err("等待 OpenAI 会话确认超时".into()); }
                break;
            }

            Some(audio_chunk) = audio_rx.recv(), if ready && drain_deadline.is_none() => {
                let b64 = B64.encode(&audio_chunk);
                let evt = serde_json::json!({
                    "type": "session.input_audio_buffer.append",
                    "audio": b64,
                });
                if let Err(e) = ws_sink.send(Message::Text(evt.to_string().into())).await {
                    return Err(format!("send audio: {}", e));
                }
            }

            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_server_event(&text, &event_ch, cfg.audio_output);
                        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                        match value["type"].as_str() {
                            Some("session.updated") if !ready => {
                                ready = true;
                                let _ = event_ch.send(OpenAiEvent::Status { state: "ready".into(), message: None });
                            }
                            Some("session.closed" | "error") => break,
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(format!("ws error: {}", e)),
                    None => break,
                }
            }
        }
    }

    let _ = timeout(Duration::from_millis(100), ws_sink.close()).await;
    Ok(())
}

fn build_session_update(cfg: &OpenAiRealtimeConfig) -> String {
    // GA Realtime Translation schema (May 2026):
    // - WebSocket URL is /v1/realtime/translations?model=gpt-realtime-translate
    // - No voice / turn_detection at session level
    // - Input transcription uses gpt-realtime-whisper
    // - Output language goes under audio.output.language
    // The translation endpoint does NOT support text-only modality config —
    // both `session.modalities` and `session.output_modalities` are rejected.
    // For "mute", we drop output_audio events client-side instead (see
    // run_session — audio_output flag suppresses AudioChunk forwarding).
    let session = serde_json::json!({
        "audio": {
            "input": {
                "transcription": {"model": "gpt-realtime-whisper"},
                "noise_reduction": {"type": "near_field"}
            },
            "output": {"language": cfg.target_language}
        }
    });
    serde_json::json!({
        "type": "session.update",
        "session": session,
    })
    .to_string()
}

fn handle_server_event(text: &str, event_ch: &Channel<OpenAiEvent>, audio_output: bool) {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };

    let evt_type = match value.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return,
    };

    // Dev trace: log every event type (not the full payload — that's noisy)
    // so we can see the lifecycle around speech pauses and turn boundaries.
    eprintln!("[openai-realtime] event: {}", evt_type);

    match evt_type {
        "session.created" | "session.updated" => {
            // ack only
        }
        "session.input_transcript.delta" => {
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                let _ = event_ch.send(OpenAiEvent::SourceTranscript {
                    text: delta.into(),
                    is_final: false,
                });
            }
        }
        "session.input_transcript.done" | "session.input_audio_transcription.completed" => {
            // Whisper-side final for the input chunk. The `transcript` field
            // carries the full source text — use it as the authoritative source
            // (deltas can arrive at a different cadence than output translation
            // deltas, so accumulating deltas alone misaligns source/target pairs).
            let final_text = value
                .get("transcript")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("text").and_then(|v| v.as_str()));
            if let Some(t) = final_text {
                let _ = event_ch.send(OpenAiEvent::SourceTranscript {
                    text: t.into(),
                    is_final: true,
                });
            }
        }
        "session.output_transcript.delta" => {
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                let _ = event_ch.send(OpenAiEvent::Transcript {
                    text: delta.into(),
                    is_final: false,
                });
            }
        }
        "session.output_transcript.done" => {
            if let Some(t) = value.get("transcript").and_then(|v| v.as_str()) {
                let _ = event_ch.send(OpenAiEvent::Transcript {
                    text: t.into(),
                    is_final: true,
                });
            }
        }
        "session.output_audio.delta" => {
            // Always forward; JS client gates playback via setMuted() so the
            // user can toggle mute live without restarting the session.
            let _ = audio_output; // unused — kept on signature for future server-side hint.
            if let Some(b64) = value.get("delta").and_then(|v| v.as_str()) {
                let _ = event_ch.send(OpenAiEvent::AudioChunk {
                    pcm_base64: b64.into(),
                });
            }
        }
        "session.closed" => {}
        "error" => {
            let code = value
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let msg = "OpenAI 服务返回错误，请检查密钥、额度及语言设置".to_string();
            let _ = event_ch.send(OpenAiEvent::Error { code, message: msg });
        }
        other => {
            // Log unknown event types so we can discover server-side names we
            // haven't mapped yet (e.g. turn lifecycle around speech pauses).
            eprintln!("[openai-realtime] unhandled event type: {}", other);
        }
    }
}
