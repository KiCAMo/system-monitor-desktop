//! Tauri command handlers — IPC layer between webview and the Rust backend.
//!
//! Each submodule mirrors a route group from the original FastAPI app
//! (`app/main.py`). Commands take `tauri::State<'_, AppState>` first, then
//! their JSON-deserialized arguments, and return `Result<T, String>` where T
//! is `Serialize`. The frontend invokes them via `@tauri-apps/api/core`'s
//! `invoke(name, args)`.

pub mod browse;
pub mod control;
pub mod history;
pub mod profiles;
pub mod pty;
pub mod settings;
