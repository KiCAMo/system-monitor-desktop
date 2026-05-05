//! Application entry point. Wires plugins, builds the singleton `AppState`,
//! spawns the broadcast→Tauri-event relay, and registers every IPC command.

pub mod commands;
pub mod config;
pub mod events;
pub mod history;
pub mod metrics;
pub mod pty_session;
pub mod ssh_monitor;
pub mod state;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{Manager, path::BaseDirectory};

use crate::config::{save_config, AppConfig};
use crate::events::EventBus;
use crate::history::HistoryStore;
use crate::state::AppState;

/// Resolve the per-user data directory the app writes config / history /
/// profiles into. macOS = `~/Library/Application Support/<bundle id>/`.
/// Falls back to cwd when running under tests / unbundled binary.
fn resolve_data_dir(app: &tauri::App) -> PathBuf {
    if let Ok(p) = app.path().resolve("", BaseDirectory::AppData) {
        return p;
    }
    PathBuf::from(".")
}

fn cfg_path_for(data_dir: &PathBuf) -> PathBuf {
    if let Ok(p) = std::env::var("DDODOLI_CONFIG") {
        return PathBuf::from(p);
    }
    data_dir.join("config.yaml")
}

fn history_db_path_for(data_dir: &PathBuf) -> String {
    if let Ok(p) = std::env::var("DDODOLI_HISTORY_DB") {
        return p;
    }
    data_dir.join("history.db").to_string_lossy().into_owned()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,system_monitor=debug".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Resolve per-user data directory + ensure it exists. Without this
            // the app would try to write config.yaml/history.db to its cwd
            // (often /Applications/... read-only) and abort on first launch.
            let data_dir = resolve_data_dir(app);
            std::fs::create_dir_all(&data_dir).ok();

            // Profiles also live under data_dir; tell the profiles command
            // module where to look via env so we don't need a global.
            std::env::set_var(
                "DDODOLI_PROFILES_DIR",
                data_dir.join("profiles").to_string_lossy().to_string(),
            );

            let cfg_path = cfg_path_for(&data_dir);
            // First-run bootstrap: write a default AppConfig so load_config
            // doesn't fail. The user fills in nodes/keys via the settings UI.
            if !cfg_path.exists() {
                let default_cfg = AppConfig::default();
                if let Err(e) = save_config(&cfg_path, &default_cfg) {
                    tracing::warn!(
                        "failed to bootstrap default config at {}: {e:?}",
                        cfg_path.display()
                    );
                }
            }

            // Build singletons (bus + history) outside the AppState mutex —
            // they survive every reload.
            let bus = EventBus::new();
            let history = Arc::new(
                HistoryStore::new(history_db_path_for(&data_dir))
                    .map_err(|e| format!("open history db: {e:?}"))?,
            );

            // Async init: build Inner with the loaded config.
            let inner = tauri::async_runtime::block_on(state::initial(
                cfg_path,
                bus.clone(),
                history,
            ))
            .map_err(|e| format!("AppState init: {e:?}"))?;

            app.manage(AppState::new(inner));

            // Subscribe BEFORE spawning the relay so we don't miss early
            // events emitted during startup. The test-mode `spawn_event_relay`
            // takes `()` for the handle, so we only call it in non-test builds.
            let rx = bus.subscribe();
            #[cfg(not(test))]
            {
                let app_handle = app.handle().clone();
                state::spawn_event_relay(app_handle, rx);
            }
            #[cfg(test)]
            {
                let _ = rx;
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::get_config_summary,
            commands::profiles::list_profiles,
            commands::profiles::save_profile,
            commands::profiles::load_profile,
            commands::profiles::delete_profile,
            commands::history::list_history,
            commands::history::get_history,
            commands::history::inject_history,
            commands::history::delete_history,
            commands::history::clear_history,
            commands::pty::pty_input,
            commands::pty::pty_raw,
            commands::pty::pty_resize,
            commands::browse::browse_fs,
            commands::control::start_monitoring,
            commands::control::stop_monitoring,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
