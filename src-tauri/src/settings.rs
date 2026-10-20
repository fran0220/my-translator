use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// Translation term: source → target mapping for Soniox
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranslationTerm {
    pub source: String,
    pub target: String,
}

/// Custom context for Soniox — provides domain-specific hints
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct CustomContext {
    pub domain: Option<String>,
    pub translation_terms: Vec<TranslationTerm>,
}

/// App settings — persisted to JSON
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// Soniox API key
    pub soniox_api_key: String,
    /// OpenAI API key (for gpt-realtime-translate)
    pub openai_api_key: String,
    /// Alibaba Cloud DashScope API key (for Qwen LiveTranslate Flash)
    #[serde(default)]
    pub qwen_api_key: String,
    /// Qwen endpoint profile: China 3.8 or Singapore legacy.
    pub qwen_service: String,
    /// Optional authenticated WebSocket relay for Soniox and OpenAI.
    pub realtime_proxy_url: String,
    pub realtime_proxy_token: String,
    /// Source language: "auto" or ISO 639-1 code
    pub source_language: String,
    /// Target language: ISO 639-1 code
    pub target_language: String,
    pub translation_type: String,
    pub language_a: String,
    pub language_b: String,
    pub language_hints_strict: bool,
    pub endpoint_delay: u32,
    /// Audio source: "system" | "microphone" | "both"
    pub audio_source: String,
    /// Native Windows endpoint IDs. Empty resolves the default once at start.
    pub input_device_id: String,
    pub output_device_id: String,
    /// Overlay opacity: 0.0 - 1.0
    pub overlay_opacity: f64,
    /// Font size in px
    pub font_size: u32,
    /// Max transcript lines to display
    pub max_lines: u32,
    /// Whether to show original text alongside translation
    pub show_original: bool,
    /// Translation mode: "soniox" | "local" | "openai"
    pub translation_mode: String,
    /// Optional custom context for better transcription
    pub custom_context: Option<CustomContext>,
    /// ElevenLabs API key for TTS narration
    pub elevenlabs_api_key: String,
    /// Whether TTS narration is enabled
    pub tts_enabled: bool,
    /// TTS provider: "edge" | "microsoft" | "google-free" | "tiktok" | "google" | "elevenlabs"
    pub tts_provider: String,
    /// ElevenLabs voice ID
    pub tts_voice_id: String,
    /// TTS speed multiplier (Web Speech)
    pub tts_speed: f64,
    /// Edge TTS voice name
    pub edge_tts_voice: String,
    /// Edge TTS speed percentage
    pub edge_tts_speed: i32,
    /// Auto-read new translations aloud
    pub tts_auto_read: bool,
    /// Google Cloud TTS API key
    pub google_tts_api_key: String,
    /// Google TTS voice name
    pub google_tts_voice: String,
    /// Google TTS speaking rate
    pub google_tts_speed: f64,
    /// Microsoft v2 (Edge endpoint, dynamic voice list) selected voice
    pub microsoft_v2_voice: String,
    /// Microsoft v2 speed percentage (reuses edge synth)
    pub microsoft_v2_speed: i32,
    /// Google Free (android-tts) language token, e.g. "vi-VN" | "en-US"
    pub google_free_voice: String,
    /// Optional user Google API key for Google Free. When set, overrides the build-time
    /// GOOGLE_FREE_TTS_KEY. Empty → fall back to the build-time key (if any).
    #[serde(default)]
    pub google_free_api_key: String,
    /// Google Free client-side playback speed (endpoint has no rate param). 1.0 = normal.
    #[serde(default = "default_local_tts_speed")]
    pub google_free_speed: f32,
    /// TikTok TTS speaker code, e.g. "BV074_streaming"
    pub tiktok_voice: String,
    /// TikTok client-side playback speed (endpoint has no rate param). 1.0 = normal.
    #[serde(default = "default_local_tts_speed")]
    pub tiktok_speed: f32,
    /// TikTok sessionid cookie (user-supplied; required by the endpoint)
    pub tiktok_session_id: String,
    /// Local offline (Piper/sherpa-onnx) selected voice id, e.g. "vi_VN-vais1000-medium"
    #[serde(default)]
    pub local_tts_voice: String,
    /// Local offline TTS speed (1.0 = normal). Piper length-scale is applied inversely.
    #[serde(default = "default_local_tts_speed")]
    pub local_tts_speed: f32,
    /// Folder where local TTS models are stored; empty = default app-data location.
    #[serde(default)]
    pub local_tts_models_dir: String,
    /// OpenAI Realtime: when true server generates translated audio.
    /// Default false — speaker → mic feedback loop on shared devices.
    #[serde(default)]
    pub openai_audio_output: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let bundled: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/private-settings.json"
        )))
        .expect("Invalid bundled settings");
        let preset = |key: &str| bundled[key].as_str().unwrap_or_default().to_owned();
        Self {
            soniox_api_key: preset("soniox_api_key"),
            openai_api_key: preset("openai_api_key"),
            qwen_api_key: preset("qwen_api_key"),
            qwen_service: "qianwen".to_string(),
            realtime_proxy_url: preset("realtime_proxy_url"),
            realtime_proxy_token: preset("realtime_proxy_token"),
            source_language: "en".to_string(),
            target_language: "zh".to_string(),
            translation_type: "one_way".to_string(),
            language_a: "en".to_string(),
            language_b: "zh".to_string(),
            language_hints_strict: false,
            endpoint_delay: 3000,
            audio_source: "system".to_string(),
            input_device_id: String::new(),
            output_device_id: String::new(),
            overlay_opacity: 0.85,
            font_size: 16,
            max_lines: 5,
            show_original: true,
            translation_mode: "soniox".to_string(),
            custom_context: None,
            elevenlabs_api_key: String::new(),
            tts_enabled: false,
            tts_provider: "edge".to_string(),
            tts_voice_id: "21m00Tcm4TlvDq8ikWAM".to_string(),
            tts_speed: 1.2,
            edge_tts_voice: "vi-VN-HoaiMyNeural".to_string(),
            edge_tts_speed: 50,
            tts_auto_read: true,
            google_tts_api_key: String::new(),
            google_tts_voice: "vi-VN-Chirp3-HD-Aoede".to_string(),
            google_tts_speed: 1.0,
            microsoft_v2_voice: "vi-VN-HoaiMyNeural".to_string(),
            microsoft_v2_speed: 20,
            google_free_voice: "vi-VN".to_string(),
            google_free_api_key: String::new(),
            google_free_speed: 1.0,
            tiktok_voice: "BV074_streaming".to_string(),
            tiktok_speed: 1.0,
            tiktok_session_id: String::new(),
            local_tts_voice: "vi_VN-vais1000-medium".to_string(),
            local_tts_speed: 1.0,
            local_tts_models_dir: String::new(),
            openai_audio_output: false,
        }
    }
}

/// Serde default for `local_tts_speed` (field-level default would give 0.0).
fn default_local_tts_speed() -> f32 {
    1.0
}

/// Get the settings file path
/// ~/Library/Application Support/com.personal.translator/settings.json
#[cfg(feature = "tauri-host")]
fn settings_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("com.personal.translator");
    // A private one-off build must not inherit blank or unrelated public relay settings.
    path.push(if cfg!(feature = "private-team") {
        "settings-team.json"
    } else {
        "settings.json"
    });
    path
}

#[cfg(feature = "tauri-host")]
impl Settings {
    /// Load settings from disk, or return defaults
    pub fn load() -> Self {
        let path = settings_path();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                Err(_) => Self::default(),
            }
        } else {
            Self::default()
        }
    }

    /// Save settings to disk
    pub fn save(&self) -> Result<(), String> {
        let path = settings_path();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {}", e))?;
        }

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize: {}", e))?;

        fs::write(&path, json).map_err(|e| format!("Failed to write settings: {}", e))?;

        Ok(())
    }
}

/// Thread-safe settings state managed by Tauri
pub struct SettingsState(pub Mutex<Settings>);

/// Native-only persistence. Resolve once so reads and writes cannot diverge if
/// the environment changes. Never inspect or migrate the Tauri configuration.
#[cfg(not(feature = "tauri-host"))]
pub struct NativeSettingsStore {
    path: PathBuf,
}

#[cfg(not(feature = "tauri-host"))]
impl NativeSettingsStore {
    pub fn open() -> Result<(Self, Settings), String> {
        Self::open_at(
            std::env::var_os("TRANSLATOR_NATIVE_CONFIG_DIR"),
            dirs::config_dir,
        )
    }

    fn open_at(
        override_dir: Option<std::ffi::OsString>,
        config_root: impl FnOnce() -> Option<PathBuf>,
    ) -> Result<(Self, Settings), String> {
        let directory = match override_dir {
            Some(value) => {
                let directory = PathBuf::from(value);
                if directory.as_os_str().is_empty() || !directory.is_absolute() {
                    return Err(
                        "TRANSLATOR_NATIVE_CONFIG_DIR 必须是非空绝对目录；修正后重启应用".into(),
                    );
                }
                directory
            }
            None => config_root()
                .filter(|path| path.is_absolute())
                .ok_or("无法确定原生配置目录；请设置 TRANSLATOR_NATIVE_CONFIG_DIR 后重启")?
                .join("com.personal.translator.native"),
        };
        let store = Self {
            path: directory.join(if cfg!(feature = "private-team") {
                "settings-team.json"
            } else {
                "settings.json"
            }),
        };
        let settings = store.read()?;
        Ok((store, settings))
    }

    fn read(&self) -> Result<Settings, String> {
        let directory = self.path.parent().expect("settings has a parent");
        match fs::metadata(directory) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err("原生配置路径不是目录；修正后重启应用".into())
            }
            Ok(_) => {
                fs::read_dir(directory).map_err(|_| "原生配置目录不可读；修正权限后重启应用")?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法访问原生配置目录；修正后重启应用".into()),
        }
        match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(_) => Err("无法访问原生配置文件；修正后重启应用".into()),
            Ok(_) => {
                let content = fs::read_to_string(&self.path)
                    .map_err(|_| "原生配置文件不可读；未覆盖文件，请修复后重启应用")?;
                serde_json::from_str(&content)
                    .map_err(|_| "原生配置文件损坏；未覆盖文件，请修复后重启应用".into())
            }
        }
    }

    pub fn save(&self, settings: &Settings) -> Result<(), String> {
        // Refuse to overwrite a file that became unreadable/corrupt since startup.
        self.read()?;
        let directory = self.path.parent().expect("settings has a parent");
        fs::create_dir_all(directory).map_err(|_| "无法创建原生配置目录；请修正后重启应用")?;
        let json = serde_json::to_vec_pretty(settings).map_err(|_| "无法序列化原生配置")?;
        let temporary = self.path.with_extension("json.tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|_| "无法创建原生配置临时文件；请修正后重启应用")?;
        let result = (|| {
            use std::io::Write;
            file.write_all(&json)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result.map_err(|_| "保存原生配置失败；请检查目录权限后重启应用".into())
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[cfg(not(feature = "tauri-host"))]
    mod native {
        use super::super::{NativeSettingsStore, Settings};
        use std::{
            fs,
            path::PathBuf,
            sync::atomic::{AtomicUsize, Ordering},
        };

        struct Temp(PathBuf);
        impl Temp {
            fn new() -> Self {
                static NEXT: AtomicUsize = AtomicUsize::new(0);
                let path = std::env::temp_dir().join(format!(
                    "translator-config-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                fs::create_dir(&path).unwrap();
                Self(path)
            }
            fn open(&self) -> Result<(NativeSettingsStore, Settings), String> {
                NativeSettingsStore::open_at(Some(self.0.clone().into_os_string()), || {
                    panic!("explicit override must not consult OS config root")
                })
            }
        }
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn separate_default_and_override_roundtrip_without_tauri_migration() {
            let root = Temp::new();
            let tauri = root.0.join("com.personal.translator");
            fs::create_dir(&tauri).unwrap();
            fs::write(
                tauri.join("settings.json"),
                b"not valid JSON -- must not be read or overwritten",
            )
            .unwrap();
            let (store, mut settings) =
                NativeSettingsStore::open_at(None, || Some(root.0.clone())).unwrap();
            assert_eq!(
                store.path.parent().unwrap(),
                root.0.join("com.personal.translator.native")
            );
            assert!(settings.soniox_api_key.is_empty());
            settings.source_language = "zh".into();
            settings.target_language = "en".into();
            store.save(&settings).unwrap();
            let (_, restored) =
                NativeSettingsStore::open_at(None, || Some(root.0.clone())).unwrap();
            assert_eq!(
                (
                    restored.source_language.as_str(),
                    restored.target_language.as_str()
                ),
                ("zh", "en")
            );
            assert_eq!(
                fs::read(tauri.join("settings.json")).unwrap(),
                b"not valid JSON -- must not be read or overwritten"
            );

            let override_dir = Temp::new();
            let (store, mut settings) = override_dir.open().unwrap();
            settings.font_size = 27;
            settings.input_device_id = "capture-stable-id".into();
            settings.output_device_id = "render-stable-id".into();
            store.save(&settings).unwrap();
            settings.font_size = 31;
            store.save(&settings).unwrap(); // Replaces existing file on Windows too.
            assert_eq!(override_dir.open().unwrap().1.font_size, 31);
            let restored = override_dir.open().unwrap().1;
            assert_eq!(restored.input_device_id, "capture-stable-id");
            assert_eq!(restored.output_device_id, "render-stable-id");
            assert_eq!(fs::read_dir(&override_dir.0).unwrap().count(), 1);
        }

        #[test]
        fn invalid_overrides_and_unavailable_roots_never_fall_back() {
            for value in ["", "relative", " "] {
                assert!(NativeSettingsStore::open_at(Some(value.into()), || panic!(
                    "no fallback permitted"
                ))
                .is_err());
            }
            assert!(NativeSettingsStore::open_at(None, || None).is_err());
            let root = Temp::new();
            let file = root.0.join("file");
            fs::write(&file, b"unchanged").unwrap();
            assert!(
                NativeSettingsStore::open_at(Some(file.clone().into_os_string()), || panic!(
                    "no fallback"
                ))
                .is_err()
            );
            assert_eq!(fs::read(&file).unwrap(), b"unchanged");
        }

        #[test]
        fn corrupt_or_unreadable_configuration_is_not_replaced() {
            let root = Temp::new();
            let (store, settings) = root.open().unwrap();
            for content in [b"{broken".as_slice(), b"null", &[0xff, 0xfe]] {
                fs::write(&store.path, content).unwrap();
                assert!(root.open().is_err());
                assert!(store.save(&settings).is_err());
                assert_eq!(fs::read(&store.path).unwrap(), content);
            }
            fs::remove_file(&store.path).unwrap();
            fs::create_dir(&store.path).unwrap();
            assert!(root.open().is_err());
            assert!(store.save(&settings).is_err());
            assert!(store.path.is_dir());
        }

        #[cfg(unix)]
        #[test]
        fn unreadable_file_is_an_error_even_when_metadata_is_available() {
            let root = Temp::new();
            let (store, settings) = root.open().unwrap();
            // A self-link reliably fails reads even when tests run as root.
            std::os::unix::fs::symlink(&store.path, &store.path).unwrap();
            assert!(root.open().is_err());
            assert!(store.save(&settings).is_err());
            assert!(fs::symlink_metadata(&store.path)
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    #[test]
    fn bilingual_settings_survive_serialization_and_legacy_defaults() {
        let legacy: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.source_language, "en");
        assert_eq!(legacy.target_language, "zh");
        assert_eq!(legacy.qwen_service, "qianwen");
        if cfg!(feature = "private-team") {
            assert_eq!(legacy.realtime_proxy_url, "https://translate.xiaomao.chat");
            assert!(!legacy.realtime_proxy_token.is_empty());
        } else {
            assert!(legacy.realtime_proxy_url.is_empty());
            assert!(legacy.realtime_proxy_token.is_empty());
            assert!(legacy.soniox_api_key.is_empty());
            assert!(legacy.openai_api_key.is_empty());
            assert!(legacy.qwen_api_key.is_empty());
        }
        let settings: Settings = serde_json::from_str(
            r#"{
            "source_language":"zh", "target_language":"en",
            "soniox_api_key":"user-override-soniox", "openai_api_key":"user-override-openai",
            "qwen_api_key":"user-override-qwen",
            "translation_type":"two_way", "language_a":"zh", "language_b":"en",
            "language_hints_strict":true, "endpoint_delay":1500,
            "qwen_service":"dashscope_intl",
            "realtime_proxy_url":"https://relay.example.com",
            "realtime_proxy_token":"test-only-team-token-0123456789abcdef"
        }"#,
        )
        .unwrap();
        let saved = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.source_language, "zh");
        assert_eq!(restored.target_language, "en");
        assert_eq!(restored.translation_type, "two_way");
        assert_eq!(restored.language_a, "zh");
        assert_eq!(restored.language_b, "en");
        assert!(restored.language_hints_strict);
        assert_eq!(restored.endpoint_delay, 1500);
        assert_eq!(restored.qwen_service, "dashscope_intl");
        assert!(restored.soniox_api_key == "user-override-soniox");
        assert!(restored.openai_api_key == "user-override-openai");
        assert!(restored.qwen_api_key == "user-override-qwen");
        assert_eq!(restored.realtime_proxy_url, "https://relay.example.com");
        assert_eq!(
            restored.realtime_proxy_token,
            "test-only-team-token-0123456789abcdef"
        );
    }
}
