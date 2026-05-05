//! SSH-based docker log monitor — Rust port of `app/ssh_monitor.py`.
//!
//! Connects to an EC2 node via russh, streams `docker logs -f --tail N`,
//! applies regex error matching with before/after context windowing and
//! stack continuation tracking, persists hits to `HistoryStore`, and fires
//! a caller-supplied async callback (typically prompts injection into the
//! Claude/Codex PTY).
//!
//! Concurrency model:
//! - One `tokio::task` per monitor (`start()`)
//! - `CancellationToken` for cooperative shutdown (mirrors the python
//!   `asyncio.Event` stop flag)
//! - Reconnect with exponential backoff (2s → 60s)
//!
//! Security note: this implementation accepts ANY server host key. The python
//! original passed `known_hosts=None` to asyncssh which is the same behavior.
//! When the desktop app exposes proper host-key pinning the
//! `AcceptAnyHostKey` handler should be replaced with one that verifies
//! against `Ec2Config::known_hosts`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::future::BoxFuture;
use regex::{Regex, RegexBuilder};
use russh::client::{self, Handle};
use russh::ChannelMsg;
use russh_keys::key;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{AppConfig, Ec2Config, MonitorConfig, NodeConfig};
use crate::events::{Event, EventBus};
use crate::history::HistoryStore;

/// Async callback invoked when a fully-collected error block fires (and the
/// cooldown allows). Mirrors `Callable[[NodeConfig, str], Awaitable[None]]`.
pub type OnErrorCallback =
    Arc<dyn Fn(NodeConfig, String) -> BoxFuture<'static, ()> + Send + Sync>;

/// Mutable runtime state for the line-windowing state machine.
#[derive(Default)]
pub(crate) struct MonitorState {
    pub(crate) before: VecDeque<String>,
    pub(crate) active: Option<Vec<String>>,
    pub(crate) active_started: Option<Instant>,
    pub(crate) active_after_count: usize,
    pub(crate) active_matched_line: String,
    pub(crate) active_pattern: String,
    pub(crate) last_fired: Option<Instant>,
}

pub struct DockerLogMonitor {
    pub ec2: Ec2Config,
    pub node: NodeConfig,
    pub monitor: MonitorConfig,
    pub bus: EventBus,
    pub history: Option<Arc<HistoryStore>>,
    /// Resolved SSH key path (caller supplies the post-`AppConfig::node_deploy_key`
    /// value to avoid leaking the whole `AppConfig` into this struct).
    pub key_path: String,
    on_error: OnErrorCallback,
    patterns: Vec<Regex>,
    stack_pat: Option<Regex>,
    state: Mutex<MonitorState>,
    cancel: CancellationToken,
}

impl DockerLogMonitor {
    /// Construct a monitor. `key_path` should be obtained via
    /// `AppConfig::node_deploy_key(&node)`. Returns an error if any of the
    /// configured regex patterns fails to compile (matching python which
    /// raises at `re.compile` time).
    pub fn new(
        ec2: Ec2Config,
        node: NodeConfig,
        monitor: MonitorConfig,
        bus: EventBus,
        key_path: String,
        on_error: OnErrorCallback,
        history: Option<Arc<HistoryStore>>,
    ) -> Result<Self> {
        let patterns = monitor
            .error_patterns
            .iter()
            .map(|p| {
                RegexBuilder::new(p)
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| anyhow!("invalid error pattern '{p}': {e}"))
            })
            .collect::<Result<Vec<_>>>()?;
        let stack_pat = if monitor.stack_continuation.is_empty() {
            None
        } else {
            Some(
                Regex::new(&monitor.stack_continuation)
                    .map_err(|e| anyhow!("invalid stack_continuation: {e}"))?,
            )
        };
        let before_cap = monitor.before_lines;
        Ok(Self {
            ec2,
            node,
            monitor,
            bus,
            history,
            key_path,
            on_error,
            patterns,
            stack_pat,
            state: Mutex::new(MonitorState {
                before: VecDeque::with_capacity(before_cap.max(1)),
                ..Default::default()
            }),
            cancel: CancellationToken::new(),
        })
    }

    /// Convenience constructor that pulls the SSH key out of `AppConfig`.
    pub fn from_app_config(
        cfg: &AppConfig,
        node: NodeConfig,
        bus: EventBus,
        on_error: OnErrorCallback,
        history: Option<Arc<HistoryStore>>,
    ) -> Result<Self> {
        let key = cfg.node_deploy_key(&node)?;
        Self::new(
            cfg.ec2.clone(),
            node,
            cfg.monitor.clone(),
            bus,
            key,
            on_error,
            history,
        )
    }

    /// Spawn the streaming task. Returns a handle the caller can `await`.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            me.run().await;
        })
    }

    /// Cancel the inner task. Awaiting the returned `JoinHandle` from
    /// `start()` will then complete shortly afterwards.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    async fn run(self: Arc<Self>) {
        self.console("SYSTEM", format!("모니터링 시작 [{}]", self.node.name))
            .await;
        let mut backoff_secs: u64 = 2;
        while !self.cancel.is_cancelled() {
            match self.stream_once().await {
                Ok(()) => {
                    backoff_secs = 2;
                }
                Err(e) => {
                    if self.cancel.is_cancelled() {
                        break;
                    }
                    self.console(
                        "ERROR",
                        format!("SSH 오류: {e:?} — {backoff_secs}초 후 재연결"),
                    )
                    .await;
                    let sleep = tokio::time::sleep(Duration::from_secs(backoff_secs));
                    tokio::select! {
                        _ = sleep => {}
                        _ = self.cancel.cancelled() => break,
                    }
                    backoff_secs = (backoff_secs * 2).min(60);
                }
            }
        }
    }

    async fn stream_once(&self) -> Result<()> {
        let (user, host) = split_user_host(&self.ec2, &self.node.host);
        self.console(
            "SYSTEM",
            format!(
                "SSH 연결 → {}@{} (container={})",
                user.as_deref().unwrap_or(""),
                host,
                self.node.container,
            ),
        )
        .await;
        let session = ssh_connect(&self.ec2, &host, user.as_deref(), &self.key_path).await?;

        let mut channel = session.channel_open_session().await?;
        let sudo = if self.ec2.sudo { "sudo -n " } else { "" };
        let cmd = format!(
            "{sudo}docker logs -f --tail {tail} {container} 2>&1",
            tail = self.monitor.tail_lines,
            container = self.node.container,
        );
        channel.exec(true, cmd).await?;

        // Inline "delay flusher": after each read iteration we check the
        // active timer. Plus a tokio::select! with a 0.5s timeout to wake
        // the loop when the upstream is silent.
        let mut buf = String::new();
        loop {
            if self.cancel.is_cancelled() {
                break;
            }
            tokio::select! {
                msg = channel.wait() => {
                    let Some(msg) = msg else { break; };
                    match msg {
                        ChannelMsg::Data { ref data } => {
                            buf.push_str(&String::from_utf8_lossy(data));
                            self.drain_lines(&mut buf).await;
                        }
                        ChannelMsg::ExtendedData { ref data, .. } => {
                            buf.push_str(&String::from_utf8_lossy(data));
                            self.drain_lines(&mut buf).await;
                        }
                        ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitStatus { .. } => {
                            // Server closed.
                        }
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => {
                    self.maybe_delay_flush().await;
                }
                _ = self.cancel.cancelled() => break,
            }
        }
        let _ = session
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
        Ok(())
    }

    /// Pull complete lines out of `buf`, leaving any trailing partial line
    /// behind for the next read.
    async fn drain_lines(&self, buf: &mut String) {
        loop {
            let Some(idx) = buf.find('\n') else { break; };
            let line: String = buf.drain(..=idx).collect();
            let line = line.trim_end_matches('\n').to_string();
            if line.is_empty() {
                continue;
            }
            // forward decision (only_stream_errors gate)
            let forward = if self.monitor.only_stream_errors {
                self.matches_any(&line) || self.has_active().await
            } else {
                true
            };
            if forward {
                self.bus.publish(Event::ec2(&self.node.name, line.clone()));
            }
            self.on_line(&line).await;
        }
    }

    pub(crate) async fn has_active(&self) -> bool {
        self.state.lock().await.active.is_some()
    }

    /// Public for testing — drives the line state machine without networking.
    pub(crate) async fn on_line(&self, line: &str) {
        let mut st = self.state.lock().await;
        if st.active.is_some() {
            // append + continuation accounting
            let is_continuation = self
                .stack_pat
                .as_ref()
                .map(|p| p.is_match(line))
                .unwrap_or(false);
            if let Some(buf) = st.active.as_mut() {
                buf.push(line.to_string());
            }
            if !is_continuation {
                st.active_after_count += 1;
            }
            if st.active_after_count >= self.monitor.after_lines {
                drop(st);
                self.flush_active().await;
            }
            return;
        }

        // Maintain ring buffer.
        if self.monitor.before_lines > 0 {
            if st.before.len() == self.monitor.before_lines {
                st.before.pop_front();
            }
            st.before.push_back(line.to_string());
        }

        if let Some(pat_src) = self.first_match(line) {
            let mut active: Vec<String> = st.before.iter().cloned().collect();
            active.push(format!(">>> [MATCH] {line}"));
            let stripped = strip_ansi(line);
            st.active = Some(active);
            st.active_started = Some(Instant::now());
            st.active_after_count = 0;
            st.active_matched_line = stripped.clone();
            st.active_pattern = pat_src;
            drop(st);
            // Truncate to 500 chars (matches python's `[:500]`).
            let preview: String = stripped.chars().take(500).collect();
            self.console("MATCH", preview).await;
        }
    }

    async fn maybe_delay_flush(&self) {
        let should_flush = {
            let st = self.state.lock().await;
            if let (Some(_), Some(started)) = (&st.active, st.active_started) {
                started.elapsed() >= Duration::from_secs(self.monitor.after_delay_seconds)
            } else {
                false
            }
        };
        if should_flush {
            self.flush_active().await;
        }
    }

    pub(crate) async fn flush_active(&self) {
        let (block_lines_opt, _started) = {
            let mut st = self.state.lock().await;
            let started = st.active_started.take();
            (st.active.take(), started)
        };
        let Some(block_lines) = block_lines_opt else {
            return;
        };
        let line_count = block_lines.len();
        let block = block_lines.join("\n");
        let cap = self.monitor.max_block_chars;
        let block = if cap > 0 && block.len() > cap {
            let keep = ((cap.saturating_sub(30)) / 2).max(200);
            // Char-safe slice at byte boundaries: clamp keep into a valid
            // boundary for both the prefix and suffix slices.
            let prefix = safe_prefix(&block, keep);
            let suffix = safe_suffix(&block, keep);
            format!("{prefix}\n...[truncated]...\n{suffix}")
        } else {
            block
        };
        self.maybe_fire(block, line_count).await;
        // Reset after_count for next round.
        let mut st = self.state.lock().await;
        st.active_after_count = 0;
    }

    pub(crate) async fn maybe_fire(&self, block: String, line_count: usize) {
        let now = Instant::now();
        let (matched_line, pattern) = {
            let st = self.state.lock().await;
            (st.active_matched_line.clone(), st.active_pattern.clone())
        };

        // Persist regardless of cooldown.
        let mut match_id: Option<i64> = None;
        if let Some(history) = &self.history {
            match history
                .record(
                    &self.node.name,
                    Some(&self.node.container),
                    &matched_line,
                    &block,
                    Some(&pattern),
                )
                .await
            {
                Ok(id) => match_id = Some(id),
                Err(e) => {
                    self.console("WARN", format!("히스토리 저장 실패: {e:?}"))
                        .await;
                }
            }
        }

        let preview: String = matched_line.chars().take(500).collect();
        self.bus.publish(Event::match_detected(
            match_id,
            &self.node.name,
            &self.node.container,
            &pattern,
            preview,
        ));

        let cooldown = Duration::from_secs(self.monitor.cooldown_seconds);
        let last = self.state.lock().await.last_fired;
        if let Some(prev) = last {
            if now.duration_since(prev) < cooldown {
                self.console(
                    "MONITOR",
                    format!(
                        "[쿨다운] 직전 주입 후 {}초 미경과 — 스킵",
                        self.monitor.cooldown_seconds
                    ),
                )
                .await;
                return;
            }
        }
        self.state.lock().await.last_fired = Some(now);
        self.console(
            "MONITOR",
            format!(
                "[ERROR 감지] [{}] {}줄 컨텍스트 → 프롬프트 입력창에 주입",
                self.node.name, line_count
            ),
        )
        .await;
        let cb = self.on_error.clone();
        let node = self.node.clone();
        // The original called the callback inside a try/except. We mirror by
        // catching panics through the Future return type (Err is encoded by
        // the callback returning normally; panics become a console error via
        // the JoinHandle below).
        let fut = (cb)(node, block);
        let bus = self.bus.clone();
        let node_name = self.node.name.clone();
        if let Err(e) = tokio::spawn(fut).await {
            bus.publish(Event::console(
                "ERROR",
                format!("프롬프트 입력창 주입 실패: {e:?}"),
                Some(node_name),
            ));
        }
    }

    pub(crate) fn matches_any(&self, line: &str) -> bool {
        self.patterns.iter().any(|p| p.is_match(line))
    }

    pub(crate) fn first_match(&self, line: &str) -> Option<String> {
        for (src, pat) in self.monitor.error_patterns.iter().zip(self.patterns.iter()) {
            if pat.is_match(line) {
                return Some(src.clone());
            }
        }
        None
    }

    async fn console(&self, level: &str, msg: impl Into<String>) {
        self.bus
            .publish(Event::console(level, msg, Some(self.node.name.clone())));
    }
}

// ---- low-level SSH helpers ----------------------------------------------------

/// Russh client handler that accepts any server key. See module docs.
pub(crate) struct AcceptAnyHostKey;

#[async_trait]
impl client::Handler for AcceptAnyHostKey {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Open an authenticated russh session to `host:port` using the supplied
/// key file. Mirrors the python `_ssh_connect_kwargs` + `asyncssh.connect`
/// pair. Used by both `ssh_monitor` and `metrics`.
pub(crate) async fn ssh_connect(
    ec2: &Ec2Config,
    host: &str,
    user_override: Option<&str>,
    key_path: &str,
) -> Result<Handle<AcceptAnyHostKey>> {
    let key_pair = russh_keys::load_secret_key(key_path, None)
        .map_err(|e| anyhow!("load ssh key '{key_path}': {e}"))?;
    let cfg = client::Config {
        inactivity_timeout: Some(Duration::from_secs(300)),
        ..Default::default()
    };
    let cfg = Arc::new(cfg);
    let mut session = client::connect(cfg, (host, ec2.ssh_port), AcceptAnyHostKey)
        .await
        .map_err(|e| anyhow!("ssh connect {host}:{}: {e}", ec2.ssh_port))?;
    let user = user_override
        .map(str::to_string)
        .or_else(|| ec2.user.clone())
        .unwrap_or_default();
    let auth_ok = session
        .authenticate_publickey(user, Arc::new(key_pair))
        .await
        .map_err(|e| anyhow!("ssh auth: {e}"))?;
    if !auth_ok {
        return Err(anyhow!("ssh auth failed (publickey rejected)"));
    }
    Ok(session)
}

/// If `host` is `user@h` and ec2.user is empty, split. Returns
/// `(resolved_user, bare_host)`.
pub(crate) fn split_user_host(ec2: &Ec2Config, host: &str) -> (Option<String>, String) {
    if let Some((u, h)) = host.split_once('@') {
        if ec2.user.as_deref().unwrap_or("").is_empty() {
            return (Some(u.to_string()), h.to_string());
        }
    }
    (ec2.user.clone(), host.to_string())
}

// ---- text helpers ------------------------------------------------------------

static ANSI_RE: once_cell::sync::Lazy<Regex> =
    once_cell::sync::Lazy::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap());

pub(crate) fn strip_ansi(s: &str) -> String {
    ANSI_RE.replace_all(s, "").into_owned()
}

fn safe_prefix(s: &str, n: usize) -> &str {
    let mut end = n.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
fn safe_suffix(s: &str, n: usize) -> &str {
    let mut start = s.len().saturating_sub(n);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Ec2Config, MonitorConfig, NodeConfig};
    use crate::events::EventBus;

    fn dummy_node() -> NodeConfig {
        NodeConfig {
            name: "n1".into(),
            host: "1.2.3.4".into(),
            container: "app".into(),
            health_url: None,
            deploy_key: None,
            diag_key: None,
        }
    }
    fn dummy_callback() -> OnErrorCallback {
        Arc::new(|_n, _b| Box::pin(async {}))
    }

    fn make(monitor: MonitorConfig) -> Arc<DockerLogMonitor> {
        let bus = EventBus::new();
        Arc::new(
            DockerLogMonitor::new(
                Ec2Config::default(),
                dummy_node(),
                monitor,
                bus,
                "/dev/null".into(),
                dummy_callback(),
                None,
            )
            .unwrap(),
        )
    }

    #[test]
    fn ansi_strip_removes_color_codes() {
        let s = "\x1b[31mERROR\x1b[0m something \x1b[1;32mok\x1b[m";
        assert_eq!(strip_ansi(s), "ERROR something ok");
    }

    #[test]
    fn first_match_returns_source_pattern_string() {
        let mut m = MonitorConfig::default();
        m.error_patterns = vec![r"\bFOO\b".into(), "Exception".into()];
        let mon = make(m);
        assert_eq!(
            mon.first_match("oops Exception thrown"),
            Some("Exception".to_string())
        );
        assert_eq!(mon.first_match("nothing here"), None);
    }

    #[tokio::test]
    async fn flush_fires_after_after_lines_non_continuation() {
        let mut m = MonitorConfig::default();
        m.error_patterns = vec!["ERROR".into()];
        m.before_lines = 1;
        m.after_lines = 2;
        m.cooldown_seconds = 0;
        m.after_delay_seconds = 9999;
        let mon = make(m);
        let mut rx = mon.bus.subscribe();

        mon.on_line("preceding").await;
        mon.on_line("ERROR boom").await;
        // After this only after_count == 1
        mon.on_line("trailing-1").await;
        assert!(mon.has_active().await, "should still be capturing");
        mon.on_line("trailing-2").await;
        assert!(!mon.has_active().await, "should have flushed");

        // Drain bus for sanity (don't deadlock if empty).
        for _ in 0..20 {
            match tokio::time::timeout(Duration::from_millis(20), rx.recv()).await {
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
    }

    #[tokio::test]
    async fn block_truncated_when_over_max_block_chars() {
        let mut m = MonitorConfig::default();
        m.error_patterns = vec!["ERR".into()];
        m.before_lines = 0;
        m.after_lines = 1;
        m.cooldown_seconds = 0;
        m.max_block_chars = 80;
        let mon = make(m);
        let mut rx = mon.bus.subscribe();

        // Match line is 200 chars.
        let big = format!("ERR {}", "x".repeat(200));
        mon.on_line(&big).await;
        mon.on_line("after").await; // triggers flush

        // Find a console MONITOR event whose msg references the block.
        // Easier: just look at the match payload's matched_line preview and
        // confirm internal block went through truncation by re-running with
        // history captured. Here we just assert it didn't panic & flushed.
        for _ in 0..30 {
            match tokio::time::timeout(Duration::from_millis(20), rx.recv()).await {
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
        // Re-flush a second time should be a no-op.
        mon.flush_active().await;

        // Direct unit-test of truncation logic via safe_prefix/safe_suffix:
        let s = "a".repeat(500);
        let p = safe_prefix(&s, 200);
        let q = safe_suffix(&s, 200);
        let combined = format!("{p}\n...[truncated]...\n{q}");
        assert!(combined.contains("...[truncated]..."));
        assert_eq!(p.len(), 200);
        assert_eq!(q.len(), 200);
    }

    #[tokio::test]
    async fn cooldown_skip_publishes_monitor_skip_event() {
        let mut m = MonitorConfig::default();
        m.error_patterns = vec!["ERR".into()];
        m.before_lines = 0;
        m.after_lines = 1;
        m.cooldown_seconds = 60;
        let mon = make(m);
        let mut rx = mon.bus.subscribe();

        // First fire.
        mon.on_line("ERR one").await;
        mon.on_line("after-1").await;
        // Second fire within cooldown.
        mon.on_line("ERR two").await;
        mon.on_line("after-2").await;

        // Collect events for ~200ms and ensure we see at least one MONITOR
        // [쿨다운] message.
        let mut saw_skip = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(ev)) => {
                    if let crate::events::EventPayload::Console { level, msg, .. } = &ev.payload {
                        if level == "MONITOR" && msg.contains("쿨다운") {
                            saw_skip = true;
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(saw_skip, "expected a cooldown-skip MONITOR event");
    }
}
