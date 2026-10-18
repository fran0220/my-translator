fn main() {
    let mut settings = serde_json::Map::new();
    // Public builds never read credentials, including from .env or the process environment.
    if std::env::var_os("CARGO_FEATURE_PRIVATE_TEAM").is_some() {
        for (field, variable) in [
            ("realtime_proxy_token", "TRANSLATOR_TEAM_TOKEN"),
            ("soniox_api_key", "SONIOX_API_KEY"),
            ("openai_api_key", "OPENAI_API_KEY"),
            ("qwen_api_key", "DASHSCOPE_API_KEY"),
        ] {
            println!("cargo:rerun-if-env-changed={variable}");
            settings.insert(
                field.into(),
                std::env::var(variable).unwrap_or_default().trim().into(),
            );
        }
        let token = settings["realtime_proxy_token"].as_str().unwrap();
        assert!(
            (32..=128).contains(&token.len())
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "private-team requires a valid team token"
        );
        settings.insert(
            "realtime_proxy_url".into(),
            "https://translate.xiaomao.chat".into(),
        );
    }
    let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap())
        .join("private-settings.json");
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}
