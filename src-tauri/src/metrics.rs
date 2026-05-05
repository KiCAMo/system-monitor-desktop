//! Per-node metrics poller — Rust port of `app/metrics.py`.
//!
//! Pulls `docker stats --no-stream` over SSH every `interval_seconds` and,
//! if a `health_url` is configured, fetches it via `reqwest` with a 5s
//! timeout. Publishes a `Metrics` event for the UI.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use russh::ChannelMsg;
use serde_json::{json, Value};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{AppConfig, Ec2Config, NodeConfig};
use crate::events::{Event, EventBus};
use crate::ssh_monitor::{ssh_connect, split_user_host};

pub struct MetricsPoller {
    pub ec2: Ec2Config,
    pub node: NodeConfig,
    pub bus: EventBus,
    pub interval_seconds: u64,
    /// Pre-resolved key path: prefer node.diag_key → ec2.diag_key →
    /// node.deploy_key → ec2.deploy_key. The factory `from_app_config`
    /// applies that fallback.
    pub key_path: String,
    cancel: CancellationToken,
}

impl MetricsPoller {
    pub fn new(
        ec2: Ec2Config,
        node: NodeConfig,
        bus: EventBus,
        key_path: String,
        interval_seconds: u64,
    ) -> Self {
        Self {
            ec2,
            node,
            bus,
            interval_seconds: if interval_seconds == 0 { 5 } else { interval_seconds },
            key_path,
            cancel: CancellationToken::new(),
        }
    }

    pub fn from_app_config(cfg: &AppConfig, node: NodeConfig, bus: EventBus) -> Result<Self> {
        let key = cfg
            .node_diag_key(&node)
            .or_else(|| cfg.node_deploy_key(&node).ok())
            .ok_or_else(|| {
                anyhow!(
                    "노드 '{}' 의 SSH 키가 설정되지 않았습니다 (diag/deploy 모두 비어있음)",
                    node.name
                )
            })?;
        Ok(Self::new(cfg.ec2.clone(), node, bus, key, 5))
    }

    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move { me.run().await })
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }

    async fn run(self: Arc<Self>) {
        loop {
            if self.cancel.is_cancelled() {
                break;
            }
            match self.collect().await {
                Ok((docker, health)) => {
                    self.bus
                        .publish(Event::metrics(self.node.name.clone(), docker, health));
                }
                Err(e) => {
                    self.bus.publish(Event::console(
                        "WARN",
                        format!("메트릭 수집 실패: {e:?}"),
                        Some(self.node.name.clone()),
                    ));
                }
            }
            let sleep = tokio::time::sleep(Duration::from_secs(self.interval_seconds));
            tokio::select! {
                _ = sleep => {}
                _ = self.cancel.cancelled() => break,
            }
        }
    }

    async fn collect(&self) -> Result<(Value, Option<Value>)> {
        let docker = self.docker_stats().await?;
        let health = if self.node.health_url.is_some() {
            Some(self.health_probe().await)
        } else {
            None
        };
        Ok((docker, health))
    }

    async fn docker_stats(&self) -> Result<Value> {
        let (user, host) = split_user_host(&self.ec2, &self.node.host);
        let session = ssh_connect(&self.ec2, &host, user.as_deref(), &self.key_path).await?;
        let mut channel = session.channel_open_session().await?;

        let fmt = r#"{"name":"{{.Name}}","cpu":"{{.CPUPerc}}","mem":"{{.MemUsage}}","mem_pct":"{{.MemPerc}}","net":"{{.NetIO}}","block":"{{.BlockIO}}","pids":"{{.PIDs}}"}"#;
        let sudo = if self.ec2.sudo { "sudo -n " } else { "" };
        let cmd = format!(
            "{sudo}docker stats --no-stream --format '{fmt}' {c} ; echo '---' ; {sudo}docker inspect --format='{{{{.State.Status}}}} {{{{.State.StartedAt}}}}' {c}",
            c = self.node.container,
        );
        channel.exec(true, cmd).await?;

        let mut out = String::new();
        let mut err = String::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => out.push_str(&String::from_utf8_lossy(data)),
                ChannelMsg::ExtendedData { ref data, .. } => {
                    err.push_str(&String::from_utf8_lossy(data))
                }
                ChannelMsg::Eof | ChannelMsg::Close => {}
                ChannelMsg::ExitStatus { .. } => {}
                _ => {}
            }
        }
        let _ = session
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;

        Ok(parse_docker_stats(&out, &err))
    }

    async fn health_probe(&self) -> Value {
        let url = self.node.health_url.as_deref().unwrap_or("");
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => return json!({"error": format!("{e:?}")}),
        };
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                let body: Value = match serde_json::from_str::<Value>(&text) {
                    Ok(v) => v,
                    Err(_) => Value::String(text.chars().take(500).collect()),
                };
                json!({"status_code": status, "body": body})
            }
            Err(e) => json!({"error": format!("{e:?}")}),
        }
    }
}

/// Parse the combined `docker stats ... ; echo '---' ; docker inspect ...`
/// output into the same dict shape the python implementation produced.
pub(crate) fn parse_docker_stats(stdout: &str, stderr: &str) -> Value {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return json!({
            "raw": "",
            "error": if stderr.is_empty() { "empty" } else { stderr },
        });
    }
    let mut parts = trimmed.splitn(2, "---");
    let stats_part = parts.next().unwrap_or("").trim();
    let inspect_part = parts.next().unwrap_or("").trim();

    let stats_line = stats_part.lines().next().unwrap_or("").trim();
    let mut stats: Value = if stats_line.is_empty() {
        json!({})
    } else {
        match serde_json::from_str::<Value>(stats_line) {
            Ok(v) => v,
            Err(_) => json!({ "raw": stats_line }),
        }
    };

    let inspect_line = inspect_part.lines().next().unwrap_or("").trim();
    let (state, started_at) = match inspect_line.split_once(char::is_whitespace) {
        Some((s, rest)) => (Some(s.to_string()), Some(rest.trim().to_string())),
        None => {
            if inspect_line.is_empty() {
                (None, None)
            } else {
                (Some(inspect_line.to_string()), None)
            }
        }
    };
    if let Some(obj) = stats.as_object_mut() {
        obj.insert(
            "state".to_string(),
            state.map(Value::String).unwrap_or(Value::Null),
        );
        obj.insert(
            "started_at".to_string(),
            started_at.map(Value::String).unwrap_or(Value::Null),
        );
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_docker_stats_typical_output() {
        let stdout = r#"{"name":"app","cpu":"0.05%","mem":"40MiB / 2GiB","mem_pct":"2.0%","net":"1kB / 0B","block":"0B / 0B","pids":"5"}
---
running 2024-01-02T03:04:05.6789Z
"#;
        let v = parse_docker_stats(stdout, "");
        assert_eq!(v["name"], "app");
        assert_eq!(v["cpu"], "0.05%");
        assert_eq!(v["state"], "running");
        assert_eq!(v["started_at"], "2024-01-02T03:04:05.6789Z");
    }

    #[test]
    fn parse_docker_stats_empty_returns_error() {
        let v = parse_docker_stats("", "no such container");
        assert_eq!(v["raw"], "");
        assert_eq!(v["error"], "no such container");
    }

    #[test]
    fn parse_docker_stats_malformed_json_keeps_raw() {
        let stdout = "not-json-here\n---\nstopped\n";
        let v = parse_docker_stats(stdout, "");
        assert_eq!(v["raw"], "not-json-here");
        assert_eq!(v["state"], "stopped");
        assert_eq!(v["started_at"], Value::Null);
    }
}
