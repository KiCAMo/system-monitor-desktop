//! Application configuration — Rust port of `app/config.py`.
//!
//! Mirrors the Pydantic models 1:1 with `serde(default)` providing the same
//! defaults. `load_config` reads a YAML file and expands `~` in path-ish
//! fields exactly like the Python implementation.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "ServerConfig::default_host")]
    pub host: String,
    #[serde(default = "ServerConfig::default_port")]
    pub port: u16,
}
impl ServerConfig {
    fn default_host() -> String {
        "0.0.0.0".into()
    }
    fn default_port() -> u16 {
        8000
    }
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: Self::default_host(),
            port: Self::default_port(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2Config {
    #[serde(default = "Ec2Config::default_user")]
    pub user: Option<String>,
    #[serde(default = "Ec2Config::default_port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub known_hosts: Option<String>,
    /// Optional shared default. Each node may override with its own key.
    /// Either `ec2.deploy_key` or every `node.deploy_key` must be set.
    #[serde(default)]
    pub deploy_key: Option<String>,
    #[serde(default)]
    pub diag_key: Option<String>,
    /// If true, prepend `sudo -n ` to docker commands. Useful when the SSH
    /// user isn't in the docker group; requires NOPASSWD sudoers entry.
    #[serde(default)]
    pub sudo: bool,
}
impl Ec2Config {
    fn default_user() -> Option<String> {
        Some("ubuntu".into())
    }
    fn default_port() -> u16 {
        22
    }
}
impl Default for Ec2Config {
    fn default() -> Self {
        Self {
            user: Self::default_user(),
            ssh_port: Self::default_port(),
            known_hosts: None,
            deploy_key: None,
            diag_key: None,
            sudo: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub name: String,
    pub host: String,
    pub container: String,
    #[serde(default)]
    pub health_url: Option<String>,
    /// Per-node SSH key override. Falls back to `ec2.deploy_key` if `None`.
    #[serde(default)]
    pub deploy_key: Option<String>,
    #[serde(default)]
    pub diag_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "ProjectConfig::default_local")]
    pub local_path: String,
    #[serde(default = "ProjectConfig::default_ec2_path")]
    pub ec2_path: String,
    #[serde(default)]
    pub description: String,
}
impl ProjectConfig {
    fn default_local() -> String {
        "~/projects/app".into()
    }
    fn default_ec2_path() -> String {
        "/home/ubuntu/app".into()
    }
}
impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            local_path: Self::default_local(),
            ec2_path: Self::default_ec2_path(),
            description: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorConfig {
    #[serde(default = "MonitorConfig::default_patterns")]
    pub error_patterns: Vec<String>,
    #[serde(default)]
    pub before_lines: usize,
    #[serde(default)]
    pub after_lines: usize,
    #[serde(default = "MonitorConfig::default_after_delay")]
    pub after_delay_seconds: u64,
    #[serde(default = "MonitorConfig::default_stack_continuation")]
    pub stack_continuation: String,
    #[serde(default = "MonitorConfig::default_cooldown")]
    pub cooldown_seconds: u64,
    #[serde(default = "MonitorConfig::default_max_block_chars")]
    pub max_block_chars: usize,
    #[serde(default)]
    pub only_stream_errors: bool,
    #[serde(default = "MonitorConfig::default_tail_lines")]
    pub tail_lines: u32,
}
impl MonitorConfig {
    fn default_patterns() -> Vec<String> {
        vec![r"\bERROR\b".into(), "Exception".into()]
    }
    fn default_after_delay() -> u64 {
        2
    }
    fn default_stack_continuation() -> String {
        r"^\s+at |^Caused by:|^\s+\.\.\.".into()
    }
    fn default_cooldown() -> u64 {
        60
    }
    fn default_max_block_chars() -> usize {
        4000
    }
    fn default_tail_lines() -> u32 {
        50
    }
}
impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            error_patterns: Self::default_patterns(),
            before_lines: 0,
            after_lines: 0,
            after_delay_seconds: Self::default_after_delay(),
            stack_continuation: Self::default_stack_continuation(),
            cooldown_seconds: Self::default_cooldown(),
            max_block_chars: Self::default_max_block_chars(),
            only_stream_errors: false,
            tail_lines: Self::default_tail_lines(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    /// "claude" (Anthropic) or "codex" (OpenAI). Both are PTY-driven REPLs.
    #[serde(default = "ClaudeConfig::default_provider")]
    pub provider: String,
    #[serde(default = "ClaudeConfig::default_command")]
    pub command: Vec<String>,
    #[serde(default)]
    pub auto_submit: bool,
    #[serde(default)]
    pub auto_apply: bool,
    /// Default false — matches still hit history + notification. Turn on if
    /// you want every match auto-pasted into the live agent input.
    #[serde(default)]
    pub auto_inject_on_match: bool,
    #[serde(default = "ClaudeConfig::default_install_deny")]
    pub install_deny_rules: bool,
    #[serde(default = "ClaudeConfig::default_deny_patterns")]
    pub deny_patterns: Vec<String>,
}
impl ClaudeConfig {
    fn default_provider() -> String {
        "claude".into()
    }
    fn default_command() -> Vec<String> {
        vec!["claude".into()]
    }
    fn default_install_deny() -> bool {
        true
    }
    fn default_deny_patterns() -> Vec<String> {
        vec![
            "rm -rf".into(),
            "sudo *".into(),
            "systemctl stop *".into(),
            "systemctl restart *".into(),
            "scp *".into(),
            "sftp *".into(),
            "shutdown *".into(),
            "reboot".into(),
        ]
    }
}
impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            provider: Self::default_provider(),
            command: Self::default_command(),
            auto_submit: false,
            auto_apply: false,
            auto_inject_on_match: false,
            install_deny_rules: Self::default_install_deny(),
            deny_patterns: Self::default_deny_patterns(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub build_cmd: Option<String>,
    #[serde(default)]
    pub build_cwd: Option<String>,
    #[serde(default)]
    pub artifact_path: Option<String>,
    #[serde(default)]
    pub sftp_target: Option<String>,
    #[serde(default)]
    pub remote_deploy_cmd: Option<String>,
    #[serde(default = "DeployConfig::default_build_timeout")]
    pub build_timeout_seconds: u64,
}
impl DeployConfig {
    fn default_build_timeout() -> u64 {
        300
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub ec2: Ec2Config,
    #[serde(default)]
    pub nodes: Vec<NodeConfig>,
    #[serde(default)]
    pub project: ProjectConfig,
    #[serde(default)]
    pub monitor: MonitorConfig,
    #[serde(default)]
    pub claude: ClaudeConfig,
    #[serde(default)]
    pub deploy: DeployConfig,
}

impl AppConfig {
    /// Parse from a YAML string, applying the same back-compat shim the Python
    /// model_validator did: a legacy single-node config (`ec2.host` +
    /// `target.container` with empty `nodes`) is migrated into a one-element
    /// `nodes` list before deserialization.
    pub fn from_yaml(text: &str) -> Result<Self> {
        let mut value: serde_yaml::Value = serde_yaml::from_str(text).context("parse yaml")?;
        legacy_single_node_migrate(&mut value);
        let cfg: AppConfig = serde_yaml::from_value(value).context("validate config")?;
        Ok(cfg)
    }

    /// Serialize back to YAML for write-back / profile snapshots.
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).context("serialize yaml")
    }

    /// Expand `~` in every path-shaped field. Mirrors the post-processing in
    /// the Python `load_config`. Errors during expansion fall back to the
    /// original string (so a malformed `~user/...` still round-trips).
    pub fn expand_paths(&mut self) {
        for slot in [
            self.ec2.deploy_key.as_mut(),
            self.ec2.diag_key.as_mut(),
            self.ec2.known_hosts.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            if !slot.is_empty() {
                *slot = expand_tilde(slot);
            }
        }
        if !self.project.local_path.is_empty() {
            self.project.local_path = expand_tilde(&self.project.local_path);
        }
        for n in self.nodes.iter_mut() {
            if let Some(k) = n.deploy_key.as_mut() {
                if !k.is_empty() {
                    *k = expand_tilde(k);
                }
            }
            if let Some(k) = n.diag_key.as_mut() {
                if !k.is_empty() {
                    *k = expand_tilde(k);
                }
            }
        }
    }

    /// Resolve the SSH key to use for a node — node override wins, falls back
    /// to `ec2.deploy_key`. Returns an error if neither is set.
    pub fn node_deploy_key(&self, node: &NodeConfig) -> Result<String> {
        node.deploy_key
            .as_ref()
            .or(self.ec2.deploy_key.as_ref())
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "노드 '{}' 의 SSH 키가 설정되지 않았습니다 — 노드별 deploy_key 또는 ec2.deploy_key 중 하나를 입력하세요.",
                    node.name
                )
            })
    }

    /// Diag key falls back through node → ec2 → deploy key. Returns `None`
    /// only when no key at all is configured (caller decides what to do).
    pub fn node_diag_key(&self, node: &NodeConfig) -> Option<String> {
        node.diag_key
            .clone()
            .or_else(|| self.ec2.diag_key.clone())
    }
}

/// Mutate the YAML value in place to convert legacy single-node configs
/// (`ec2.host` + `target.container`) into the new `nodes:` list form.
fn legacy_single_node_migrate(value: &mut serde_yaml::Value) {
    let mapping = match value.as_mapping_mut() {
        Some(m) => m,
        None => return,
    };
    let already_has_nodes = mapping
        .get(serde_yaml::Value::String("nodes".into()))
        .and_then(|v| v.as_sequence())
        .is_some_and(|s| !s.is_empty());
    if already_has_nodes {
        return;
    }

    // Pull `ec2.host` out of the ec2 sub-mapping and build a node from it +
    // the legacy `target` block.
    let ec2_host = mapping
        .get_mut(serde_yaml::Value::String("ec2".into()))
        .and_then(|v| v.as_mapping_mut())
        .and_then(|m| m.remove(serde_yaml::Value::String("host".into())))
        .and_then(|v| v.as_str().map(str::to_owned));
    let target = mapping
        .remove(serde_yaml::Value::String("target".into()))
        .and_then(|v| match v {
            serde_yaml::Value::Mapping(m) => Some(m),
            _ => None,
        });

    if ec2_host.is_some() || target.is_some() {
        let host = ec2_host.unwrap_or_else(|| "1.2.3.4".into());
        let container = target
            .as_ref()
            .and_then(|m| m.get(serde_yaml::Value::String("container".into())))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let health_url = target
            .as_ref()
            .and_then(|m| m.get(serde_yaml::Value::String("health_url".into())))
            .and_then(|v| v.as_str())
            .map(str::to_owned);

        let mut node = serde_yaml::Mapping::new();
        node.insert("name".into(), "node-1".into());
        node.insert("host".into(), host.into());
        node.insert("container".into(), container.into());
        node.insert(
            "health_url".into(),
            health_url
                .map(serde_yaml::Value::from)
                .unwrap_or(serde_yaml::Value::Null),
        );
        mapping.insert(
            "nodes".into(),
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::Mapping(node)]),
        );
    }
}

fn expand_tilde(s: &str) -> String {
    shellexpand::tilde(s).into_owned()
}

/// Read + parse + path-expand a yaml config file.
pub fn load_config(path: impl AsRef<Path>) -> Result<AppConfig> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read config: {}", path.display()))?;
    let mut cfg = AppConfig::from_yaml(&text)?;
    cfg.expand_paths();
    Ok(cfg)
}

/// Write a config back to disk (YAML).
pub fn save_config(path: impl AsRef<Path>, cfg: &AppConfig) -> Result<()> {
    let path = path.as_ref();
    let text = cfg.to_yaml()?;
    std::fs::write(path, text)
        .with_context(|| format!("write config: {}", path.display()))?;
    Ok(())
}

/// Default location for the active config file (next to the binary's cwd).
pub fn default_config_path() -> PathBuf {
    PathBuf::from("config.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_default_yaml() {
        let mut cfg = AppConfig::default();
        cfg.nodes.push(NodeConfig {
            name: "web-1".into(),
            host: "1.2.3.4".into(),
            container: "app".into(),
            health_url: None,
            deploy_key: None,
            diag_key: None,
        });
        let yaml = cfg.to_yaml().unwrap();
        let cfg2 = AppConfig::from_yaml(&yaml).unwrap();
        assert_eq!(cfg2.nodes.len(), 1);
        assert_eq!(cfg2.nodes[0].name, "web-1");
    }

    #[test]
    fn legacy_single_node_yaml_migrates() {
        let yaml = r#"
ec2:
  host: 1.2.3.4
  user: ubuntu
  deploy_key: /tmp/key.pem
target:
  container: web
  health_url: null
"#;
        let cfg = AppConfig::from_yaml(yaml).unwrap();
        assert_eq!(cfg.nodes.len(), 1);
        assert_eq!(cfg.nodes[0].host, "1.2.3.4");
        assert_eq!(cfg.nodes[0].container, "web");
        assert_eq!(cfg.ec2.deploy_key.as_deref(), Some("/tmp/key.pem"));
    }

    #[test]
    fn node_deploy_key_falls_back_to_ec2() {
        let mut cfg = AppConfig::default();
        cfg.ec2.deploy_key = Some("/tmp/shared.pem".into());
        cfg.nodes.push(NodeConfig {
            name: "n".into(),
            host: "h".into(),
            container: "c".into(),
            health_url: None,
            deploy_key: None,
            diag_key: None,
        });
        let key = cfg.node_deploy_key(&cfg.nodes[0]).unwrap();
        assert_eq!(key, "/tmp/shared.pem");
    }

    #[test]
    fn node_deploy_key_node_overrides_ec2() {
        let mut cfg = AppConfig::default();
        cfg.ec2.deploy_key = Some("/tmp/shared.pem".into());
        cfg.nodes.push(NodeConfig {
            name: "n".into(),
            host: "h".into(),
            container: "c".into(),
            health_url: None,
            deploy_key: Some("/tmp/per-node.pem".into()),
            diag_key: None,
        });
        let key = cfg.node_deploy_key(&cfg.nodes[0]).unwrap();
        assert_eq!(key, "/tmp/per-node.pem");
    }

    #[test]
    fn missing_keys_errors_on_resolve() {
        let mut cfg = AppConfig::default();
        cfg.nodes.push(NodeConfig {
            name: "n".into(),
            host: "h".into(),
            container: "c".into(),
            health_url: None,
            deploy_key: None,
            diag_key: None,
        });
        assert!(cfg.node_deploy_key(&cfg.nodes[0]).is_err());
    }
}
