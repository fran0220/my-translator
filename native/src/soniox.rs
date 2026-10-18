use crate::settings::Settings;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{sync::mpsc::Sender, time::Duration};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

pub enum Event {
    Ready,
    Text(Value),
    Boundary,
}

pub fn configuration(settings: &Settings, context: &str) -> Value {
    let mut config = json!({
        "api_key": settings.soniox_api_key, "model":"stt-rt-v5",
        "audio_format":"pcm_s16le", "sample_rate":16000, "num_channels":1,
        "enable_endpoint_detection":true, "max_endpoint_delay_ms": settings.endpoint_delay,
        "enable_speaker_diarization":true, "enable_language_identification":true,
    });
    if settings.translation_type == "two_way" {
        config["translation"] = json!({"type":"two_way", "language_a":settings.language_a, "language_b":settings.language_b});
        config["language_hints"] = json!([settings.language_a, settings.language_b]);
    } else {
        config["translation"] =
            json!({"type":"one_way", "target_language":settings.target_language});
        if settings.source_language != "auto" {
            config["language_hints"] = json!([settings.source_language]);
        }
    }
    if settings.language_hints_strict {
        config["language_hints_strict"] = json!(true);
    }
    let mut hints = json!({});
    if let Some(custom) = &settings.custom_context {
        if let Some(domain) = &custom.domain {
            hints["general"] = json!([{"key":"domain","value":domain}]);
        }
        if !custom.translation_terms.is_empty() {
            hints["translation_terms"] = json!(custom.translation_terms);
        }
    }
    if !context.is_empty() {
        hints["text"] = json!(context);
    }
    if hints.as_object().is_some_and(|o| !o.is_empty()) {
        config["context"] = hints;
    }
    config
}

fn request(settings: &Settings) -> Result<http::Request<()>, String> {
    let proxied = !settings.realtime_proxy_url.trim().is_empty();
    let endpoint = if proxied {
        let mut url =
            reqwest::Url::parse(settings.realtime_proxy_url.trim()).map_err(|_| "中转地址无效")?;
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
        let token = &settings.realtime_proxy_token;
        if !(32..=128).contains(&token.len())
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("团队中转令牌格式无效".into());
        }
        url.set_scheme("wss").map_err(|_| "中转协议无效")?;
        url.set_path("/soniox");
        url.to_string()
    } else {
        "wss://stt-rt.soniox.com/transcribe-websocket".into()
    };
    let mut request = endpoint
        .into_client_request()
        .map_err(|_| "无法创建 Soniox 连接")?;
    if proxied {
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            format!("translator-v1, auth.{}", settings.realtime_proxy_token)
                .parse()
                .map_err(|_| "中转令牌无效")?,
        );
    }
    Ok(request)
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(
    settings: Settings,
    context: String,
    request: http::Request<()>,
) -> Result<Socket, String> {
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(15), connect_async(request))
        .await
        .map_err(|_| "Soniox 连接超时")?
        .map_err(|_| "Soniox 连接失败，请检查密钥和网络")?;
    tokio::time::timeout(
        Duration::from_secs(3),
        socket.send(Message::Text(
            configuration(&settings, &context).to_string().into(),
        )),
    )
    .await
    .map_err(|_| "Soniox 配置发送超时")?
    .map_err(|_| "Soniox 配置发送失败")?;
    Ok(socket)
}

pub async fn run(
    settings: Settings,
    audio: mpsc::Receiver<Vec<u8>>,
    stop: mpsc::Receiver<()>,
    events: Sender<Event>,
) -> Result<(), String> {
    let request = request(&settings)?;
    run_connection(
        settings,
        audio,
        stop,
        events,
        request,
        Duration::from_secs(180),
        Duration::from_secs(5),
    )
    .await
}

async fn run_connection(
    settings: Settings,
    mut audio: mpsc::Receiver<Vec<u8>>,
    mut stop: mpsc::Receiver<()>,
    events: Sender<Event>,
    request: http::Request<()>,
    rotation: Duration,
    drain_timeout: Duration,
) -> Result<(), String> {
    let mut socket = connect(settings.clone(), String::new(), request.clone()).await?;
    let _ = events.send(Event::Ready);
    let mut keepalive = tokio::time::interval(Duration::from_secs(5));
    let mut rotate_at = tokio::time::Instant::now() + rotation;
    let mut history = String::new();
    let mut replacement = None;
    let mut closing = false;
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        tokio::select! {
            biased;
            _ = stop.recv(), if !closing => {
                closing = true;
                replacement = None;
                audio.close();
                deadline = tokio::time::Instant::now() + drain_timeout;
                while let Some(pcm) = audio.recv().await {
                    tokio::time::timeout_at(deadline, socket.send(Message::Binary(pcm.into()))).await.map_err(|_| "Soniox 音频收尾超时")?.map_err(|_| "Soniox 音频收尾失败")?;
                }
                tokio::time::timeout_at(deadline, socket.send(Message::Binary(Vec::new().into()))).await.map_err(|_| "Soniox 收尾超时")?.map_err(|_| "Soniox 收尾失败")?;
            }
            _ = tokio::time::sleep_until(deadline), if closing => return Err("Soniox 收尾超时，尾句可能不完整".into()),
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).map_err(|_| "Soniox 响应格式错误")?;
                    if value.get("error_code").is_some() { return Err("Soniox 服务返回错误，请检查密钥、额度和设置".into()); }
                    let finished = value["finished"] == true;
                    if let Some(tokens) = value["tokens"].as_array() {
                        for token in tokens {
                            if token["is_final"] == true && token["translation_status"] == "translation" {
                                let text = token["text"].as_str().unwrap_or_default();
                                if text != "<end>" { history.push_str(text); }
                            }
                        }
                        let length = history.chars().count();
                        if length > 1500 { history = history.chars().skip(length - 1500).collect(); }
                    }
                    let _ = events.send(Event::Text(value));
                    if finished { return if closing { Ok(()) } else { Err("Soniox 会话已结束".into()) }; }
                }
                Some(Ok(Message::Close(_))) | None => return Err("Soniox 在最终确认前断开，尾句可能不完整".into()),
                Some(Err(_)) => return Err("Soniox 网络连接中断".into()),
                _ => {}
            },
            _ = tokio::time::sleep_until(rotate_at), if !closing && replacement.is_none() => {
                // Make-before-break: capture and the old socket continue while the replacement connects.
                replacement = Some(Box::pin(connect(settings.clone(), history.clone(), request.clone())));
            }
            result = async { replacement.as_mut().unwrap().await }, if replacement.is_some() => {
                let next = result?;
                replacement = None;
                // Do not read or feed the new connection until every old final has
                // been published. Capture remains queued, with bounded backpressure.
                finish_rotation(&mut socket, &events, drain_timeout).await?;
                let _ = events.send(Event::Boundary);
                socket = next;
                rotate_at = tokio::time::Instant::now() + rotation;
            }
            _ = keepalive.tick(), if !closing => {
                tokio::time::timeout(Duration::from_secs(3), socket.send(Message::Text(json!({"type":"keepalive"}).to_string().into()))).await.map_err(|_| "Soniox 网络超时")?.map_err(|_| "Soniox 网络中断")?;
            }
            Some(pcm) = audio.recv(), if !closing => {
                tokio::time::timeout(Duration::from_secs(3), socket.send(Message::Binary(pcm.into()))).await.map_err(|_| "Soniox 音频发送超时")?.map_err(|_| "Soniox 音频发送失败")?;
            }
        }
    }
}

async fn finish_rotation(
    socket: &mut Socket,
    events: &Sender<Event>,
    limit: Duration,
) -> Result<(), String> {
    tokio::time::timeout(limit, async {
        socket
            .send(Message::Binary(Vec::new().into()))
            .await
            .map_err(|_| "Soniox 轮换收尾发送失败")?;
        while let Some(message) = socket.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let value: Value =
                        serde_json::from_str(&text).map_err(|_| "Soniox 轮换响应格式错误")?;
                    if value.get("error_code").is_some() {
                        return Err("Soniox 轮换收尾服务错误");
                    }
                    let finished = value["finished"] == true;
                    let _ = events.send(Event::Text(value));
                    if finished {
                        return Ok(());
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        Err("Soniox 轮换连接在最终确认前断开，尾句可能不完整")
    })
    .await
    .map_err(|_| "Soniox 轮换收尾超时，尾句可能不完整")?
    .map_err(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn next_binary<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Vec<u8>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        loop {
            if let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() {
                return bytes.to_vec();
            }
        }
    }

    fn tokens(text: &str, final_: bool) -> Message {
        Message::Text(
            json!({"tokens":[{"text":text,"is_final":final_,"translation_status":"translation"}]})
                .to_string()
                .into(),
        )
    }

    #[tokio::test]
    async fn rotation_delivers_old_final_before_new_stream_and_drains_stop_audio() {
        tokio::time::timeout(Duration::from_secs(3), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let request = format!("ws://{}", listener.local_addr().unwrap())
                .into_client_request()
                .unwrap();
            let (audio_tx, audio) = mpsc::channel(15);
            let (stop_tx, stop) = mpsc::channel(1);
            let (events, rx) = std::sync::mpsc::channel();
            audio_tx.send(vec![1, 2, 3, 4]).await.unwrap();
            let server = tokio::spawn(async move {
                let mut old = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                    .await
                    .unwrap();
                assert!(matches!(
                    old.next().await.unwrap().unwrap(),
                    Message::Text(_)
                ));
                assert_eq!(next_binary(&mut old).await, [1, 2, 3, 4]);
                old.send(tokens("旧临时", false)).await.unwrap();
                let mut new = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                    .await
                    .unwrap();
                assert!(matches!(
                    new.next().await.unwrap().unwrap(),
                    Message::Text(_)
                ));
                // New data is available first: it must not enter old transcript state.
                new.send(tokens("新完整", true)).await.unwrap();
                assert!(next_binary(&mut old).await.is_empty());
                audio_tx.send(vec![9, 8]).await.unwrap();
                audio_tx.send(vec![7, 6, 5, 4]).await.unwrap();
                stop_tx.send(()).await.unwrap();
                tokio::time::sleep(Duration::from_millis(25)).await;
                old.send(tokens("旧完整", true)).await.unwrap();
                old.send(Message::Text(json!({"finished":true}).to_string().into()))
                    .await
                    .unwrap();
                assert_eq!(next_binary(&mut new).await, [9, 8]);
                assert_eq!(next_binary(&mut new).await, [7, 6, 5, 4]);
                assert!(next_binary(&mut new).await.is_empty());
                new.send(Message::Text(json!({"finished":true}).to_string().into()))
                    .await
                    .unwrap();
            });
            run_connection(
                Settings::default(),
                audio,
                stop,
                events,
                request,
                Duration::from_millis(40),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
            server.await.unwrap();
            let mut transcript = crate::transcript::Transcript::default();
            let mut boundaries = 0;
            for event in rx.try_iter() {
                match event {
                    Event::Text(value) => transcript.soniox(value),
                    Event::Boundary => {
                        boundaries += 1;
                        transcript.finish();
                    }
                    Event::Ready => {}
                }
            }
            transcript.finish();
            assert_eq!(boundaries, 1);
            assert_eq!(
                transcript
                    .segments
                    .iter()
                    .map(|s| s.target.as_str())
                    .collect::<Vec<_>>(),
                ["旧完整", "新完整"]
            );
            assert!(transcript.target.is_empty());
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rotation_missing_final_is_bounded_and_never_publishes_new_stream() {
        for disconnect in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let request = format!("ws://{}", listener.local_addr().unwrap())
                .into_client_request()
                .unwrap();
            let (_audio_tx, audio) = mpsc::channel(15);
            let (_stop_tx, stop) = mpsc::channel(1);
            let (events, rx) = std::sync::mpsc::channel();
            let server = tokio::spawn(async move {
                let mut old = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                    .await
                    .unwrap();
                old.next().await.unwrap().unwrap();
                let mut new = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                    .await
                    .unwrap();
                new.next().await.unwrap().unwrap();
                new.send(tokens("不应显示", true)).await.unwrap();
                assert!(next_binary(&mut old).await.is_empty());
                if disconnect {
                    old.close(None).await.unwrap();
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            });
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                run_connection(
                    Settings::default(),
                    audio,
                    stop,
                    events,
                    request,
                    Duration::from_millis(20),
                    Duration::from_millis(50),
                ),
            )
            .await
            .unwrap();
            assert!(
                result
                    .unwrap_err()
                    .contains(if disconnect { "断开" } else { "超时" })
            );
            assert!(rx.try_iter().all(|e| matches!(e, Event::Ready)));
            server.await.unwrap();
        }
    }

    #[test]
    fn bilingual_configuration_and_relay_validation() {
        let mut settings = Settings::default();
        settings.translation_type = "two_way".into();
        let config = configuration(&settings, "last sentence");
        assert_eq!(
            config["translation"],
            json!({"type":"two_way","language_a":"en","language_b":"zh"})
        );
        assert_eq!(config["context"]["text"], "last sentence");
        settings.realtime_proxy_url = "https://relay.example/path".into();
        assert!(request(&settings).is_err());
        settings.realtime_proxy_url = "https://relay.example".into();
        settings.realtime_proxy_token = "test-only-token-0123456789abcdefgh".into();
        let req = request(&settings).unwrap();
        assert_eq!(req.uri(), "wss://relay.example/soniox");
        assert!(req.headers().contains_key("Sec-WebSocket-Protocol"));
    }
}
