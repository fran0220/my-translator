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

pub async fn run(
    settings: Settings,
    mut audio: mpsc::Receiver<Vec<u8>>,
    mut stop: mpsc::Receiver<()>,
    events: Sender<Event>,
) -> Result<(), String> {
    let request = request(&settings)?;
    let (mut socket, _) = tokio::select! {
        _ = stop.recv() => return Ok(()),
        result = tokio::time::timeout(Duration::from_secs(15), connect_async(request)) => {
            result.map_err(|_| "Soniox 连接超时")?.map_err(|_| "Soniox 连接失败，请检查密钥和网络")?
        }
    };
    socket
        .send(Message::Text(
            configuration(&settings, "").to_string().into(),
        ))
        .await
        .map_err(|_| "Soniox 配置发送失败")?;
    let _ = events.send(Event::Ready);
    let mut keepalive = tokio::time::interval(Duration::from_secs(5));
    let mut closing = false;
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        tokio::select! {
            biased;
            _ = stop.recv(), if !closing => {
                closing = true;
                audio.close(); while audio.try_recv().is_ok() {}
                deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                tokio::time::timeout_at(deadline, socket.send(Message::Binary(Vec::new().into()))).await.map_err(|_| "Soniox 收尾超时")?.map_err(|_| "Soniox 收尾失败")?;
            }
            _ = tokio::time::sleep_until(deadline), if closing => return Err("Soniox 收尾超时，尾句可能不完整".into()),
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).map_err(|_| "Soniox 响应格式错误")?;
                    if value.get("error_code").is_some() { return Err("Soniox 服务返回错误，请检查密钥、额度和设置".into()); }
                    let finished = value["finished"] == true;
                    let _ = events.send(Event::Text(value));
                    if finished { return if closing { Ok(()) } else { Err("Soniox 会话已结束".into()) }; }
                }
                Some(Ok(Message::Close(_))) | None => return if closing { Ok(()) } else { Err("Soniox 连接已断开".into()) },
                Some(Err(_)) => return Err("Soniox 网络连接中断".into()),
                _ => {}
            },
            _ = keepalive.tick(), if !closing => {
                tokio::time::timeout(Duration::from_secs(3), socket.send(Message::Text(json!({"type":"keepalive"}).to_string().into()))).await.map_err(|_| "Soniox 网络超时")?.map_err(|_| "Soniox 网络中断")?;
            }
            Some(pcm) = audio.recv(), if !closing => {
                tokio::time::timeout(Duration::from_secs(3), socket.send(Message::Binary(pcm.into()))).await.map_err(|_| "Soniox 音频发送超时")?.map_err(|_| "Soniox 音频发送失败")?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
