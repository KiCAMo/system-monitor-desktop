# Architecture (Tauri rewrite)

Senior-architect plan that drives this project. Read together with the Python
predecessor at https://github.com/KiCAMo/system-management — every Rust module
here replaces a Python file with the same semantics.

## Module layout

```
src-tauri/src/
├── main.rs              # Tauri app entry, plugin registration, AppState init
├── lib.rs               # `pub fn run()` — invoke_handler + setup
├── config.rs            # ← config.py: AppConfig + sub-structs, load_config()
├── events.rs            # ← events.py: tokio::sync::broadcast bus
├── ssh_monitor.rs       # ← ssh_monitor.py: docker logs streaming + match logic
├── metrics.rs           # ← metrics.py: docker stats + /health probe
├── pty_session.rs       # ← claude_session.py: portable-pty agent driver
├── history.rs           # ← history.py: rusqlite store
├── state.rs             # ← main.py AppState class: Arc<tokio::Mutex<Inner>>
└── commands/
    ├── mod.rs
    ├── settings.rs      # get_settings, save_settings, get_config_summary
    ├── profiles.rs      # list_profiles, save_profile, load_profile, delete_profile
    ├── history.rs       # list_history, get_history, inject_history, delete_history, clear_history
    ├── pty.rs           # pty_input, pty_raw, pty_resize
    ├── browse.rs        # browse_fs
    └── control.rs       # start_monitoring, stop_monitoring
```

## Event bus & frontend IPC

A single `tokio::sync::broadcast::Sender<Event>` lives in `state::Inner`. A
relay task spawned at `setup()` time holds an `AppHandle` clone and re-emits
each `Event` as a Tauri event with one of these names:

| Tauri event       | Replaces Python channel | Payload (TS-ish)                                                        |
|-------------------|-------------------------|-------------------------------------------------------------------------|
| `ec2-line`        | `ec2`                   | `{ node, line, ts }`                                                    |
| `console-msg`     | `console`               | `{ level, msg, node?, ts }`                                             |
| `claude-chunk`    | `claude`                | `{ chunk, ts }`                                                         |
| `metrics-update`  | `metrics`               | `{ node, docker, health?, ts }`                                         |
| `monitor-status`  | `status`                | `{ running, ts }`                                                       |
| `match-detected`  | `match`                 | `{ id?, node, container, pattern, matched_line, ts }`                   |

The frontend replaces `new WebSocket("/ws")` with one `listen("...", handler)`
per event name (see `dist/app.js` after migration).

## Tauri commands (≈ FastAPI routes)

Settings — `get_settings`, `save_settings`, `get_config_summary`.
Profiles — `list_profiles`, `save_profile`, `load_profile`, `delete_profile`.
History — `list_history`, `get_history`, `inject_history`, `delete_history`, `clear_history`.
PTY — `pty_input`, `pty_raw`, `pty_resize`.
Browse — `browse_fs`.
Control — `start_monitoring`, `stop_monitoring`.

## State management

```rust
pub struct Inner {
    pub cfg: AppConfig,
    pub cfg_path: PathBuf,
    pub running: bool,
    pub monitor_handles: Vec<JoinHandle<()>>,
    pub poller_handles: Vec<JoinHandle<()>>,
    pub pty: PtySession,
    pub history: Arc<HistoryStore>,
    pub tx: broadcast::Sender<Event>,
}
pub struct AppState(pub Arc<tokio::sync::Mutex<Inner>>);
```

`HistoryStore` and `tx` survive profile hot reload; only monitors/pollers/PTY
are torn down and rebuilt. The mutex must be `tokio::sync::Mutex` because hot
reload holds it across `.await` points.

## Concurrency replacements

| Python                                | Rust                                                           |
|---------------------------------------|-----------------------------------------------------------------|
| `asyncio.Lock` (PTY write)            | `tokio::sync::Mutex<()>`                                        |
| `asyncio.Queue` per subscriber        | `broadcast::Receiver<Event>` (drop-oldest via `Lagged`)         |
| `asyncio.Event` for `_bracketed_paste_ready` | `tokio::sync::Notify` + `tokio::time::timeout`           |
| `asyncio.Event` for `_stop`           | `tokio_util::sync::CancellationToken`                           |
| `asyncio.create_task`                 | `tokio::spawn` returning `JoinHandle`                           |
| `asyncio.to_thread` for sqlite        | `tokio::task::spawn_blocking`                                   |
| pexpect blocking read in executor     | `tokio::io::unix::AsyncFd<RawFd>` with read-until-WouldBlock loop |

## Risks (must address during impl)

1. **russh key auth** — passphrase-protected keys block; decode up front and cache.
2. **russh known_hosts** — config currently allows null; need `AcceptAnyHost` policy gated on a flag.
3. **App Sandbox** — leave off; PTY/`posix_openpt` would be blocked otherwise.
4. **broadcast lag** — `Lagged(n)` is silent unless logged; add a counter to console events.
5. **Updater signing** — generate Tauri signer key, embed pubkey, store privkey in CI secret. Apple Developer cert optional; without it Gatekeeper warns on first install.
6. **PTY async reads** — must loop on `readable().await + try_read` until `WouldBlock` per wakeup, not single-shot.
7. **Mid-capture reconnect** — `_before` deque and `_active` window must be reset at top of each `stream_once`.
8. **serde_yaml 0.9** — round-trip via `serde_yaml::to_string`, not through `serde_json::Value`.
