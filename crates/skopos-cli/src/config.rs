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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            project_root: default_project_root(),
            default_provider: ProviderId::Claude,
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
        };
        let serialized = toml::to_string_pretty(&config).unwrap();
        let back: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(back.project_root, config.project_root);
        assert_eq!(back.default_provider, config.default_provider);
    }
}
