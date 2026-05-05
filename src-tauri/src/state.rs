//! Runtime container — Rust port of the `AppState` class in
//! `app/main.py`.
//!
//! Aggregates every long-lived backend object (config, history store, event
//! bus, monitors, pollers, agent PTY) under a single `tokio::sync::Mutex` so
//! that Tauri commands can safely mutate it from async contexts and so that a
//! settings-save / profile-load can hot-reload everything in place without
//! tearing down the broadcast bus or the SQLite connection.
//!
//! The `EventBus` and `HistoryStore` survive reload — they're injected into
//! `initial()` from the outside (`run()` constructs them once) — while every
//! other field is rebuilt from the new `AppConfig`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::config::{load_config, AppConfig, NodeConfig};
use crate::events::{Event, EventBus};
use crate::history::HistoryStore;
use crate::metrics::MetricsPoller;
use crate::pty_session::PtySession;
use crate::ssh_monitor::{DockerLogMonitor, OnErrorCallback};

/// Mutable runtime state. Held inside `AppState`'s mutex.
pub struct Inner {
    pub cfg: AppConfig,
    pub cfg_path: PathBuf,
    pub running: bool,
    pub monitor_handles: Vec<JoinHandle<()>>,
    pub poller_handles: Vec<JoinHandle<()>>,
    /// Kept so we can call `.stop()` (cancels the cancellation token) before
    /// awaiting the matching join handle in `stop_all`.
    pub monitors: Vec<Arc<DockerLogMonitor>>,
    pub pollers: Vec<Arc<MetricsPoller>>,
    pub pty: Arc<PtySession>,
    pub history: Arc<HistoryStore>,
    pub bus: EventBus,
}

/// Tauri-managed handle. `app.manage(AppState(...))` and every
/// `tauri::command` receives `tauri::State<'_, AppState>`.
pub struct AppState(pub Arc<Mutex<Inner>>);

impl AppState {
    pub fn new(inner: Inner) -> Self {
        Self(Arc::new(Mutex::new(inner)))
    }
}

/// Build the initial `Inner`. Doesn't spawn anything — call `start_all` after.
///
/// `bus` and `history` are injected so they survive subsequent reloads.
pub async fn initial(
    cfg_path: PathBuf,
    bus: EventBus,
    history: Arc<HistoryStore>,
) -> Result<Inner> {
    let cfg = load_config(&cfg_path)?;
    let pty = Arc::new(PtySession::new(
        cfg.claude.clone(),
        cfg.project.clone(),
        bus.clone(),
    ));
    Ok(Inner {
        cfg,
        cfg_path,
        running: false,
        monitor_handles: Vec::new(),
        poller_handles: Vec::new(),
        monitors: Vec::new(),
        pollers: Vec::new(),
        pty,
        history,
        bus,
    })
}

/// Spawn monitors + pollers + (optionally) agent PTY based on the current
/// `cfg`. Idempotent: if `running` is already true, returns Ok immediately.
///
/// PTY is only started when `auto_inject_on_match` is true. Otherwise the user
/// can trigger an inject from history, which calls `pty.inject_error` →
/// auto-starts on demand.
pub async fn start_all(inner: &mut Inner) -> Result<()> {
    if inner.running {
        return Ok(());
    }

    // ---- on_error callback (closes over current pty + auto_inject flag) ----
    let pty = inner.pty.clone();
    let auto_inject = inner.cfg.claude.auto_inject_on_match;
    let on_error: OnErrorCallback = Arc::new(move |node: NodeConfig, block: String| {
        let pty = pty.clone();
        Box::pin(async move {
            if auto_inject {
                let label = format!("{} ({})", node.name, node.container);
                let _ = pty.inject_error(&label, &block).await;
            }
        })
    });

    // ---- monitors ----
    let mut monitors: Vec<Arc<DockerLogMonitor>> = Vec::with_capacity(inner.cfg.nodes.len());
    let mut monitor_handles: Vec<JoinHandle<()>> = Vec::with_capacity(inner.cfg.nodes.len());
    for node in inner.cfg.nodes.iter().cloned() {
        let mon = DockerLogMonitor::from_app_config(
            &inner.cfg,
            node,
            inner.bus.clone(),
            on_error.clone(),
            Some(inner.history.clone()),
        )?;
        let mon = Arc::new(mon);
        monitor_handles.push(mon.clone().start());
        monitors.push(mon);
    }

    // ---- pollers ----
    let mut pollers: Vec<Arc<MetricsPoller>> = Vec::with_capacity(inner.cfg.nodes.len());
    let mut poller_handles: Vec<JoinHandle<()>> = Vec::with_capacity(inner.cfg.nodes.len());
    for node in inner.cfg.nodes.iter().cloned() {
        let p = MetricsPoller::from_app_config(&inner.cfg, node, inner.bus.clone())?;
        let p = Arc::new(p);
        poller_handles.push(p.clone().start());
        pollers.push(p);
    }

    // ---- agent PTY (only if auto-injecting) ----
    if auto_inject {
        if let Err(e) = inner.pty.start().await {
            inner.bus.publish(Event::console(
                "WARN",
                format!("에이전트 PTY 시작 실패: {e:?} — 매칭 발생 시 재시도합니다."),
                None,
            ));
        }
    }

    inner.monitors = monitors;
    inner.monitor_handles = monitor_handles;
    inner.pollers = pollers;
    inner.poller_handles = poller_handles;
    inner.running = true;
    inner.bus.publish(Event::status(true));
    Ok(())
}

/// Cancel + await every spawned task. Each join is awaited with a 2s timeout;
/// stragglers are logged but don't block the rest of the shutdown. PTY child
/// is killed via `PtySession::stop()`.
pub async fn stop_all(inner: &mut Inner) -> Result<()> {
    inner.running = false;

    for m in inner.monitors.iter() {
        m.stop();
    }
    for p in inner.pollers.iter() {
        p.stop();
    }

    for h in inner.monitor_handles.drain(..) {
        if let Err(e) = tokio::time::timeout(Duration::from_secs(2), h).await {
            tracing::warn!("monitor task did not terminate within 2s: {e:?}");
        }
    }
    for h in inner.poller_handles.drain(..) {
        if let Err(e) = tokio::time::timeout(Duration::from_secs(2), h).await {
            tracing::warn!("poller task did not terminate within 2s: {e:?}");
        }
    }
    inner.monitors.clear();
    inner.pollers.clear();

    inner.pty.stop().await;

    inner.bus.publish(Event::status(false));
    Ok(())
}

/// Apply a new `AppConfig`. Stops everything, swaps cfg + rebuilds the PTY
/// (since `ClaudeConfig` / `ProjectConfig` may have changed), and restarts if
/// the previous state was running.
pub async fn reload(inner: &mut Inner, new_cfg: AppConfig) -> Result<()> {
    let was_running = inner.running;
    stop_all(inner).await?;
    inner.cfg = new_cfg;
    // Rebuild the PTY: the old one captured the previous ClaudeConfig /
    // ProjectConfig values. The bus is reused so subscribers stay connected.
    inner.pty = Arc::new(PtySession::new(
        inner.cfg.claude.clone(),
        inner.cfg.project.clone(),
        inner.bus.clone(),
    ));
    if was_running {
        start_all(inner).await?;
    }
    Ok(())
}

/// Spawn the broadcast→Tauri-event relay. Lives across reloads (the bus is
/// shared). Call once from `setup()` after `app.manage(state)`.
///
/// Uses `tauri::async_runtime::spawn` rather than `tokio::spawn` because
/// `setup()` runs on the main thread outside any tokio runtime context;
/// `tokio::spawn` would panic with "there is no reactor running".
#[cfg(not(test))]
pub fn spawn_event_relay(
    app_handle: tauri::AppHandle,
    mut rx: broadcast::Receiver<Event>,
) {
    use tauri::Emitter;
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let name = ev.tauri_event_name();
                    if let Err(e) = app_handle.emit(name, &ev) {
                        tracing::warn!("relay emit '{name}' failed: {e:?}");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let warn = Event::console(
                        "WARN",
                        format!("이벤트 릴레이 지연 — {n} 개 이벤트가 드롭되었습니다."),
                        None,
                    );
                    let _ = app_handle.emit(warn.tauri_event_name(), &warn);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Stub for `cargo test --lib` (no `tauri::AppHandle` available in unit tests).
#[cfg(test)]
pub fn spawn_event_relay(
    _app_handle: (),
    mut rx: broadcast::Receiver<Event>,
) {
    tokio::spawn(async move {
        while rx.recv().await.is_ok() {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use std::io::Write;

    /// Tiny RAII tempdir to avoid pulling in the `tempfile` crate just for
    /// tests. Best-effort cleanup on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p = std::env::temp_dir().join(format!(
                "state_test_{}_{}",
                std::process::id(),
                nanos
            ));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_config(yaml: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new();
        let path = dir.path().join("config.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(yaml.as_bytes()).unwrap();
        (dir, path)
    }

    fn empty_yaml() -> &'static str {
        // No nodes → start_all has zero monitors/pollers to spawn, and we
        // leave auto_inject_on_match at its default (false) so PTY isn't
        // spawned either.
        "nodes: []\n"
    }

    fn empty_yaml_alt() -> &'static str {
        // Different cfg used for reload() comparison.
        "nodes: []\nclaude:\n  provider: codex\n"
    }

    fn fresh_history() -> Arc<HistoryStore> {
        // Use a unique tempfile path (rusqlite ":memory:" doesn't persist
        // across new Connection::open calls, which HistoryStore does
        // internally).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "state_test_history_{}_{}.db",
            std::process::id(),
            nanos
        ));
        Arc::new(HistoryStore::new(p.to_string_lossy().into_owned()).unwrap())
    }

    #[tokio::test]
    async fn initial_builds_inner_with_empty_runtime() {
        let (_dir, path) = write_config(empty_yaml());
        let bus = EventBus::new();
        let history = fresh_history();
        let inner = initial(path.clone(), bus.clone(), history.clone())
            .await
            .unwrap();
        assert_eq!(inner.cfg_path, path);
        assert!(!inner.running);
        assert!(inner.monitors.is_empty());
        assert!(inner.pollers.is_empty());
        assert!(inner.monitor_handles.is_empty());
        assert!(inner.poller_handles.is_empty());
        // History + bus references are the same Arc/clone-of-tx.
        assert_eq!(
            Arc::as_ptr(&inner.history) as usize,
            Arc::as_ptr(&history) as usize
        );
    }

    #[tokio::test]
    async fn start_then_stop_with_empty_nodes_flips_running() {
        let (_dir, path) = write_config(empty_yaml());
        let bus = EventBus::new();
        let history = fresh_history();
        let mut inner = initial(path, bus, history).await.unwrap();

        start_all(&mut inner).await.unwrap();
        assert!(inner.running);
        // Empty nodes → no monitor/poller tasks spawned.
        assert!(inner.monitors.is_empty());
        assert!(inner.pollers.is_empty());
        assert!(inner.monitor_handles.is_empty());
        assert!(inner.poller_handles.is_empty());

        // Idempotency: a second start_all is a no-op.
        start_all(&mut inner).await.unwrap();
        assert!(inner.running);

        stop_all(&mut inner).await.unwrap();
        assert!(!inner.running);
        assert!(inner.monitor_handles.is_empty());
        assert!(inner.poller_handles.is_empty());
    }

    #[tokio::test]
    async fn reload_swaps_cfg_and_preserves_running_state() {
        let (_dir, path) = write_config(empty_yaml());
        let bus = EventBus::new();
        let history = fresh_history();
        let mut inner = initial(path, bus, history).await.unwrap();
        assert_eq!(inner.cfg.claude.provider, "claude");

        start_all(&mut inner).await.unwrap();
        assert!(inner.running);

        let new_cfg = AppConfig::from_yaml(empty_yaml_alt()).unwrap();
        reload(&mut inner, new_cfg).await.unwrap();
        // cfg field changed.
        assert_eq!(inner.cfg.claude.provider, "codex");
        // Previous running=true → still running after reload.
        assert!(inner.running);

        // Stopped → reload keeps it stopped.
        stop_all(&mut inner).await.unwrap();
        let new_cfg2 = AppConfig::from_yaml(empty_yaml()).unwrap();
        reload(&mut inner, new_cfg2).await.unwrap();
        assert!(!inner.running);
        assert_eq!(inner.cfg.claude.provider, "claude");
    }
}
