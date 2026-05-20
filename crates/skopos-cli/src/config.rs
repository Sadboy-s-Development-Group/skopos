//! User configuration for the Skopos CLI.
//!
//! Loaded from `~/.config/skopos/config.toml`. The file is created with
//! defaults the first time it is needed so users have a discoverable place
//! to tweak settings.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::providers::ProviderId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Config {
    /// Directory whose immediate subfolders are listed by `skopos work`.
    pub project_root: PathBuf,
    /// Provider used by `skopos work` when none is passed on the CLI.
    pub default_provider: ProviderId,
    /// Tunables for `skopos network`. Defaulted so configs predating the
    /// feature still parse.
    #[serde(default)]
    pub network: NetworkConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            project_root: default_project_root(),
            default_provider: ProviderId::Claude,
            network: NetworkConfig::default(),
        }
    }
}

/// `[network]` block — drives the connectivity probe daemon and the
/// `skopos network` dashboard's stable / moderate / severe verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NetworkConfig {
    /// Websites the probe daemon pings each tick. The link is only `down`
    /// when every target fails, so list a few independent hosts.
    pub targets: Vec<String>,
    /// Seconds between probe ticks.
    pub interval_secs: u64,
    /// ICMP echoes sent per target per tick.
    pub ping_count: u32,
    /// Network interface to read carrier state from; auto-detected if unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iface: Option<String>,
    /// Rolling window, in minutes, behind the headline health verdict.
    pub headline_window_mins: i64,
    /// A tick counts as `degraded` once best RTT reaches this many ms.
    pub degraded_rtt_ms: f64,
    /// A tick counts as `degraded` once packet loss reaches this percent.
    pub degraded_loss_pct: f64,
    /// Outages in the window at/above which health is at least `moderate`.
    pub moderate_outages: i64,
    /// Outages in the window at/above which health is `severe`.
    pub severe_outages: i64,
    /// Cumulative downtime (s) at/above which health is at least `moderate`.
    pub moderate_downtime_secs: i64,
    /// Cumulative downtime (s) at/above which health is `severe`.
    pub severe_downtime_secs: i64,
    /// Fraction of `degraded` ticks at/above which health is `moderate`.
    pub moderate_degraded_ratio: f64,
    /// Probe samples older than this many days are pruned by the daemon.
    pub retention_days: i64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            targets: vec![
                "cloudflare.com".to_string(),
                "google.com".to_string(),
                "wikipedia.org".to_string(),
            ],
            interval_secs: 10,
            ping_count: 3,
            iface: None,
            headline_window_mins: 60,
            degraded_rtt_ms: 250.0,
            degraded_loss_pct: 5.0,
            moderate_outages: 1,
            severe_outages: 4,
            moderate_downtime_secs: 30,
            severe_downtime_secs: 300,
            moderate_degraded_ratio: 0.20,
            retention_days: 30,
        }
    }
}

/// Load `~/.config/skopos/config.toml`, writing the defaults if the file
/// does not yet exist. A malformed file is reported instead of silently
/// overwritten — surprises in the user's `$HOME` should be loud.
pub(crate) fn load() -> anyhow::Result<Config> {
    let path = config_path();
    if !path.exists() {
        let config = Config::default();
        write(&path, &config)?;
        return Ok(config);
    }
    let raw = fs::read_to_string(&path)?;
    let config: Config = toml::from_str(&raw)
        .map_err(|err| anyhow::anyhow!("failed to parse {}: {err}", path.display()))?;
    Ok(config)
}

fn write(path: &Path, config: &Config) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = toml::to_string_pretty(config)?;
    fs::write(path, body)?;
    Ok(())
}

fn config_path() -> PathBuf {
    home_dir()
        .join(".config")
        .join("skopos")
        .join("config.toml")
}

fn default_project_root() -> PathBuf {
    home_dir().join("Coding")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrips_through_toml() {
        let config = Config {
            project_root: PathBuf::from("/tmp/foo"),
            default_provider: ProviderId::Claude,
            network: NetworkConfig::default(),
        };
        let serialized = toml::to_string_pretty(&config).unwrap();
        let back: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(back.project_root, config.project_root);
        assert_eq!(back.default_provider, config.default_provider);
        assert_eq!(back.network.targets, config.network.targets);
        assert_eq!(back.network.interval_secs, config.network.interval_secs);
    }

    #[test]
    fn config_without_network_block_still_parses() {
        let legacy = r#"
            project_root = "/tmp/foo"
            default_provider = "claude"
        "#;
        let config: Config = toml::from_str(legacy).unwrap();
        assert_eq!(config.network.interval_secs, 10);
        assert_eq!(config.network.targets.len(), 3);
    }
}
