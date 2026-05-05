//! Settings commands — get/save the active `AppConfig` and a small summary
//! used by the dashboard header.

use serde::Serialize;

use crate::config::{save_config, AppConfig};
use crate::state::{self, AppState};

#[tauri::command]
pub async fn get_settings(state: tauri::State<'_, AppState>) -> Result<AppConfig, String> {
    let inner = state.0.lock().await;
    Ok(inner.cfg.clone())
}

/// Persist the new config to disk and hot-reload the runtime. Returns the
/// `running` flag after reload so the UI can refresh its toggles.
#[tauri::command]
pub async fn save_settings(
    state: tauri::State<'_, AppState>,
    config: AppConfig,
) -> Result<bool, String> {
    let mut inner = state.0.lock().await;
    save_config(&inner.cfg_path, &config).map_err(|e| e.to_string())?;
    state::reload(&mut inner, config)
        .await
        .map_err(|e| e.to_string())?;
    Ok(inner.running)
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeSummary {
    pub name: String,
    pub host: String,
    pub container: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSummary {
    pub nodes: Vec<NodeSummary>,
    pub auto_submit: bool,
    pub auto_apply: bool,
    pub running: bool,
}

#[tauri::command]
pub async fn get_config_summary(
    state: tauri::State<'_, AppState>,
) -> Result<ConfigSummary, String> {
    let inner = state.0.lock().await;
    Ok(ConfigSummary {
        nodes: inner
            .cfg
            .nodes
            .iter()
            .map(|n| NodeSummary {
                name: n.name.clone(),
                host: n.host.clone(),
                container: n.container.clone(),
            })
            .collect(),
        auto_submit: inner.cfg.claude.auto_submit,
        auto_apply: inner.cfg.claude.auto_apply,
        running: inner.running,
    })
}
