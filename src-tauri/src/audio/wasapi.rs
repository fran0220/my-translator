//! WASAPI shared-mode input and render-loopback, selected by opaque endpoint ID.
use super::{
    devices::{DeviceInfo, Kind},
    TARGET_SAMPLE_RATE,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};
use windows::{
    core::{Interface, BSTR, PCWSTR},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::{Endpoints::IAudioEndpointVolume, *},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
            COINIT_MULTITHREADED, STGM_READ,
        },
    },
};

struct Com;
impl Com {
    fn new() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }
            .map_err(|_| "无法初始化音频设备线程")?;
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
unsafe fn enumerator() -> Result<IMMDeviceEnumerator, String> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|_| "无法枚举 Windows 音频端点".into())
}
unsafe fn id(device: &IMMDevice) -> Result<String, String> {
    let raw = unsafe { device.GetId() }.map_err(|_| "无法读取设备 ID")?;
    let value = unsafe { raw.to_string() };
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    value.map_err(|_| "音频设备 ID 编码错误".into())
}

pub fn enumerate() -> Result<Vec<DeviceInfo>, String> {
    let _com = Com::new()?;
    unsafe {
        let enumerator = enumerator()?;
        let mut result = vec![];
        for (flow, kind) in [(eCapture, Kind::Input), (eRender, Kind::Output)] {
            let default = enumerator
                .GetDefaultAudioEndpoint(flow, eConsole)
                .ok()
                .and_then(|d| id(&d).ok());
            let devices = enumerator
                .EnumAudioEndpoints(flow, DEVICE_STATE(DEVICE_STATEMASK_ALL))
                .map_err(|_| "无法枚举设备列表")?;
            for index in 0..devices.GetCount().map_err(|_| "无法读取设备数量")? {
                let device = devices.Item(index).map_err(|_| "无法读取设备")?;
                let id = id(&device)?;
                let name = device
                    .OpenPropertyStore(STGM_READ)
                    .ok()
                    .and_then(|p| p.GetValue(&PKEY_Device_FriendlyName).ok())
                    .and_then(|v| BSTR::try_from(&v).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "名称不可读".into());
                let state = device.GetState().map_err(|_| "无法读取设备状态")?;
                let muted = device
                    .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                    .ok()
                    .and_then(|v| v.GetMute().ok())
                    .map(|v| v.as_bool());
                result.push(DeviceInfo {
                    default: default.as_ref() == Some(&id),
                    id,
                    name,
                    kind,
                    available: state == DEVICE_STATE_ACTIVE,
                    state: match state {
                        DEVICE_STATE_ACTIVE => "可用",
                        DEVICE_STATE_DISABLED => "已禁用",
                        DEVICE_STATE_UNPLUGGED => "未插入",
                        _ => "离线",
                    }
                    .into(),
                    muted,
                });
            }
        }
        Ok(result)
    }
}

pub struct SystemAudioCapture {
    is_capturing: Arc<AtomicBool>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl SystemAudioCapture {
    pub fn new() -> Self {
        Self {
            is_capturing: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
        }
    }
    /// Compatibility entry: default render endpoint resolved once per start.
    pub fn start(&self) -> Result<mpsc::Receiver<Vec<u8>>, String> {
        self.start_selected("", Kind::Output)
    }
    pub fn start_selected(
        &self,
        endpoint: &str,
        kind: Kind,
    ) -> Result<mpsc::Receiver<Vec<u8>>, String> {
        let mut worker = self.worker.lock().unwrap();
        if worker.as_ref().is_some_and(|task| !task.is_finished()) {
            return Err("上次采集尚未释放，请稍后重试".into());
        }
        if let Some(task) = worker.take() {
            let _ = task.join();
        }
        if self.is_capturing.swap(true, Ordering::SeqCst) {
            return Err("音频设备已在采集中".into());
        }
        let (sender, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let flag = self.is_capturing.clone();
        let endpoint = endpoint.to_owned();
        *worker = Some(std::thread::spawn(move || {
            let result = capture(&endpoint, kind, &flag, sender, &ready_tx);
            if let Err(error) = result {
                let _ = ready_tx.try_send(Err(error));
            }
            flag.store(false, Ordering::SeqCst);
        }));
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(receiver),
            Ok(Err(error)) => {
                self.stop();
                Err(error)
            }
            Err(_) => {
                self.is_capturing.store(false, Ordering::SeqCst);
                Err("打开音频设备超时".into())
            }
        }
    }
    pub fn stop(&self) {
        self.is_capturing.store(false, Ordering::SeqCst);
    }
    pub fn is_capturing(&self) -> bool {
        self.is_capturing.load(Ordering::SeqCst)
    }
}
impl Default for SystemAudioCapture {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for SystemAudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn capture(
    endpoint: &str,
    kind: Kind,
    flag: &AtomicBool,
    sender: mpsc::Sender<Vec<u8>>,
    ready: &mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let _com = Com::new()?;
    unsafe {
        let enumerator = enumerator()?;
        let flow = if kind == Kind::Input {
            eCapture
        } else {
            eRender
        };
        let device = if endpoint.is_empty() {
            enumerator.GetDefaultAudioEndpoint(flow, eConsole)
        } else {
            let wide: Vec<u16> = endpoint.encode_utf16().chain(Some(0)).collect();
            enumerator.GetDevice(PCWSTR(wide.as_ptr()))
        }
        .map_err(|_| "所选设备已离线或不存在；不会切换其他设备")?;
        if device.GetState().map_err(|_| "无法读取设备状态")? != DEVICE_STATE_ACTIVE
            || device
                .cast::<IMMEndpoint>()
                .map_err(|_| "无法确认设备方向")?
                .GetDataFlow()
                .map_err(|_| "无法确认设备方向")?
                != flow
        {
            return Err("设备不可用或输入/输出方向不匹配".into());
        }
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|_| "无法打开所选音频设备")?;
        let format = client.GetMixFormat().map_err(|_| "无法读取设备音频格式")?;
        let base = *format;
        let tag = if base.wFormatTag == 0xfffe && base.cbSize >= 22 {
            (*(format as *const WAVEFORMATEXTENSIBLE)).SubFormat.data1
        } else {
            base.wFormatTag as u32
        };
        let init = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            if kind == Kind::Output {
                AUDCLNT_STREAMFLAGS_LOOPBACK
            } else {
                0
            },
            1_000_000,
            0,
            format,
            None,
        );
        CoTaskMemFree(Some(format.cast()));
        init.map_err(|_| "无法初始化所选音频设备；请检查权限和独占占用")?;
        if base.nChannels == 0
            || base.nSamplesPerSec == 0
            || !matches!(
                (tag, base.wBitsPerSample),
                (3, 32) | (1, 16) | (1, 24) | (1, 32)
            )
        {
            return Err("音频设备格式不受支持（需要 float32 或 PCM16/24/32）".into());
        }
        let capture: IAudioCaptureClient = client.GetService().map_err(|_| "无法初始化音频采集")?;
        client.Start().map_err(|_| "无法启动音频设备")?;
        let _ = ready.send(Ok(()));
        let result = (|| -> Result<(), String> {
            let mut position = 0f64;
            while flag.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
                while capture.GetNextPacketSize().map_err(|_| "音频设备已断开")? > 0 {
                    let mut data = std::ptr::null_mut();
                    let mut frames = 0;
                    let mut flags = 0;
                    capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .map_err(|_| "音频设备读取失败")?;
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    let mut pcm = vec![];
                    let stride = base.wBitsPerSample as usize / 8;
                    while position < frames as f64 {
                        let frame = position as usize;
                        let mut sample = 0f32;
                        if !silent && !data.is_null() {
                            for channel in 0..base.nChannels as usize {
                                let p =
                                    data.add((frame * base.nChannels as usize + channel) * stride);
                                sample += match (tag, stride) {
                                    (3, 4) => std::ptr::read_unaligned(p.cast::<f32>()),
                                    (1, 2) => {
                                        std::ptr::read_unaligned(p.cast::<i16>()) as f32 / 32768.
                                    }
                                    (1, 3) => {
                                        ((i32::from_le_bytes([*p, *p.add(1), *p.add(2), 0]) << 8)
                                            >> 8) as f32
                                            / 8388608.
                                    }
                                    _ => {
                                        std::ptr::read_unaligned(p.cast::<i32>()) as f32
                                            / 2147483648.
                                    }
                                };
                            }
                            sample /= base.nChannels as f32;
                        }
                        pcm.extend_from_slice(
                            &((sample.clamp(-1., 1.) * 32767.) as i16).to_le_bytes(),
                        );
                        position += base.nSamplesPerSec as f64 / TARGET_SAMPLE_RATE as f64;
                    }
                    position -= frames as f64;
                    capture
                        .ReleaseBuffer(frames)
                        .map_err(|_| "音频设备缓冲释放失败")?;
                    if !pcm.is_empty() && sender.send(pcm).is_err() {
                        return Ok(());
                    }
                    if !flag.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
            Ok(())
        })();
        let _ = client.Stop();
        result
    }
}
