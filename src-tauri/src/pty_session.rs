//! PTY session — Rust port of `app/claude_session.py`.
//!
//! Drives a long-lived agent CLI (`claude` or `codex`) under a real PTY via
//! `portable_pty`, streams output chunks to the [`EventBus`], and supports
//! injecting bracketed-paste prompts on demand.
//!
//! # Concurrency mapping
//! - `asyncio.Lock`            -> `tokio::sync::Mutex<()>`
//! - `asyncio.Event` (one-shot)-> `tokio::sync::Notify` + `AtomicBool`
//! - `pexpect.spawn`           -> `portable_pty::native_pty_system().openpty(...)`
//! - blocking PTY reader       -> `tokio::task::spawn_blocking` loop
//!
//! # Quirks
//! - `portable_pty::Reader` and `Writer` are blocking sync I/O; we wrap reads
//!   in `spawn_blocking` so the tokio runtime stays unblocked.
//! - The reader handle is obtained via `master.try_clone_reader()` BEFORE the
//!   master is moved into a synchronization wrapper.
//! - Master/child are `Send` but not `Sync`; we keep them behind a
//!   `tokio::sync::Mutex` so `&self` methods can use them across `.await`.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::config::{ClaudeConfig, ProjectConfig};
use crate::events::{Event, EventBus};

const BRACKET_START: &str = "\x1b[200~";
const BRACKET_END: &str = "\x1b[201~";
const BRACKETED_PASTE_ENABLE: &str = "\x1b[?2004h";

/// Build the inject prompt — mirrors `PROMPT_TEMPLATE` in the Python source.
fn build_prompt(container: &str, block: &str) -> String {
    let body = format!("[{container}] 에러 감지 — 직전 로그 전문\n\n{block}\n");
    // Strip trailing CR/LF so the receiving TUI doesn't auto-submit on a
    // dangling newline (codex / some claude builds do that).
    body.trim_end_matches(|c| c == '\r' || c == '\n').to_string()
}

/// Strip env vars that would confuse the spawned agent CLI. Mirrors the Python
/// loop that pops `CLAUDECODE*`, `CLAUDE_CODE*`, `CODEX_*`, `OPENAI_CODEX*`,
/// and `AI_AGENT`. Also fills in TERM / LANG / LC_ALL defaults.
fn scrub_env(input: HashMap<String, String>) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = input
        .into_iter()
        .filter(|(k, _)| {
            !(k.starts_with("CLAUDECODE")
                || k.starts_with("CLAUDE_CODE")
                || k.starts_with("CODEX_")
                || k.starts_with("OPENAI_CODEX")
                || k == "AI_AGENT")
        })
        .collect();
    out.entry("TERM".into()).or_insert_with(|| "xterm-256color".into());
    out.entry("LANG".into()).or_insert_with(|| "en_US.UTF-8".into());
    out.entry("LC_ALL".into()).or_insert_with(|| "en_US.UTF-8".into());
    out
}

/// Internal mutable state — kept behind the session's locks.
struct Inner {
    master: Option<Box<dyn MasterPty + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    writer: Option<Box<dyn Write + Send>>,
    reader_handle: Option<JoinHandle<()>>,
}

pub struct PtySession {
    pub cfg: ClaudeConfig,
    pub project: ProjectConfig,
    pub bus: EventBus,
    /// Serializes paste injection (bracketed paste + optional Enter) to avoid
    /// interleaving two prompts on the wire.
    write_lock: Mutex<()>,
    /// Holds master/child/writer/reader-handle. Behind `Mutex` because the
    /// underlying portable-pty types are `Send` but not `Sync`.
    inner: Mutex<Inner>,
    /// Set when the TUI emits `\x1b[?2004h`. Until then, paste injection
    /// would have its newlines treated as Enter presses.
    bracketed_paste_ready: AtomicBool,
    bracketed_paste_notify: Arc<Notify>,
    /// True between an injection and the next observed `\r`/`\n` from the
    /// user. Prevents stacking two pastes in the same input box.
    inject_pending: AtomicBool,
}

impl PtySession {
    pub fn new(cfg: ClaudeConfig, project: ProjectConfig, bus: EventBus) -> Self {
        Self {
            cfg,
            project,
            bus,
            write_lock: Mutex::new(()),
            inner: Mutex::new(Inner {
                master: None,
                child: None,
                writer: None,
                reader_handle: None,
            }),
            bracketed_paste_ready: AtomicBool::new(false),
            bracketed_paste_notify: Arc::new(Notify::new()),
            inject_pending: AtomicBool::new(false),
        }
    }

    /// True if the underlying child process is alive. Used to gate auto-start
    /// in [`inject_error`].
    pub async fn is_alive(&self) -> bool {
        let mut inner = self.inner.lock().await;
        match inner.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Spawn the agent CLI under a PTY. Idempotent: returns Ok if already
    /// running.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        if self.is_alive().await {
            return Ok(());
        }

        // ---- env ----
        let parent_env: HashMap<String, String> = std::env::vars().collect();
        let env = scrub_env(parent_env);

        // ---- cwd ----
        let configured = self.project.local_path.clone();
        let cwd: Option<String> = if !configured.is_empty()
            && Path::new(&configured).is_dir()
        {
            Some(configured.clone())
        } else {
            if !configured.is_empty() {
                self.bus.publish(Event::console(
                    "WARN",
                    format!(
                        "project.local_path={configured:?} 가 존재하지 않아 cwd 미지정으로 spawn 합니다."
                    ),
                    None,
                ));
            }
            None
        };

        // ---- command ----
        let command = self.cfg.command.clone();
        let (cmd, args) = command
            .split_first()
            .ok_or_else(|| anyhow!("claude.command is empty"))?;

        let mut builder = CommandBuilder::new(cmd);
        for a in args {
            builder.arg(a);
        }
        if let Some(c) = cwd.as_ref() {
            builder.cwd(c);
        }
        for (k, v) in &env {
            builder.env(k, v);
        }

        // ---- spawn ----
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 40,
                cols: 160,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;
        let child = pair.slave.spawn_command(builder).context("spawn agent")?;
        // Drop the slave so EOF on master propagates correctly when the
        // child exits.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("clone pty reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("take pty writer")?;

        // ---- save state ----
        {
            let mut inner = self.inner.lock().await;
            inner.master = Some(pair.master);
            inner.child = Some(child);
            inner.writer = Some(writer);
        }

        let cwd_display = cwd
            .as_deref()
            .map(|s| format!("{s:?}"))
            .unwrap_or_else(|| "None".into());
        self.bus.publish(Event::console(
            "SYSTEM",
            format!(
                "{} 세션 시작: {} (cwd={})",
                self.cfg.provider,
                self.cfg.command.join(" "),
                cwd_display
            ),
            None,
        ));

        // ---- reader loop ----
        let me = Arc::clone(self);
        let handle = tokio::spawn(async move { me.reader_loop(reader).await });
        {
            let mut inner = self.inner.lock().await;
            inner.reader_handle = Some(handle);
        }

        Ok(())
    }

    async fn reader_loop(self: Arc<Self>, mut reader: Box<dyn std::io::Read + Send>) {
        // Read in 4 KiB blocking chunks on a worker thread; emit chunks as
        // claude events. We accumulate a small UTF-8 spillover so multi-byte
        // characters never split across events.
        let mut spill: Vec<u8> = Vec::new();
        loop {
            let read_res = tokio::task::spawn_blocking(move || {
                let mut buf = [0u8; 4096];
                let n = reader.read(&mut buf);
                (reader, n.map(|n| buf[..n].to_vec()))
            })
            .await;

            let (returned_reader, read_outcome) = match read_res {
                Ok(t) => t,
                Err(join_err) => {
                    self.bus.publish(Event::console(
                        "ERROR",
                        format!("Claude reader 오류: {join_err:?}"),
                        None,
                    ));
                    break;
                }
            };
            reader = returned_reader;

            let bytes = match read_outcome {
                Ok(b) => b,
                Err(e) => {
                    self.bus.publish(Event::console(
                        "ERROR",
                        format!("Claude reader 오류: {e:?}"),
                        None,
                    ));
                    break;
                }
            };

            if bytes.is_empty() {
                self.bus
                    .publish(Event::console("SYSTEM", "에이전트 세션 종료됨", None));
                break;
            }

            spill.extend_from_slice(&bytes);
            // Find the largest valid UTF-8 prefix; carry the rest.
            let chunk = match std::str::from_utf8(&spill) {
                Ok(s) => {
                    let owned = s.to_string();
                    spill.clear();
                    owned
                }
                Err(e) => {
                    let valid_up_to = e.valid_up_to();
                    let valid = String::from_utf8_lossy(&spill[..valid_up_to]).into_owned();
                    spill.drain(..valid_up_to);
                    valid
                }
            };

            if chunk.is_empty() {
                continue;
            }

            // Detect bracketed-paste enable on first sighting.
            if !self.bracketed_paste_ready.load(Ordering::Acquire)
                && chunk.contains(BRACKETED_PASTE_ENABLE)
            {
                self.bracketed_paste_ready.store(true, Ordering::Release);
                self.bracketed_paste_notify.notify_waiters();
                self.bus.publish(Event::console(
                    "SYSTEM",
                    "에이전트 입력창 준비 완료 (bracketed paste 활성)",
                    None,
                ));
            }

            self.bus.publish(Event::claude(chunk));
        }
    }

    /// Stop the session: abort the reader, kill the child, and clear the
    /// runtime fields so a subsequent `start()` re-spawns cleanly.
    pub async fn stop(&self) {
        let (handle, mut child, _master, _writer) = {
            let mut inner = self.inner.lock().await;
            (
                inner.reader_handle.take(),
                inner.child.take(),
                inner.master.take(),
                inner.writer.take(),
            )
        };
        if let Some(h) = handle {
            h.abort();
            let _ = h.await;
        }
        if let Some(c) = child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.bracketed_paste_ready.store(false, Ordering::Release);
        self.inject_pending.store(false, Ordering::Release);
    }

    /// Forward a raw keystroke (or paste fragment) from xterm.js straight to
    /// the PTY. Best-effort: errors are swallowed. Clears `inject_pending`
    /// when an Enter is observed, so the next inject can fire fresh.
    pub async fn send_raw(&self, data: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(w) = inner.writer.as_mut() {
            let _ = w.write_all(data.as_bytes());
            let _ = w.flush();
            if data.contains('\r') || data.contains('\n') {
                self.inject_pending.store(false, Ordering::Release);
            }
        }
    }

    /// Resize the PTY. No-op if the session isn't running.
    pub async fn resize(&self, cols: u16, rows: u16) {
        let inner = self.inner.lock().await;
        if let Some(m) = inner.master.as_ref() {
            let _ = m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    /// Internal: write a bracketed-paste-wrapped block, optionally followed by
    /// a CR submit. Mirrors `_send` in the Python.
    async fn send_inner(&self, text: &str, submit: bool) -> Result<()> {
        let _g = self.write_lock.lock().await;
        let trimmed = text.trim_end_matches(|c| c == '\r' || c == '\n');
        let payload = format!("{BRACKET_START}{trimmed}{BRACKET_END}");
        {
            let mut inner = self.inner.lock().await;
            let writer = inner
                .writer
                .as_mut()
                .ok_or_else(|| anyhow!("agent session not running"))?;
            writer.write_all(payload.as_bytes()).context("pty write")?;
            writer.flush().ok();
        }
        if submit {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut inner = self.inner.lock().await;
            if let Some(w) = inner.writer.as_mut() {
                let _ = w.write_all(b"\r");
                let _ = w.flush();
            }
        }
        Ok(())
    }

    pub async fn send_user_input(&self, text: &str, submit: bool) -> Result<()> {
        self.send_inner(text, submit).await
    }

    /// Inject an error block into the agent's input box. Auto-starts the
    /// session if it isn't running. Skips (with a MONITOR notice) if a
    /// previous paste hasn't been submitted yet.
    pub async fn inject_error(self: &Arc<Self>, container: &str, block: &str) -> Result<()> {
        if !self.is_alive().await {
            self.start().await?;
        }
        if self.inject_pending.load(Ordering::Acquire) {
            self.bus.publish(Event::console(
                "MONITOR",
                "[건너뜀] 이전 paste 가 아직 입력창에 대기 중 — Enter 누른 뒤 다음 매칭부터 다시 주입됩니다.",
                None,
            ));
            return Ok(());
        }
        if !self.bracketed_paste_ready.load(Ordering::Acquire) {
            self.bus
                .publish(Event::console("SYSTEM", "에이전트 입력창 준비 대기 중...", None));
            let notify = Arc::clone(&self.bracketed_paste_notify);
            // Re-check before awaiting, in case the flag flipped between the
            // first load and now (avoiding a missed notification).
            if !self.bracketed_paste_ready.load(Ordering::Acquire) {
                let waited =
                    tokio::time::timeout(Duration::from_secs(15), notify.notified()).await;
                if waited.is_err() {
                    self.bus.publish(Event::console(
                        "WARN",
                        "에이전트가 bracketed paste 를 활성화하지 않아 그대로 시도합니다.",
                        None,
                    ));
                }
            }
        }

        let prompt = build_prompt(container, block);
        self.send_inner(&prompt, self.cfg.auto_submit).await?;
        // If we auto-submitted, the next inject can fire immediately.
        self.inject_pending
            .store(!self.cfg.auto_submit, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn scrub_env_drops_agent_markers_and_fills_defaults() {
        let input = map(&[
            ("CLAUDECODE_VERSION", "1"),
            ("CLAUDE_CODE_SOMETHING", "x"),
            ("CODEX_HOME", "/tmp/codex"),
            ("OPENAI_CODEX_FOO", "bar"),
            ("AI_AGENT", "claude"),
            ("PATH", "/usr/bin"),
        ]);
        let out = scrub_env(input);
        assert!(!out.contains_key("CLAUDECODE_VERSION"));
        assert!(!out.contains_key("CLAUDE_CODE_SOMETHING"));
        assert!(!out.contains_key("CODEX_HOME"));
        assert!(!out.contains_key("OPENAI_CODEX_FOO"));
        assert!(!out.contains_key("AI_AGENT"));
        assert_eq!(out.get("PATH").map(String::as_str), Some("/usr/bin"));
        // Defaults filled in.
        assert_eq!(out.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert_eq!(out.get("LANG").map(String::as_str), Some("en_US.UTF-8"));
        assert_eq!(out.get("LC_ALL").map(String::as_str), Some("en_US.UTF-8"));
    }

    #[test]
    fn scrub_env_preserves_user_provided_term() {
        let input = map(&[("TERM", "screen-256color"), ("HOME", "/Users/me")]);
        let out = scrub_env(input);
        // User TERM survives.
        assert_eq!(out.get("TERM").map(String::as_str), Some("screen-256color"));
        assert_eq!(out.get("HOME").map(String::as_str), Some("/Users/me"));
    }

    #[test]
    fn build_prompt_strips_trailing_newlines() {
        let p = build_prompt("web-1", "boom\nstack");
        // The template adds a trailing \n which must be stripped.
        assert!(!p.ends_with('\n'));
        assert!(!p.ends_with('\r'));
        assert!(p.starts_with("[web-1] 에러 감지"));
        assert!(p.contains("boom\nstack"));
    }

    #[test]
    fn build_prompt_round_wrapped_with_brackets_for_send() {
        let p = build_prompt("c", "B");
        let wrapped = format!("{BRACKET_START}{p}{BRACKET_END}");
        assert!(wrapped.starts_with("\x1b[200~"));
        assert!(wrapped.ends_with("\x1b[201~"));
        // The body sits between the markers.
        let mid = &wrapped[BRACKET_START.len()..wrapped.len() - BRACKET_END.len()];
        assert_eq!(mid, p);
    }

    /// In-memory writer that mimics the PTY writer for unit testing the
    /// `send_raw` flag-toggle path without spawning a real PTY. The writer
    /// stores everything written so we can also assert pass-through bytes.
    struct VecWriter(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for VecWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn inject_pending_flag_clears_on_observed_enter() {
        // Inject a fake writer so send_raw exercises the full path (write +
        // CR/LF detection that flips inject_pending).
        let bus = EventBus::new();
        let session = PtySession::new(
            ClaudeConfig::default(),
            ProjectConfig::default(),
            bus,
        );
        let buf = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        {
            let mut inner = session.inner.lock().await;
            inner.writer = Some(Box::new(VecWriter(Arc::clone(&buf))));
        }

        session.inject_pending.store(true, Ordering::Release);
        session.send_raw("\r").await;
        assert!(
            !session.inject_pending.load(Ordering::Acquire),
            "Enter on a staged paste should clear inject_pending"
        );
        assert_eq!(&*buf.lock().unwrap(), b"\r");

        // Re-arm and try with a non-newline keystroke: flag must remain set.
        session.inject_pending.store(true, Ordering::Release);
        session.send_raw("a").await;
        assert!(
            session.inject_pending.load(Ordering::Acquire),
            "non-Enter keystroke must NOT clear inject_pending"
        );
        assert_eq!(&*buf.lock().unwrap(), b"\ra");
    }

    #[tokio::test]
    async fn bracketed_paste_notify_triggers_on_marker_byte() {
        // Simulate the reader's detection logic by feeding a chunk through the
        // same checks the reader_loop does. We don't run the loop, just
        // assert the side-effects when the marker is seen.
        let bus = EventBus::new();
        let session = Arc::new(PtySession::new(
            ClaudeConfig::default(),
            ProjectConfig::default(),
            bus,
        ));

        // Subscribe to the bus so the SYSTEM message has a receiver.
        let mut rx = session.bus.subscribe();

        // Pre-condition: not ready.
        assert!(!session.bracketed_paste_ready.load(Ordering::Acquire));

        // Wait future races with the trigger.
        let notify = Arc::clone(&session.bracketed_paste_notify);
        let waiter = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(2), notify.notified())
                .await
                .is_ok()
        });
        // Give the spawned task a moment to register on the Notify before we
        // fire `notify_waiters` (which only wakes currently-registered
        // waiters).
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Mirror the reader_loop branch.
        let chunk = format!("garbage {BRACKETED_PASTE_ENABLE} more");
        if !session.bracketed_paste_ready.load(Ordering::Acquire)
            && chunk.contains(BRACKETED_PASTE_ENABLE)
        {
            session.bracketed_paste_ready.store(true, Ordering::Release);
            session.bracketed_paste_notify.notify_waiters();
            session.bus.publish(Event::console(
                "SYSTEM",
                "에이전트 입력창 준비 완료 (bracketed paste 활성)",
                None,
            ));
        }

        assert!(session.bracketed_paste_ready.load(Ordering::Acquire));
        assert!(waiter.await.unwrap(), "notify_waiters should wake the listener");
        // SYSTEM event was published.
        let ev = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match ev.payload {
            crate::events::EventPayload::Console { level, msg, .. } => {
                assert_eq!(level, "SYSTEM");
                assert!(msg.contains("bracketed paste"));
            }
            _ => panic!("expected console event"),
        }
    }
}
