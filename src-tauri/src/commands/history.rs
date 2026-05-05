//! History commands — list/get/inject/delete/clear stored matches.
//!
//! Inject re-pastes a saved error block into the live agent PTY, prefixed
//! with a node/container/timestamp label.

use crate::history::MatchRow;
use crate::state::AppState;

#[tauri::command]
pub async fn list_history(
    state: tauri::State<'_, AppState>,
    limit: Option<i64>,
    node: Option<String>,
    q: Option<String>,
    before_id: Option<i64>,
) -> Result<Vec<MatchRow>, String> {
    let history = {
        let inner = state.0.lock().await;
        inner.history.clone()
    };
    let lim = limit.unwrap_or(100).clamp(1, 500) as usize;
    history
        .list(lim, node.as_deref(), q.as_deref(), before_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_history(
    state: tauri::State<'_, AppState>,
    match_id: i64,
) -> Result<MatchRow, String> {
    let history = {
        let inner = state.0.lock().await;
        inner.history.clone()
    };
    history
        .get(match_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "match not found".into())
}

#[tauri::command]
pub async fn inject_history(
    state: tauri::State<'_, AppState>,
    match_id: i64,
) -> Result<(), String> {
    let (history, pty) = {
        let inner = state.0.lock().await;
        (inner.history.clone(), inner.pty.clone())
    };
    let row = history
        .get(match_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "match not found".to_string())?;
    let block = row.block.clone().unwrap_or_default();
    let label = format!(
        "{} ({}) @ {}",
        row.node,
        row.container.as_deref().unwrap_or("?"),
        row.ts
    );
    pty.inject_error(&label, &block)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_history(
    state: tauri::State<'_, AppState>,
    match_id: i64,
) -> Result<(), String> {
    let history = {
        let inner = state.0.lock().await;
        inner.history.clone()
    };
    history.delete(match_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_history(state: tauri::State<'_, AppState>) -> Result<i64, String> {
    let history = {
        let inner = state.0.lock().await;
        inner.history.clone()
    };
    history
        .clear()
        .await
        .map(|n| n as i64)
        .map_err(|e| e.to_string())
}
