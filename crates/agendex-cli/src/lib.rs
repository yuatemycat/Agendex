use agendex_core::agent::AgentConfig;
use agendex_core::config::{
    load_config_from_path, load_default_config, parse_tier_string, resolve_provider_preset,
};
use agendex_core::model::ModelConfig;
use agendex_trajectory::compactor::ModelTier;
use std::path::PathBuf;

pub mod model_selector;

#[derive(Debug, Clone)]
pub struct CliParsedArgs {
    pub prompt: Option<String>,
    pub session_id: String,
    pub config: AgentConfig,
    pub show_help: bool,
    pub json_mode: bool,
    pub git_commit: bool,
    pub git_branch: Option<String>,
}

pub fn parse_cli_args(args: &[String], current_dir: PathBuf) -> CliParsedArgs {
    try_parse_cli_args(args, current_dir).unwrap_or_else(|e| {
        eprintln!("[Error] {}", e);
        std::process::exit(1);
    })
}

pub fn try_parse_cli_args(args: &[String], current_dir: PathBuf) -> anyhow::Result<CliParsedArgs> {
    let mut session_id = uuid::Uuid::new_v4().to_string();
    let mut prompt: Option<String> = None;
    let mut tier = ModelTier::Standard128k;
    let mut auto_compact = true;
    let mut model_config = ModelConfig::default();
    let mut mcp_config: Option<PathBuf> = None;
    let mut auto_approve = false;
    let mut show_help = false;
    let mut json_mode = false;
    let mut git_commit = false;
    let mut git_branch: Option<String> = None;

    // First scan for an explicit --config flag
    let mut explicit_config_path: Option<PathBuf> = None;
    for (i, arg) in args.iter().enumerate() {
        if arg == "--config" && i + 1 < args.len() {
            explicit_config_path = Some(PathBuf::from(&args[i + 1]));
            break;
        }
    }

    // Load base configuration from config file if present
    let loaded_cfg = if let Some(ref p) = explicit_config_path {
        Some(load_config_from_path(p).map_err(|e| {
            anyhow::anyhow!("Failed to load configuration file '{}': {}", p.display(), e)
        })?)
    } else {
        load_default_config()
    };

    if let Some(ref cfg) = loaded_cfg {
        let mut temp_agent_config = AgentConfig {
            workspace_dir: current_dir.clone(),
            model_tier: tier,
            auto_compaction: auto_compact,
            session_id: session_id.clone(),
            model: model_config.clone(),
            max_tool_iterations: 25,
            mcp_config: mcp_config.clone(),
            auto_approve_risky: auto_approve,
        };
        cfg.apply_to_agent_config(&mut temp_agent_config);
        model_config = temp_agent_config.model;
        tier = temp_agent_config.model_tier;
        auto_approve = temp_agent_config.auto_approve_risky;
    }

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--print" | "--prompt" => {
                if i + 1 < args.len() {
                    prompt = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--conversation" => {
                if i + 1 < args.len() {
                    session_id = args[i + 1].clone();
                    i += 1;
                }
            }
            "--config" => {
                // Already processed in first scan
                if i + 1 < args.len() {
                    i += 1;
                }
            }
            "--format" | "--provider" => {
                if i + 1 < args.len() {
                    let val = args[i + 1].to_lowercase();
                    if let Some(ref cfg) = loaded_cfg {
                        if let Some(entry) = cfg.providers.get(&val) {
                            if let Some(ref m) = entry.model {
                                model_config.model_name = m.clone();
                            }
                            if let Some(ref fmt_str) = entry.format {
                                let (fmt, db) = resolve_provider_preset(fmt_str);
                                model_config.format = fmt;
                                if let Some(base) = db {
                                    model_config.base_url = Some(base);
                                }
                            }
                            if let Some(ref b) = entry.base_url {
                                let expanded = agendex_core::config::expand_env_var_or_literal(b);
                                if !expanded.is_empty() {
                                    model_config.base_url = Some(expanded);
                                }
                            }
                            if let Some(ref k) = entry.api_key {
                                let expanded = agendex_core::config::expand_env_var_or_literal(k);
                                if !expanded.is_empty() {
                                    model_config.api_key = expanded;
                                }
                            }
                            if let Some(ref t) = entry.tier {
                                tier = parse_tier_string(t);
                            }
                        } else {
                            let (fmt, default_base) = resolve_provider_preset(&val);
                            model_config.format = fmt;
                            if let Some(base) = default_base {
                                model_config.base_url = Some(base);
                            }
                            model_config.model_name =
                                agendex_core::config::default_model_for_preset(&val).to_string();
                            let key = agendex_core::config::default_api_key_for_preset(&val);
                            if !key.is_empty() {
                                model_config.api_key =
                                    agendex_core::config::expand_env_var_or_literal(&key);
                            }
                        }
                    } else {
                        let (fmt, default_base) = resolve_provider_preset(&val);
                        model_config.format = fmt;
                        if let Some(base) = default_base {
                            model_config.base_url = Some(base);
                        }
                    }
                    i += 1;
                }
            }
            "--model" => {
                if i + 1 < args.len() {
                    model_config.model_name = args[i + 1].clone();
                    i += 1;
                }
            }
            "--base-url" => {
                if i + 1 < args.len() {
                    model_config.base_url = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--api-key" => {
                if i + 1 < args.len() {
                    model_config.api_key = args[i + 1].clone();
                    i += 1;
                }
            }
            "--tier" => {
                if i + 1 < args.len() {
                    tier = parse_tier_string(&args[i + 1]);
                    i += 1;
                }
            }
            "--no-auto-compact" => {
                auto_compact = false;
            }
            "--mcp-config" => {
                if i + 1 < args.len() {
                    mcp_config = Some(PathBuf::from(&args[i + 1]));
                    i += 1;
                }
            }
            "-y" | "--yes" | "--auto-approve" => {
                auto_approve = true;
            }
            "--json" => {
                json_mode = true;
            }
            "--git-commit" => {
                git_commit = true;
            }
            "--git-branch" => {
                if i + 1 < args.len() {
                    git_branch = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "-h" | "--help" => {
                show_help = true;
            }
            _ => {}
        }
        i += 1;
    }

    let config = AgentConfig {
        workspace_dir: current_dir,
        model_tier: tier,
        auto_compaction: auto_compact,
        session_id: session_id.clone(),
        model: model_config,
        max_tool_iterations: 25,
        mcp_config,
        auto_approve_risky: auto_approve,
    };

    Ok(CliParsedArgs {
        prompt,
        session_id,
        config,
        show_help,
        json_mode,
        git_commit,
        git_branch,
    })
}
