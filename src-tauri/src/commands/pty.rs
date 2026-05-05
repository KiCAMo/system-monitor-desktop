//! PTY commands — forward keystrokes / pastes / resize events from the
//! webview's xterm.js to the agent CLI's PTY.

use crate::state::AppState;

#[tauri::command]
pub async fn pty_input(
    state: tauri::State<'_, AppState>,
    text: String,
    submit: bool,
) -> Result<(), String> {
    let pty = {
        let inner = state.0.lock().await;
        inner.pty.clone()
    };
    pty.send_user_input(&text, submit)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pty_raw(state: tauri::State<'_, AppState>, data: String) -> Result<(), String> {
    let pty = {
        let inner = state.0.lock().await;
        inner.pty.clone()
    };
    pty.send_raw(&data).await;
    Ok(())
}

#[tauri::command]
pub async fn pty_resize(
    state: tauri::State<'_, AppState>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let pty = {
        let inner = state.0.lock().await;
        inner.pty.clone()
    };
    pty.resize(cols, rows).await;
    Ok(())
}
