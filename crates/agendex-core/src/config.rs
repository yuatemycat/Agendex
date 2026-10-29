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
    pub active_provider: Option<String>,
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

    #[serde(default)]
    pub providers: std::collections::HashMap<String, ProviderConfigEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProviderConfigEntry {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
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

    /// Saves the current configuration file representation to a file on disk.
    pub fn save_to_disk(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write(path, toml_str)?;
        Ok(())
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
            let expanded = expand_env_var_or_literal(b);
            if !expanded.is_empty() {
                config.model.base_url = Some(expanded);
            }
        }
        if let Some(k) = self.resolved_api_key() {
            let expanded = expand_env_var_or_literal(k);
            if !expanded.is_empty() {
                config.model.api_key = expanded;
            }
        }
        if let Some(t) = self.resolved_tier() {
            config.model_tier = parse_tier_string(t);
        }
        if let Some(a) = self.resolved_auto_approve_risky() {
            config.auto_approve_risky = a;
        }

        // Active provider configuration takes precedence over legacy top-level fields
        if let Some(ref active) = self.active_provider {
            if let Some(p) = self.providers.get(active) {
                if let Some(ref m) = p.model {
                    config.model.model_name = m.clone();
                }
                if let Some(ref fmt_str) = p.format {
                    let (fmt, default_base) = resolve_provider_preset(fmt_str);
                    config.model.format = fmt;
                    if let Some(db) = default_base {
                        config.model.base_url = Some(db);
                    }
                }
                if let Some(ref b) = p.base_url {
                    let expanded = expand_env_var_or_literal(b);
                    if !expanded.is_empty() {
                        config.model.base_url = Some(expanded);
                    }
                }
                if let Some(ref k) = p.api_key {
                    let expanded = expand_env_var_or_literal(k);
                    if !expanded.is_empty() {
                        config.model.api_key = expanded;
                    }
                }
                if let Some(ref t) = p.tier {
                    config.model_tier = parse_tier_string(t);
                }
            } else {
                let (fmt, default_base) = resolve_provider_preset(active);
                config.model.format = fmt;
                if let Some(db) = default_base {
                    config.model.base_url = Some(db);
                }
                config.model.model_name = default_model_for_preset(active).to_string();
                let key = default_api_key_for_preset(active);
                if !key.is_empty() {
                    config.model.api_key = expand_env_var_or_literal(&key);
                }
            }
        }
    }
}

/// Expands environment variable expressions like `${KEY}`, `$KEY`, or `env:KEY`,
/// falling back to the literal string if not referencing an environment variable.
pub fn expand_env_var_or_literal(val: &str) -> String {
    let trimmed = val.trim();
    if trimmed.starts_with("${") && trimmed.ends_with('}') && trimmed.len() > 3 {
        let var_name = &trimmed[2..trimmed.len() - 1];
        std::env::var(var_name).unwrap_or_default()
    } else if let Some(var_name) = trimmed.strip_prefix('$') {
        std::env::var(var_name).unwrap_or_default()
    } else if let Some(var_name) = trimmed.strip_prefix("env:") {
        std::env::var(var_name).unwrap_or_default()
    } else {
        trimmed.to_string()
    }
}

/// Resolves provider or format string to a `ModelFormat` and an optional default base URL.
///
/// Built-in presets: "nim" (NVIDIA NIM), "strata" (local 125B MoE), "proxymateur", "ollama", "gemini", "anthropic".
pub fn resolve_provider_preset(provider_str: &str) -> (ModelFormat, Option<String>) {
    match provider_str.to_lowercase().trim() {
        "nim" | "nvidia" => (
            ModelFormat::OpenAi,
            Some("https://integrate.api.nvidia.com/v1".to_string()),
        ),
        "strata" => (
            ModelFormat::OpenAi,
            Some("http://127.0.0.1:8080/v1".to_string()),
        ),
        "proxymateur" => (
            ModelFormat::OpenAi,
            Some("http://127.0.0.1:8045/v1".to_string()),
        ),
        "ollama" => (
            ModelFormat::OpenAi,
            Some("http://localhost:11434/v1".to_string()),
        ),
        "groq" => (
            ModelFormat::OpenAi,
            Some("https://api.groq.com/openai/v1".to_string()),
        ),
        "openrouter" => (
            ModelFormat::OpenAi,
            Some("https://openrouter.ai/api/v1".to_string()),
        ),
        "gemini" | "google" => (ModelFormat::GoogleGemini, None),
        "anthropic" | "claude" => (ModelFormat::Anthropic, None),
        _ => (ModelFormat::OpenAi, None),
    }
}

/// Returns the standard default model identifier for known provider presets.
pub fn default_model_for_preset(provider_str: &str) -> &'static str {
    match provider_str.to_lowercase().trim() {
        "nim" | "nvidia" => "meta/llama-3.1-70b-instruct",
        "strata" => "huihui-swift-1.5",
        "proxymateur" => "gemini-2.5-flash",
        "ollama" => "qwen2.5-coder:7b",
        "gemini" | "google" => "gemini-2.5-flash",
        "anthropic" | "claude" => "claude-3-7-sonnet-20250219",
        _ => "gpt-4o",
    }
}

/// Returns the default API key or environment variable token for known provider presets.
pub fn default_api_key_for_preset(provider_str: &str) -> String {
    match provider_str.to_lowercase().trim() {
        "nim" | "nvidia" => {
            std::env::var("NVIDIA_API_KEY").unwrap_or_else(|_| "$NVIDIA_API_KEY".to_string())
        }
        "strata" => "strata".to_string(),
        "ollama" => "ollama".to_string(),
        "proxymateur" => {
            std::env::var("AGY_LLM_GATEWAY_API_KEY").unwrap_or_else(|_| "dummy".to_string())
        }
        "gemini" | "google" => {
            std::env::var("GEMINI_API_KEY").unwrap_or_else(|_| "$GEMINI_API_KEY".to_string())
        }
        "anthropic" | "claude" => {
            std::env::var("ANTHROPIC_API_KEY").unwrap_or_else(|_| "$ANTHROPIC_API_KEY".to_string())
        }
        _ => std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "$OPENAI_API_KEY".to_string()),
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
    if let Ok(override_path) = std::env::var("AGENDEX_CONFIG") {
        return Some(PathBuf::from(override_path));
    }
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

/// Creates a default configuration with pre-populated providers: nim, strata, proxymateur, ollama, gemini, claude, openai.
pub fn create_default_config_with_presets() -> AgendexConfigFile {
    let mut providers = std::collections::HashMap::new();

    providers.insert(
        "nim".to_string(),
        ProviderConfigEntry {
            format: Some("openai".to_string()),
            base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
            model: Some("meta/llama-3.1-70b-instruct".to_string()),
            api_key: Some("$NVIDIA_API_KEY".to_string()),
            tier: Some("128k".to_string()),
        },
    );

    providers.insert(
        "strata".to_string(),
        ProviderConfigEntry {
            format: Some("openai".to_string()),
            base_url: Some("http://127.0.0.1:8080/v1".to_string()),
            model: Some("huihui-swift-1.5".to_string()),
            api_key: Some("strata".to_string()),
            tier: Some("128k".to_string()),
        },
    );

    providers.insert(
        "proxymateur".to_string(),
        ProviderConfigEntry {
            format: Some("openai".to_string()),
            base_url: Some("http://127.0.0.1:8045/v1".to_string()),
            model: Some("gemini-2.5-flash".to_string()),
            api_key: Some("$AGY_LLM_GATEWAY_API_KEY".to_string()),
            tier: Some("128k".to_string()),
        },
    );

    providers.insert(
        "ollama".to_string(),
        ProviderConfigEntry {
            format: Some("openai".to_string()),
            base_url: Some("http://localhost:11434/v1".to_string()),
            model: Some("qwen2.5-coder:7b".to_string()),
            api_key: Some("ollama".to_string()),
            tier: Some("128k".to_string()),
        },
    );

    providers.insert(
        "gemini".to_string(),
        ProviderConfigEntry {
            format: Some("gemini".to_string()),
            base_url: None,
            model: Some("gemini-2.5-flash".to_string()),
            api_key: Some("$GEMINI_API_KEY".to_string()),
            tier: Some("128k".to_string()),
        },
    );

    providers.insert(
        "claude".to_string(),
        ProviderConfigEntry {
            format: Some("anthropic".to_string()),
            base_url: None,
            model: Some("claude-3-7-sonnet-20250219".to_string()),
            api_key: Some("$ANTHROPIC_API_KEY".to_string()),
            tier: Some("128k".to_string()),
        },
    );

    providers.insert(
        "openai".to_string(),
        ProviderConfigEntry {
            format: Some("openai".to_string()),
            base_url: None,
            model: Some("gpt-4o".to_string()),
            api_key: Some("$OPENAI_API_KEY".to_string()),
            tier: Some("128k".to_string()),
        },
    );

    AgendexConfigFile {
        active_provider: Some("nim".to_string()),
        model: Some("meta/llama-3.1-70b-instruct".to_string()),
        model_name: None,
        format: Some("openai".to_string()),
        provider: Some("nim".to_string()),
        base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
        api_key: Some("$NVIDIA_API_KEY".to_string()),
        tier: Some("128k".to_string()),
        auto_approve_risky: Some(false),
        agent: Some(AgentConfigSection {
            tier: Some("128k".to_string()),
            auto_approve_risky: Some(false),
            max_tool_iterations: Some(25),
        }),
        model_config_section: None,
        model_section: None,
        providers,
    }
}

/// Ensures `~/.agendex/config.toml` exists on disk, creating it with defaults if missing.
pub fn ensure_default_config_exists() -> Option<PathBuf> {
    let path = default_config_path()?;
    if !path.exists() {
        let default_cfg = create_default_config_with_presets();
        let _ = default_cfg.save_to_disk(&path);
    }
    Some(path)
}

/// Loads the default config from `~/.agendex/config.toml` if it exists.
pub fn load_default_config() -> Option<AgendexConfigFile> {
    let path = ensure_default_config_exists()?;
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

    #[test]
    fn test_env_var_expansion_for_api_key() {
        std::env::set_var("TEST_AGENDEX_SECRET_KEY", "nvapi-secret-12345");
        let toml_str = r#"
            format = "openai"
            api_key = "${TEST_AGENDEX_SECRET_KEY}"
        "#;
        let cfg = load_config_from_str(toml_str).unwrap();
        let mut agent_cfg = AgentConfig::default();
        cfg.apply_to_agent_config(&mut agent_cfg);

        assert_eq!(agent_cfg.model.api_key, "nvapi-secret-12345");
        std::env::remove_var("TEST_AGENDEX_SECRET_KEY");
    }
}
