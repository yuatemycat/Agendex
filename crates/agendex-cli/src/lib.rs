use agendex_core::agent::AgentConfig;
use agendex_core::model::{ModelConfig, ModelFormat};
use agendex_trajectory::compactor::ModelTier;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CliParsedArgs {
    pub prompt: Option<String>,
    pub session_id: String,
    pub config: AgentConfig,
    pub show_help: bool,
}

pub fn parse_cli_args(args: &[String], current_dir: PathBuf) -> CliParsedArgs {
    let mut session_id = uuid::Uuid::new_v4().to_string();
    let mut prompt: Option<String> = None;
    let mut tier = ModelTier::Standard128k;
    let mut auto_compact = true;
    let mut model_config = ModelConfig::default();
    let mut mcp_config: Option<PathBuf> = None;
    let mut auto_approve = false;
    let mut show_help = false;

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
            "--format" => {
                if i + 1 < args.len() {
                    model_config.format = match args[i + 1].to_lowercase().as_str() {
                        "gemini" | "google" => ModelFormat::GoogleGemini,
                        "anthropic" | "claude" => ModelFormat::Anthropic,
                        _ => ModelFormat::OpenAi,
                    };
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
                    let val = args[i + 1].to_lowercase();
                    tier = match val.as_str() {
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
                    };
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

    CliParsedArgs {
        prompt,
        session_id,
        config,
        show_help,
    }
}
