//! Configuration file loader and provider preset resolution for Agendex.
//!
//! Supports `~/.agendex/config.toml` with presets such as "proxymateur".

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::agent::AgentConfig;
use crate::model::ModelFormat;
use agendex_trajectory::compactor::ModelTier;

/// Configuration file representation for `~/.agendex/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgendexConfigFile {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub auto_approve_risky: Option<bool>,

    #[serde(default)]
    pub agent: Option<AgentConfigSection>,
    #[serde(default, rename = "model_config")]
    pub model_config_section: Option<ModelConfigSection>,
    #[serde(default, rename = "model_section")]
    pub model_section: Option<ModelConfigSection>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentConfigSection {
    pub tier: Option<String>,
    pub auto_approve_risky: Option<bool>,
    pub max_tool_iterations: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelConfigSection {
    pub name: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub format: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

impl AgendexConfigFile {
    pub fn resolved_model(&self) -> Option<&str> {
        self.model
            .as_deref()
            .or(self.model_name.as_deref())
            .or_else(|| {
                self.model_config_section
                    .as_ref()
                    .and_then(|m| m.model.as_deref().or(m.name.as_deref()))
            })
            .or_else(|| {
                self.model_section
                    .as_ref()
                    .and_then(|m| m.model.as_deref().or(m.name.as_deref()))
            })
    }

    pub fn resolved_provider_or_format(&self) -> Option<&str> {
        self.provider
            .as_deref()
            .or(self.format.as_deref())
            .or_else(|| {
                self.model_config_section
                    .as_ref()
                    .and_then(|m| m.provider.as_deref().or(m.format.as_deref()))
            })
            .or_else(|| {
                self.model_section
                    .as_ref()
                    .and_then(|m| m.provider.as_deref().or(m.format.as_deref()))
            })
    }

    pub fn resolved_base_url(&self) -> Option<&str> {
        self.base_url
            .as_deref()
            .or_else(|| {
                self.model_config_section
                    .as_ref()
                    .and_then(|m| m.base_url.as_deref())
            })
            .or_else(|| {
                self.model_section
                    .as_ref()
                    .and_then(|m| m.base_url.as_deref())
            })
    }

    pub fn resolved_api_key(&self) -> Option<&str> {
        self.api_key
            .as_deref()
            .or_else(|| {
                self.model_config_section
                    .as_ref()
                    .and_then(|m| m.api_key.as_deref())
            })
            .or_else(|| {
                self.model_section
                    .as_ref()
                    .and_then(|m| m.api_key.as_deref())
            })
    }

    pub fn resolved_tier(&self) -> Option<&str> {
        self.tier
            .as_deref()
            .or_else(|| self.agent.as_ref().and_then(|a| a.tier.as_deref()))
    }

    pub fn resolved_auto_approve_risky(&self) -> Option<bool> {
        self.auto_approve_risky
            .or_else(|| self.agent.as_ref().and_then(|a| a.auto_approve_risky))
    }

    /// Applies configuration file values onto an existing `AgentConfig`.
    pub fn apply_to_agent_config(&self, config: &mut AgentConfig) {
        if let Some(m) = self.resolved_model() {
            config.model.model_name = m.to_string();
        }
        if let Some(pf) = self.resolved_provider_or_format() {
            let (fmt, default_base) = resolve_provider_preset(pf);
            config.model.format = fmt;
            if let Some(db) = default_base {
                config.model.base_url = Some(db);
            }
        }
        if let Some(b) = self.resolved_base_url() {
            config.model.base_url = Some(b.to_string());
        }
        if let Some(k) = self.resolved_api_key() {
            config.model.api_key = k.to_string();
        }
        if let Some(t) = self.resolved_tier() {
            config.model_tier = parse_tier_string(t);
        }
        if let Some(a) = self.resolved_auto_approve_risky() {
            config.auto_approve_risky = a;
        }
    }
}

/// Resolves provider or format string to a `ModelFormat` and an optional default base URL.
///
/// Built-in preset "proxymateur": format = OpenAi, default base_url = "http://127.0.0.1:8045/v1".
pub fn resolve_provider_preset(provider_str: &str) -> (ModelFormat, Option<String>) {
    match provider_str.to_lowercase().trim() {
        "proxymateur" => (
            ModelFormat::OpenAi,
            Some("http://127.0.0.1:8045/v1".to_string()),
        ),
        "gemini" | "google" => (ModelFormat::GoogleGemini, None),
        "anthropic" | "claude" => (ModelFormat::Anthropic, None),
        _ => (ModelFormat::OpenAi, None),
    }
}

/// Parses a string representation into a `ModelTier`.
pub fn parse_tier_string(val: &str) -> ModelTier {
    match val.to_lowercase().trim() {
        "32k" => ModelTier::Tier32k,
        "64k" => ModelTier::Tier64k,
        "80k" => ModelTier::Compact80k,
        "128k" => ModelTier::Standard128k,
        "200k" => ModelTier::Tier200k,
        "256k" => ModelTier::HighTier256k,
        "840k" => ModelTier::Extended840k,
        "1m" | "1000k" => ModelTier::Tier1M,
        "2m" | "2000k" => ModelTier::Tier2M,
        other => {
            if let Ok(num) = other.parse::<usize>() {
                ModelTier::Custom(num)
            } else {
                ModelTier::Standard128k
            }
        }
    }
}

/// Returns the standard path to `~/.agendex/config.toml`.
pub fn default_config_path() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    Some(PathBuf::from(home).join(".agendex").join("config.toml"))
}

/// Parses a TOML configuration string.
pub fn load_config_from_str(s: &str) -> anyhow::Result<AgendexConfigFile> {
    let cfg: AgendexConfigFile = toml::from_str(s)?;
    Ok(cfg)
}

/// Reads and parses a configuration file from a given path.
pub fn load_config_from_path<P: AsRef<Path>>(path: P) -> anyhow::Result<AgendexConfigFile> {
    let s = std::fs::read_to_string(path)?;
    load_config_from_str(&s)
}

/// Loads the default config from `~/.agendex/config.toml` if it exists.
pub fn load_default_config() -> Option<AgendexConfigFile> {
    let path = default_config_path()?;
    if path.exists() {
        match load_config_from_path(&path) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                tracing::warn!("Failed loading config at {}: {}", path.display(), e);
                None
            }
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_flat_config() {
        let toml_str = r#"
            model = "gpt-4o"
            provider = "proxymateur"
            tier = "256k"
            auto_approve_risky = true
        "#;
        let cfg = load_config_from_str(toml_str).unwrap();
        assert_eq!(cfg.resolved_model(), Some("gpt-4o"));
        assert_eq!(cfg.resolved_provider_or_format(), Some("proxymateur"));
        assert_eq!(cfg.resolved_tier(), Some("256k"));
        assert_eq!(cfg.resolved_auto_approve_risky(), Some(true));

        let mut agent_cfg = AgentConfig::default();
        cfg.apply_to_agent_config(&mut agent_cfg);

        assert_eq!(agent_cfg.model.model_name, "gpt-4o");
        assert_eq!(agent_cfg.model.format, ModelFormat::OpenAi);
        assert_eq!(
            agent_cfg.model.base_url,
            Some("http://127.0.0.1:8045/v1".to_string())
        );
        assert_eq!(agent_cfg.model_tier, ModelTier::HighTier256k);
        assert!(agent_cfg.auto_approve_risky);
    }

    #[test]
    fn test_proxymateur_preset_custom_base_url() {
        let toml_str = r#"
            format = "proxymateur"
            base_url = "http://localhost:9999/v1"
            api_key = "my-secret-key"
        "#;
        let cfg = load_config_from_str(toml_str).unwrap();
        let mut agent_cfg = AgentConfig::default();
        cfg.apply_to_agent_config(&mut agent_cfg);

        assert_eq!(agent_cfg.model.format, ModelFormat::OpenAi);
        assert_eq!(
            agent_cfg.model.base_url,
            Some("http://localhost:9999/v1".to_string())
        );
        assert_eq!(agent_cfg.model.api_key, "my-secret-key");
    }

    #[test]
    fn test_section_based_config() {
        let toml_str = r#"
            [model_config]
            model = "claude-3-5-sonnet"
            provider = "anthropic"

            [agent]
            tier = "840k"
            auto_approve_risky = false
        "#;
        let cfg = load_config_from_str(toml_str).unwrap();
        let mut agent_cfg = AgentConfig::default();
        cfg.apply_to_agent_config(&mut agent_cfg);

        assert_eq!(agent_cfg.model.model_name, "claude-3-5-sonnet");
        assert_eq!(agent_cfg.model.format, ModelFormat::Anthropic);
        assert_eq!(agent_cfg.model_tier, ModelTier::Extended840k);
        assert!(!agent_cfg.auto_approve_risky);
    }
}
