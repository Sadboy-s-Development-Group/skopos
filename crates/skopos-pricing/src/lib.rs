//! Pricing catalog for the providers Skopos tracks.
//!
//! A built-in [`Catalog::defaults`] ships with the per-million-token rates
//! Anthropic, OpenAI and Google publish for the models that currently
//! appear in `skopos.db`. The catalog can be overlaid at runtime from a
//! TOML file (default path: `~/.config/skopos/pricing.toml`) so the user
//! can refresh prices without recompiling.
//!
//! The expected TOML shape is one `[[model]]` table per entry:
//!
//! ```toml
//! [[model]]
//! provider = "anthropic"
//! model = "claude-opus-4-7"
//! input_per_million = 5.0
//! output_per_million = 25.0
//! cached_input_per_million = 0.50
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use skopos_core::Money;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    pub provider: String,
    pub model: String,
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cached_input_per_million: Option<f64>,
}

impl ModelPrice {
    /// Tokens are passed as Skopos stores them: `input_tokens` is the
    /// *uncached* portion (collectors already subtract `cached` from the
    /// vendor's gross input), and `cached_input_tokens` is the cached
    /// portion. Output is straightforward.
    pub fn estimate_usd(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: Option<u64>,
    ) -> Money {
        let cached = cached_input_tokens.unwrap_or(0);
        let input_cost = input_tokens as f64 / 1_000_000.0 * self.input_per_million;
        let output_cost = output_tokens as f64 / 1_000_000.0 * self.output_per_million;
        let cached_cost = cached as f64 / 1_000_000.0
            * self
                .cached_input_per_million
                .unwrap_or(self.input_per_million);

        Money::usd(input_cost + output_cost + cached_cost)
    }
}

#[derive(Debug, Default, Deserialize)]
struct PricingFile {
    #[serde(default)]
    model: Vec<ModelPrice>,
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    by_key: HashMap<(String, String), ModelPrice>,
}

impl Catalog {
    /// Built-in price table. Last validated against vendor docs on
    /// 2026-05-19. Cached rate uses the cache-read tier (0.1× base for
    /// Claude, ditto for gpt-5.5 and Gemini) because cache reads
    /// dominate the agentic-CLI workload Skopos observes.
    pub fn defaults() -> Self {
        let mut catalog = Self::default();
        for price in default_prices() {
            catalog.insert(price);
        }
        catalog
    }

    pub fn insert(&mut self, price: ModelPrice) {
        let key = (price.provider.clone(), price.model.clone());
        self.by_key.insert(key, price);
    }

    pub fn price(&self, provider: &str, model: &str) -> Option<&ModelPrice> {
        self.by_key.get(&(provider.to_string(), model.to_string()))
    }

    pub fn estimate(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        cached_input_tokens: Option<u64>,
        output_tokens: u64,
    ) -> Option<Money> {
        self.price(provider, model)
            .map(|p| p.estimate_usd(input_tokens, output_tokens, cached_input_tokens))
    }

    /// Defaults overlaid with prices from `path`. A missing file leaves
    /// the catalog at defaults; an unparseable file is a hard error so
    /// silent typos don't quietly fall back to stale numbers.
    pub fn load_with_overrides(path: &Path) -> anyhow::Result<Self> {
        let mut catalog = Self::defaults();
        if !path.exists() {
            return Ok(catalog);
        }
        let raw = fs::read_to_string(path)?;
        let parsed: PricingFile = toml::from_str(&raw)
            .map_err(|err| anyhow::anyhow!("failed to parse {}: {err}", path.display()))?;
        for price in parsed.model {
            catalog.insert(price);
        }
        Ok(catalog)
    }
}

/// Default location for the user-editable override file.
pub fn default_overrides_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("skopos").join("pricing.toml")
}

fn default_prices() -> Vec<ModelPrice> {
    vec![
        ModelPrice {
            provider: "anthropic".to_string(),
            model: "claude-opus-4-7".to_string(),
            input_per_million: 5.0,
            output_per_million: 25.0,
            cached_input_per_million: Some(0.50),
        },
        ModelPrice {
            provider: "anthropic".to_string(),
            model: "claude-haiku-4-5-20251001".to_string(),
            input_per_million: 1.0,
            output_per_million: 5.0,
            cached_input_per_million: Some(0.10),
        },
        ModelPrice {
            provider: "openai".to_string(),
            model: "gpt-5.5".to_string(),
            input_per_million: 5.0,
            output_per_million: 30.0,
            cached_input_per_million: Some(0.50),
        },
        ModelPrice {
            provider: "google".to_string(),
            model: "gemini-3-flash-preview".to_string(),
            input_per_million: 0.50,
            output_per_million: 3.0,
            cached_input_per_million: Some(0.05),
        },
        // Hermes-routed traffic. Hermes is multi-billing-route (openai-codex,
        // copilot, direct API…) and the effective price the user pays depends
        // on which subscription was active. The defaults below mirror the
        // native API pricing of each underlying model so `est cost` stops
        // reading $0 — users on flat-fee routes (e.g. Copilot) should override
        // these entries in ~/.config/skopos/pricing.toml.
        ModelPrice {
            provider: "hermes".to_string(),
            model: "gpt-5.5".to_string(),
            input_per_million: 5.0,
            output_per_million: 30.0,
            cached_input_per_million: Some(0.50),
        },
        ModelPrice {
            provider: "hermes".to_string(),
            model: "claude-haiku-4.5".to_string(),
            input_per_million: 1.0,
            output_per_million: 5.0,
            cached_input_per_million: Some(0.10),
        },
        ModelPrice {
            provider: "hermes".to_string(),
            model: "gemini-3-flash-preview".to_string(),
            input_per_million: 0.50,
            output_per_million: 3.0,
            cached_input_per_million: Some(0.05),
        },
        // No native Gemini 3.1 Pro entry yet; this estimate tracks Google's
        // published Gemini Pro tier (~2.5× Flash). Override locally once
        // Google publishes the final 3.1 Pro pricing.
        ModelPrice {
            provider: "hermes".to_string(),
            model: "gemini-3.1-pro-preview".to_string(),
            input_per_million: 1.25,
            output_per_million: 5.0,
            cached_input_per_million: Some(0.125),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_usd_from_token_counts() {
        let price = ModelPrice {
            provider: "example".to_string(),
            model: "model".to_string(),
            input_per_million: 1.0,
            output_per_million: 2.0,
            cached_input_per_million: Some(0.25),
        };

        // 1M uncached input × $1 + 500k output × $2 + 100k cached × $0.25
        // = 1.00 + 1.00 + 0.025 = 2.025
        let cost = price.estimate_usd(1_000_000, 500_000, Some(100_000));
        assert!((cost.amount - 2.025).abs() < 1e-9);
    }

    #[test]
    fn catalog_defaults_cover_known_models() {
        let catalog = Catalog::defaults();
        assert!(catalog.price("anthropic", "claude-opus-4-7").is_some());
        assert!(catalog
            .price("anthropic", "claude-haiku-4-5-20251001")
            .is_some());
        assert!(catalog.price("openai", "gpt-5.5").is_some());
        assert!(catalog.price("google", "gemini-3-flash-preview").is_some());
        // Hermes routes — model names differ slightly from the native
        // catalog (e.g. claude-haiku-4.5 vs claude-haiku-4-5-20251001),
        // so they need their own entries even though the prices match.
        assert!(catalog.price("hermes", "gpt-5.5").is_some());
        assert!(catalog.price("hermes", "claude-haiku-4.5").is_some());
        assert!(catalog.price("hermes", "gemini-3-flash-preview").is_some());
        assert!(catalog.price("hermes", "gemini-3.1-pro-preview").is_some());
        assert!(catalog.price("openai", "ghost-model-9000").is_none());
    }

    #[test]
    fn override_file_replaces_default_entry() {
        let dir =
            std::env::temp_dir().join(format!("skopos-pricing-override-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pricing.toml");
        std::fs::write(
            &path,
            r#"
[[model]]
provider = "anthropic"
model = "claude-opus-4-7"
input_per_million = 99.0
output_per_million = 199.0
cached_input_per_million = 9.0
"#,
        )
        .unwrap();

        let catalog = Catalog::load_with_overrides(&path).unwrap();
        let price = catalog.price("anthropic", "claude-opus-4-7").unwrap();
        assert_eq!(price.input_per_million, 99.0);
        assert_eq!(price.output_per_million, 199.0);
        assert_eq!(price.cached_input_per_million, Some(9.0));

        assert!(catalog.price("openai", "gpt-5.5").is_some());
    }

    #[test]
    fn missing_override_file_returns_defaults() {
        let path = std::env::temp_dir().join("skopos-pricing-does-not-exist.toml");
        let _ = std::fs::remove_file(&path);
        let catalog = Catalog::load_with_overrides(&path).unwrap();
        assert!(catalog.price("anthropic", "claude-opus-4-7").is_some());
    }

    #[test]
    fn estimate_returns_none_for_unknown_model() {
        let catalog = Catalog::defaults();
        assert!(catalog
            .estimate("openai", "ghost-model-9000", 1_000_000, None, 1_000_000)
            .is_none());
    }
}
