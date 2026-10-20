//! Own capture on one OS thread. Network tasks run on Tokio, never on the GUI executor.
use crate::{
    audio::{
        devices::{self, DeviceInfo, Kind},
        meter::Meter,
        microphone::MicCapture,
        resampler::UpsamplerTo24k,
    },
    openai, qwen,
    settings::Settings,
    soniox,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tokio::{
    runtime::Runtime,
    sync::{Semaphore, mpsc as async_mpsc},
    task::JoinHandle,
};

pub enum Event {
    Status(String),
    OpenAi(openai::OpenAiEvent),
    Qwen(qwen::QwenEvent),
    Soniox(serde_json::Value),
    Boundary,
    Stopped,
}

pub struct Session {
    stop: Arc<AtomicBool>,
    pub events: mpsc::Receiver<Event>,
    pub diagnostics: Arc<Mutex<[ChannelDiagnostics; 2]>>,
}

#[derive(Clone, Default)]
pub struct ChannelDiagnostics {
    pub device: Option<DeviceInfo>,
    pub meter: Meter,
    pub last_packet: Option<Instant>,
    pub opened: bool,
}

impl Session {
    pub fn start(settings: Settings, failover: bool) -> Result<Self, String> {
        let providers = providers(&settings, failover);
        if providers.is_empty() {
            return Err("请填写所选服务的 API 密钥；双向自动识别仅支持 Soniox".into());
        }
        let runtime = Runtime::new().map_err(|_| "无法启动网络运行时")?;
        let (tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let diagnostics = Arc::new(Mutex::new(Default::default()));
        let levels = diagnostics.clone();
        std::thread::spawn(move || {
            if let Err(message) = run(settings, providers, runtime, flag, &tx, &levels) {
                let _ = tx.send(Event::Status(message));
            }
            let _ = tx.send(Event::Boundary);
            let _ = tx.send(Event::Stopped);
        });
        Ok(Self {
            stop,
            events,
            diagnostics,
        })
    }
    /// Capture diagnostics only: no runtime, credentials, provider or network task.
    pub fn preview(settings: Settings) -> Self {
        let (tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let diagnostics = Arc::new(Mutex::new(Default::default()));
        let levels = diagnostics.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let mut capture = Capture::start(&settings, &levels)?;
                let _ = tx.send(Event::Status("试音中 · 仅本地采集，不连接翻译服务".into()));
                let mut inputs = vec![vec![]; capture.inputs.len()];
                while !flag.load(Ordering::SeqCst) {
                    capture.read(&mut inputs, &levels)?;
                    for input in &mut inputs {
                        input.clear();
                    }
                    std::thread::sleep(Duration::from_millis(30));
                }
                Ok(())
            })();
            let _ = tx.send(Event::Status(result.err().unwrap_or("试音已停止".into())));
            let _ = tx.send(Event::Stopped);
        });
        Self {
            stop,
            events,
            diagnostics,
        }
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn providers(settings: &Settings, failover: bool) -> Vec<&'static str> {
    let mut names = vec![];
    let allowed = |p: &str| {
        if settings.translation_type == "two_way" && p != "soniox" {
            return false;
        }
        match p {
            "soniox" => !settings.soniox_api_key.trim().is_empty(),
            "openai" => !settings.openai_api_key.trim().is_empty(),
            "qwen" => !settings.qwen_api_key.trim().is_empty(),
            _ => false,
        }
    };
    if let Some(name) = ["soniox", "openai", "qwen"]
        .into_iter()
        .find(|p| *p == settings.translation_mode)
    {
        if allowed(name) {
            names.push(name);
        }
    }
    if failover {
        for name in ["soniox", "openai", "qwen"] {
            if allowed(name) && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

struct Capture {
    #[cfg(not(target_os = "windows"))]
    mic: MicCapture,
    #[cfg(target_os = "windows")]
    mic: crate::audio::wasapi::SystemAudioCapture,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    system: crate::audio::SystemAudioCapture,
    inputs: Vec<mpsc::Receiver<Vec<u8>>>,
    devices: Vec<DeviceInfo>,
    checked: Instant,
}
impl Capture {
    fn start(settings: &Settings, levels: &Mutex<[ChannelDiagnostics; 2]>) -> Result<Self, String> {
        let source = settings.audio_source.as_str();
        let available = devices::enumerate()?;
        let mut capture = Self {
            #[cfg(not(target_os = "windows"))]
            mic: MicCapture::new(),
            #[cfg(target_os = "windows")]
            mic: crate::audio::wasapi::SystemAudioCapture::new(),
            inputs: vec![],
            devices: vec![],
            checked: Instant::now(),
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            system: crate::audio::SystemAudioCapture::new(),
        };
        if matches!(source, "system" | "both") {
            #[cfg(target_os = "windows")]
            {
                let device =
                    devices::resolve(&available, Kind::Output, &settings.output_device_id)?;
                capture
                    .inputs
                    .push(capture.system.start_selected(&device.id, Kind::Output)?);
                capture.devices.push(device);
            }
            #[cfg(target_os = "macos")]
            {
                capture.inputs.push(capture.system.start()?);
                capture.devices.push(DeviceInfo {
                    id: String::new(),
                    name: "系统音频".into(),
                    kind: Kind::Output,
                    default: true,
                    available: true,
                    state: "可用".into(),
                    muted: None,
                });
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            return Err("当前平台尚未提供系统音频采集，请选择麦克风；Windows 使用 WASAPI".into());
        }
        if matches!(source, "microphone" | "both") {
            let device = devices::resolve(&available, Kind::Input, &settings.input_device_id)?;
            #[cfg(target_os = "windows")]
            capture
                .inputs
                .push(capture.mic.start_selected(&device.id, Kind::Input)?);
            #[cfg(not(target_os = "windows"))]
            capture.inputs.push(capture.mic.start()?);
            capture.devices.push(device);
        }
        if capture.inputs.is_empty() {
            return Err("请选择音频来源".into());
        }
        for device in &capture.devices {
            levels.lock().unwrap()[if device.kind == Kind::Input { 0 } else { 1 }] =
                ChannelDiagnostics {
                    device: Some(device.clone()),
                    opened: true,
                    ..Default::default()
                };
        }
        Ok(capture)
    }
    fn read(
        &mut self,
        inputs: &mut [Vec<u8>],
        levels: &Mutex<[ChannelDiagnostics; 2]>,
    ) -> Result<(), String> {
        if self.checked.elapsed() >= Duration::from_secs(2) {
            let available = devices::enumerate()?;
            for device in &self.devices {
                if !device.id.is_empty()
                    && devices::resolve(&available, device.kind, &device.id).is_err()
                {
                    return Err(format!(
                        "设备已离线：{}；请重新连接后再开始，不会切换其他设备",
                        device.name
                    ));
                }
            }
            self.checked = Instant::now();
        }
        for (index, input) in self.inputs.iter().enumerate() {
            let mut batch = vec![];
            for _ in 0..100 {
                match input.try_recv() {
                    Ok(pcm) => batch.extend(pcm),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err(format!(
                            "音频设备已断开或读取失败：{}；请检查连接/权限后重新开始",
                            self.devices[index].name
                        ));
                    }
                }
            }
            if !batch.is_empty() {
                let mut levels = levels.lock().unwrap();
                let channel = &mut levels[if self.devices[index].kind == Kind::Input {
                    0
                } else {
                    1
                }];
                channel.meter = Meter::from_pcm(&batch);
                channel.last_packet = Some(Instant::now());
                inputs[index].extend(batch);
            }
        }
        Ok(())
    }
    fn stop(&mut self) {
        self.mic.stop();
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.system.stop();
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

enum Transport {
    OpenAi {
        audio: async_mpsc::Sender<Vec<u8>>,
        stop: async_mpsc::UnboundedSender<()>,
        events: mpsc::Receiver<openai::OpenAiEvent>,
        resampler: UpsamplerTo24k,
    },
    Qwen {
        audio: async_mpsc::Sender<(Vec<u8>, tokio::sync::OwnedSemaphorePermit)>,
        stop: async_mpsc::Sender<()>,
        events: mpsc::Receiver<qwen::QwenEvent>,
        budget: Arc<Semaphore>,
        generation: Arc<Mutex<u64>>,
        epoch: u64,
    },
    Soniox {
        audio: async_mpsc::Sender<Vec<u8>>,
        stop: async_mpsc::Sender<()>,
        events: mpsc::Receiver<soniox::Event>,
    },
}

impl Transport {
    fn start(
        name: &str,
        settings: &Settings,
        runtime: &Runtime,
    ) -> Result<(Self, JoinHandle<Result<(), String>>), String> {
        match name {
            "openai" => {
                let (audio, rx) = async_mpsc::channel(15);
                let (stop, stop_rx) = async_mpsc::unbounded_channel();
                let (tx, events) = mpsc::channel();
                let config = openai::OpenAiRealtimeConfig {
                    api_key: settings.openai_api_key.clone(),
                    source_language: settings.source_language.clone(),
                    target_language: settings.target_language.clone(),
                    voice: None,
                    proxy_url: settings.realtime_proxy_url.clone(),
                    proxy_token: settings.realtime_proxy_token.clone(),
                    audio_output: settings.openai_audio_output,
                };
                let resampler = UpsamplerTo24k::new()?;
                let task = runtime.spawn(openai::run_session(config, rx, stop_rx, tx));
                Ok((
                    Self::OpenAi {
                        audio,
                        stop,
                        events,
                        resampler,
                    },
                    task,
                ))
            }
            "qwen" => {
                let (audio, rx) = async_mpsc::channel(256);
                let (stop, stop_rx) = async_mpsc::channel(1);
                let (tx, events) = mpsc::channel();
                let config = qwen::QwenRealtimeConfig {
                    api_key: settings.qwen_api_key.clone(),
                    source_language: settings.source_language.clone(),
                    target_language: settings.target_language.clone(),
                    service: settings.qwen_service.clone(),
                };
                let generation = Arc::new(Mutex::new(0));
                let task = runtime.spawn(qwen::run_session_generation(
                    config,
                    rx,
                    stop_rx,
                    tx,
                    generation.clone(),
                ));
                Ok((
                    Self::Qwen {
                        audio,
                        stop,
                        events,
                        budget: Arc::new(Semaphore::new(qwen::MAX_QUEUED_AUDIO_BYTES)),
                        generation,
                        epoch: 0,
                    },
                    task,
                ))
            }
            _ => {
                let (audio, rx) = async_mpsc::channel(15);
                let (stop, stop_rx) = async_mpsc::channel(1);
                let (tx, events) = mpsc::channel();
                let task = runtime.spawn(soniox::run(settings.clone(), rx, stop_rx, tx));
                Ok((
                    Self::Soniox {
                        audio,
                        stop,
                        events,
                    },
                    task,
                ))
            }
        }
    }
    fn audio(&mut self, pcm: Vec<u8>) -> Result<(), String> {
        match self {
            Self::OpenAi {
                audio, resampler, ..
            } => {
                let pcm = resampler.push(&pcm)?;
                if !pcm.is_empty() {
                    audio
                        .try_send(pcm)
                        .map_err(|_| "OpenAI 音频队列已满或已关闭")?;
                }
            }
            Self::Qwen {
                audio,
                budget,
                generation,
                epoch,
                ..
            } => {
                let current = generation.lock().unwrap();
                if *current != *epoch {
                    return Ok(());
                }
                let permit = budget
                    .clone()
                    .try_acquire_many_owned(pcm.len() as u32)
                    .map_err(|_| "千问音频积压超过 3 秒")?;
                audio
                    .try_send((pcm, permit))
                    .map_err(|_| "千问音频队列已满或已关闭")?;
            }
            Self::Soniox { audio, .. } => audio
                .try_send(pcm)
                .map_err(|_| "Soniox 音频队列已满或已关闭")?,
        }
        Ok(())
    }
    fn stop(&self) {
        match self {
            Self::OpenAi { stop, .. } => {
                let _ = stop.send(());
            }
            Self::Qwen { stop, .. } | Self::Soniox { stop, .. } => {
                let _ = stop.try_send(());
            }
        }
    }
    fn poll(&mut self, tx: &mpsc::Sender<Event>) -> bool {
        let mut restarted = false;
        match self {
            Self::OpenAi { events, .. } => {
                for event in events.try_iter() {
                    if matches!(&event, openai::OpenAiEvent::Status { state, .. } if state == "ready")
                    {
                        let _ = tx.send(Event::Status("OpenAI · 服务已连接".into()));
                    }
                    let _ = tx.send(Event::OpenAi(event));
                }
            }
            Self::Qwen { events, epoch, .. } => {
                let pending: Vec<_> = events.try_iter().collect();
                let boundary = pending
                    .iter()
                    .rposition(|event| matches!(event, qwen::QwenEvent::Restarted { .. }))
                    .unwrap_or(0);
                for event in pending.into_iter().skip(boundary) {
                    if let qwen::QwenEvent::Restarted { generation } = &event {
                        *epoch = *generation;
                        restarted = true;
                        let _ = tx.send(Event::Status("千问 · 五分钟重开，等待新连接".into()));
                    }
                    if matches!(&event, qwen::QwenEvent::Status { state, .. } if state == "ready") {
                        let _ = tx.send(Event::Status("千问 · 服务已连接".into()));
                    }
                    let _ = tx.send(Event::Qwen(event));
                }
            }
            Self::Soniox { events, .. } => {
                for event in events.try_iter() {
                    let event = match event {
                        soniox::Event::Ready => Event::Status("Soniox · 服务已连接".into()),
                        soniox::Event::Text(value) => Event::Soniox(value),
                        soniox::Event::Boundary => Event::Boundary,
                    };
                    let _ = tx.send(event);
                }
            }
        }
        restarted
    }
}

fn run(
    settings: Settings,
    providers: Vec<&str>,
    runtime: Runtime,
    stop: Arc<AtomicBool>,
    tx: &mpsc::Sender<Event>,
    levels: &Mutex<[ChannelDiagnostics; 2]>,
) -> Result<(), String> {
    let mut capture = Capture::start(&settings, levels)?;
    let _ = tx.send(Event::Status("设备已打开 · 等待服务连接".into()));
    // At most two attempts per configured provider. A stop always wins over failover.
    for (attempt, name) in providers
        .iter()
        .cycle()
        .take(providers.len() * 2)
        .enumerate()
    {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let _ = tx.send(Event::Status(format!(
            "{name} · {}",
            if attempt == 0 {
                "正在连接"
            } else {
                "故障切换 / 重连"
            }
        )));
        let (mut transport, mut task) = Transport::start(name, &settings, &runtime)?;
        let mut inputs = vec![Vec::new(); capture.inputs.len()];
        let mut flush = Instant::now();
        let mut stopping = None;
        let mut error = None;
        loop {
            if *name == "qwen" && stop.load(Ordering::SeqCst) {
                capture.stop();
                task.abort();
                let _ = runtime.block_on(&mut task);
                let _ = tx.send(Event::Boundary);
                let _ = tx.send(Event::Status("已停止".into()));
                return Ok(());
            }
            if transport.poll(tx) {
                for (input, receiver) in inputs.iter_mut().zip(&capture.inputs) {
                    input.clear();
                    while receiver.try_recv().is_ok() {}
                }
                flush = Instant::now();
            }
            if task.is_finished() {
                break;
            }
            if stop.load(Ordering::SeqCst) && stopping.is_none() {
                capture.stop();
                // WASAPI/ScreenCaptureKit stop asynchronously. Wait for sender
                // teardown so their last callback is included, with a hard bound.
                if let Err(message) =
                    drain_capture(&capture.inputs, &mut inputs, Duration::from_secs(1))
                {
                    error = Some(message);
                }
                if let Err(message) = flush_inputs(&mut inputs, &mut transport) {
                    error = Some(message);
                }
                if let Transport::OpenAi {
                    audio, resampler, ..
                } = &mut transport
                {
                    match resampler.finish() {
                        Ok(pcm) if !pcm.is_empty() => {
                            if audio.try_send(pcm).is_err() {
                                error =
                                    Some("OpenAI 收尾音频队列已满或关闭，尾句可能不完整".into());
                            }
                        }
                        Err(message) => error = Some(message),
                        _ => {}
                    }
                }
                transport.stop();
                stopping = Some(Instant::now());
                let _ = tx.send(Event::Status("正在停止，等待尾句…".into()));
            }
            if stopping.is_some_and(|t| t.elapsed() >= Duration::from_secs(16)) {
                error = Some("停止等待超时，尾句可能不完整".into());
                task.abort();
                break;
            }
            if stopping.is_none() {
                capture.read(&mut inputs, levels)?;
            }
            if stopping.is_none()
                && flush.elapsed() >= Duration::from_millis(200)
                && inputs.iter().any(|input| !input.is_empty())
            {
                error = flush_inputs(&mut inputs, &mut transport).err();
                flush = Instant::now();
                if error.is_some() {
                    transport.stop();
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !task.is_finished() {
            transport.stop();
            let until = Instant::now();
            while !task.is_finished() && until.elapsed() < Duration::from_secs(16) {
                transport.poll(tx);
                std::thread::sleep(Duration::from_millis(20));
            }
            if !task.is_finished() {
                task.abort();
            }
        }
        if let Ok(Err(message)) = runtime.block_on(&mut task) {
            error = Some(message);
        }
        transport.poll(tx);
        let _ = tx.send(Event::Boundary);
        if stop.load(Ordering::SeqCst) {
            let _ = tx.send(Event::Status(error.unwrap_or("已停止".into())));
            return Ok(());
        }
        let _ = tx.send(Event::Status(
            error.unwrap_or("连接已结束，尝试备用服务".into()),
        ));
        // Discard audio from the outage; never replay old speech to another provider.
        for input in &capture.inputs {
            while input.try_recv().is_ok() {}
        }
    }
    Err("所有已配置服务均不可用，请检查网络、密钥或代理后重试".into())
}

fn drain_capture(
    receivers: &[mpsc::Receiver<Vec<u8>>],
    inputs: &mut [Vec<u8>],
    limit: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + limit;
    for (receiver, input) in receivers.iter().zip(inputs) {
        loop {
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(pcm) => input.extend(pcm),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err("音频设备停止超时，尾句可能不完整".into());
                }
            }
            if Instant::now() >= deadline {
                return Err("音频设备停止超时，尾句可能不完整".into());
            }
        }
    }
    Ok(())
}

fn flush_inputs(inputs: &mut [Vec<u8>], transport: &mut Transport) -> Result<(), String> {
    let buffer = mix_pcm(inputs);
    for input in inputs {
        input.clear();
    }
    // Keep 200 ms blocks so packet limits also bound bytes; retain a short tail.
    for block in buffer.chunks(6400) {
        transport.audio(block.to_vec())?;
    }
    Ok(())
}

/// Mix simultaneous PCM streams on the same timeline, not concatenated duration.
fn mix_pcm(inputs: &[Vec<u8>]) -> Vec<u8> {
    let length = inputs.iter().map(Vec::len).max().unwrap_or_default() / 2;
    (0..length)
        .flat_map(|index| {
            let sum: i32 = inputs
                .iter()
                .map(|input| {
                    input
                        .get(index * 2..index * 2 + 2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]) as i32)
                        .unwrap_or(0)
                })
                .sum();
            (sum.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn stop_includes_last_asynchronous_capture_callback_and_reports_stalled_device() {
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            tx.send(vec![3, 0]).unwrap();
        });
        let mut inputs = vec![vec![1, 0, 2, 0]];
        drain_capture(&[rx], &mut inputs, Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert_eq!(inputs[0], [1, 0, 2, 0, 3, 0]);
        let (_tx, rx) = mpsc::channel();
        assert!(
            drain_capture(&[rx], &mut inputs, Duration::from_millis(10))
                .unwrap_err()
                .contains("超时")
        );
    }

    #[tokio::test]
    async fn stop_sends_partial_capture_and_queued_audio_before_provider_finish() {
        for provider in ["openai"] {
            tokio::time::timeout(Duration::from_secs(3), async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let server = tokio::spawn(async move {
                    let mut socket =
                        tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                            .await
                            .unwrap();
                    let update = socket.next().await.unwrap().unwrap();
                    assert_eq!(
                        serde_json::from_str::<Value>(update.to_text().unwrap()).unwrap()["type"],
                        "session.update"
                    );
                    // Stop is already queued before acknowledgement. Audio must wait,
                    // then drain FIFO before the termination message.
                    socket
                        .send(Message::Text(
                            json!({"type":"session.updated"}).to_string().into(),
                        ))
                        .await
                        .unwrap();
                    let mut received = Vec::new();
                    loop {
                        let message = socket.next().await.unwrap().unwrap();
                        let value: Value =
                            serde_json::from_str(message.to_text().unwrap()).unwrap();
                        if value["type"]
                            == if provider == "openai" {
                                "session.close"
                            } else {
                                "session.finish"
                            }
                        {
                            break;
                        }
                        assert_eq!(
                            value["type"],
                            if provider == "openai" {
                                "session.input_audio_buffer.append"
                            } else {
                                "input_audio_buffer.append"
                            }
                        );
                        received.extend(B64.decode(value["audio"].as_str().unwrap()).unwrap());
                    }
                    let finished = if provider == "openai" {
                        "session.closed"
                    } else {
                        "session.finished"
                    };
                    socket
                        .send(Message::Text(json!({"type":finished}).to_string().into()))
                        .await
                        .unwrap();
                    received
                });
                let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
                    .await
                    .unwrap();
                let (audio, audio_rx) = async_mpsc::channel(15);
                let mut inputs = vec![vec![42, 0, 19, 0], vec![8, 0]];
                let expected;
                let result;
                {
                    let (stop, stop_rx) = async_mpsc::unbounded_channel();
                    let (tx, events) = mpsc::channel();
                    let mut transport = Transport::OpenAi {
                        audio: audio.clone(),
                        stop,
                        events,
                        resampler: UpsamplerTo24k::new().unwrap(),
                    };
                    // A preceding chunk has already reached the outbound queue.
                    audio.send(vec![3, 0, 4, 0]).await.unwrap();
                    flush_inputs(&mut inputs, &mut transport).unwrap();
                    if let Transport::OpenAi { resampler, .. } = &mut transport {
                        let tail = resampler.finish().unwrap();
                        assert!(
                            tail.chunks_exact(2)
                                .any(|b| i16::from_le_bytes([b[0], b[1]]) != 0)
                        );
                        expected = [vec![3, 0, 4, 0], tail.clone()].concat();
                        audio.send(tail).await.unwrap();
                    } else {
                        unreachable!()
                    }
                    transport.stop();
                    let cfg = openai::OpenAiRealtimeConfig {
                        api_key: String::new(),
                        source_language: "en".into(),
                        target_language: "zh".into(),
                        voice: None,
                        proxy_url: String::new(),
                        proxy_token: String::new(),
                        audio_output: false,
                    };
                    result = openai::run_connected(cfg, audio_rx, stop_rx, tx, socket).await;
                }
                result.unwrap();
                assert!(inputs.iter().all(Vec::is_empty));
                assert_eq!(server.await.unwrap(), expected);
            })
            .await
            .unwrap();
        }
    }

    #[test]
    fn concurrent_sources_mix_without_doubling_duration_or_overflowing() {
        let pcm = |v: &[i16]| v.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
        assert_eq!(
            mix_pcm(&[pcm(&[100, 30000, -30000]), pcm(&[-50, 10000, -10000])]),
            pcm(&[50, 32767, -32768])
        );
        assert_eq!(mix_pcm(&[pcm(&[123, 456]), vec![]]), pcm(&[123, 456]));
    }
    #[test]
    fn failover_does_not_silently_change_two_way_semantics() {
        let mut s = Settings::default();
        s.soniox_api_key = "test".into();
        s.openai_api_key = "test".into();
        s.qwen_api_key = "test".into();
        s.translation_mode = "qwen".into();
        assert_eq!(providers(&s, true), ["qwen", "soniox", "openai"]);
        assert_eq!(providers(&s, false), ["qwen"]);
        s.translation_type = "two_way".into();
        assert_eq!(providers(&s, true), ["soniox"]);
        assert!(providers(&s, false).is_empty());
    }
}
