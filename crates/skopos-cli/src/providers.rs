//! Agentic CLI providers supported by `skopos work`.
//!
//! Each provider carries an id (used in config/flags), a display label,
//! the shell command to exec, and a primary RGB colour used as the
//! selection accent in the project picker. Adding a provider here is
//! enough to make it available; the selector cycles through every entry
//! returned by [`Provider::all`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProviderId {
    Claude,
    Codex,
    Gemini,
    Hermes,
    Opencode,
}

impl ProviderId {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
            ProviderId::Gemini => "gemini",
            ProviderId::Hermes => "hermes",
            ProviderId::Opencode => "opencode",
        }
    }

    pub(crate) fn command(&self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
            ProviderId::Gemini => "gemini",
            ProviderId::Hermes => "hermes",
            ProviderId::Opencode => "opencode",
        }
    }

    /// Primary accent colour used for the selection caret and provider
    /// chip in the picker.
    pub(crate) fn color(&self) -> (u8, u8, u8) {
        match self {
            // Claude orange (matches the Anthropic brand accent).
            ProviderId::Claude => (204, 120, 50),
            // Placeholders — refined when each provider is wired up.
            ProviderId::Codex => (180, 220, 130),
            ProviderId::Gemini => (120, 170, 255),
            // Hermes caduceus gold.
            ProviderId::Hermes => (212, 175, 55),
            ProviderId::Opencode => (240, 200, 90),
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for ProviderId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "claude" => Ok(ProviderId::Claude),
            "codex" => Ok(ProviderId::Codex),
            "gemini" => Ok(ProviderId::Gemini),
            "hermes" => Ok(ProviderId::Hermes),
            "opencode" => Ok(ProviderId::Opencode),
            other => Err(format!("unknown provider: {other}")),
        }
    }
}

/// Providers the `skopos work` ←/→ cycle steps through. Claude, Codex,
/// Gemini and Hermes all have log collectors wired end-to-end today;
/// Opencode stays defined in the registry but is not pickable until its
/// integration lands.
pub(crate) fn picker_cycle() -> &'static [ProviderId] {
    &[
        ProviderId::Claude,
        ProviderId::Codex,
        ProviderId::Gemini,
        ProviderId::Hermes,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_id_case_insensitively() {
        assert_eq!("claude".parse::<ProviderId>().unwrap(), ProviderId::Claude);
        assert_eq!("CLAUDE".parse::<ProviderId>().unwrap(), ProviderId::Claude);
        assert_eq!("hermes".parse::<ProviderId>().unwrap(), ProviderId::Hermes);
        assert!("foobar".parse::<ProviderId>().is_err());
    }

    #[test]
    fn claude_command_matches_binary() {
        assert_eq!(ProviderId::Claude.command(), "claude");
    }

    #[test]
    fn hermes_is_in_picker_cycle() {
        assert!(picker_cycle().contains(&ProviderId::Hermes));
    }
}
