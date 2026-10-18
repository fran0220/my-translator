// Compile the same transport/audio implementations without the Tauri host adapter.
// There is no Tauri, Wry, browser or JavaScript runtime in this dependency graph.
#[path = "../../src-tauri/src/audio/mod.rs"]
pub mod audio;
#[path = "../../src-tauri/src/commands/openai_realtime.rs"]
pub mod openai;
#[path = "../../src-tauri/src/commands/qwen_realtime.rs"]
pub mod qwen;
pub mod session;
#[path = "../../src-tauri/src/settings.rs"]
pub mod settings;
pub mod soniox;
pub mod transcript;
