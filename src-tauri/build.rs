fn main() {
    // screencapturekit crate requires linking to libswift_Concurrency.dylib
    // On macOS 15+, it's in the dyld shared cache at /usr/lib/swift/
    // The crate's build script adds @rpath references, but we need to ensure
    // the linker can resolve them. Adding /usr/lib/swift as rpath covers modern macOS.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }

    // Load repo-root .env (gitignored) so secrets stay out of source. Exposes
    // GOOGLE_FREE_TTS_KEY to the crate via option_env!. Missing .env is fine —
    // the Google Free provider degrades to "not configured".
    if target_os == "windows" {
        // Distributed Windows builds must not embed a developer's API key,
        // even when it is present in the build environment.
        println!("cargo:rustc-env=GOOGLE_FREE_TTS_KEY=");
    } else {
        load_env_secret("GOOGLE_FREE_TTS_KEY");
    }

    write_private_settings();
    tauri_build::build()
}

/// Explicit opt-in only. Never print credential values in Cargo directives/logs.
fn write_private_settings() {
    let variables = [
        ("realtime_proxy_token", "TRANSLATOR_TEAM_TOKEN"),
        ("soniox_api_key", "SONIOX_API_KEY"),
        ("openai_api_key", "OPENAI_API_KEY"),
        ("qwen_api_key", "DASHSCOPE_API_KEY"),
    ];
    for (_, variable) in variables {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let mut settings = serde_json::Map::new();
    if std::env::var_os("CARGO_FEATURE_PRIVATE_TEAM").is_some() {
        for (field, variable) in variables {
            let value = std::env::var(variable)
                .unwrap_or_default()
                .trim()
                .to_owned();
            settings.insert(field.into(), value.into());
        }
        let token = settings["realtime_proxy_token"].as_str().unwrap();
        assert!(
            (32..=128).contains(&token.len())
                && token
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
            "private-team requires a valid TRANSLATOR_TEAM_TOKEN"
        );
        settings.insert(
            "realtime_proxy_url".into(),
            "https://translate.xiaomao.chat".into(),
        );
    }
    let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap())
        .join("private-settings.json");
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap())
        .expect("Unable to write build settings");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("Unable to restrict build settings permissions");
    }
}

/// Read `KEY=VALUE` for `key` from ../.env and re-export it as a compile-time env var.
fn load_env_secret(key: &str) {
    println!("cargo:rerun-if-changed=../.env");
    println!("cargo:rerun-if-env-changed={}", key);
    // An explicitly-set process env var wins over .env (useful for CI).
    if std::env::var(key).is_ok() {
        return;
    }
    if let Ok(content) = std::fs::read_to_string("../.env") {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    let val = v.trim().trim_matches('"').trim_matches('\'');
                    // Skip empty values so option_env! yields None (clean "not configured")
                    // rather than Some("") which would issue a request with a blank key.
                    if !val.is_empty() {
                        println!("cargo:rustc-env={}={}", key, val);
                    }
                    return;
                }
            }
        }
    }
}
