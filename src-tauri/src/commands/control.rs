//! Monitoring start/stop commands. Idempotent — safe to call repeatedly.

use crate::state::{self, AppState};

#[tauri::command]
pub async fn start_monitoring(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let mut inner = state.0.lock().await;
    state::start_all(&mut inner)
        .await
        .map_err(|e| e.to_string())?;
    Ok(inner.running)
}

#[tauri::command]
pub async fn stop_monitoring(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let mut inner = state.0.lock().await;
    state::stop_all(&mut inner)
        .await
        .map_err(|e| e.to_string())?;
    Ok(inner.running)
}
