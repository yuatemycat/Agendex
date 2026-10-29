use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use agendex_core::agent::AgendexAgent;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let parsed = agendex_cli::parse_cli_args(&args, current_dir);
    if parsed.show_help {
        print_help();
        return Ok(());
    }

    // Direct tracing logs to stderr to keep stdout 100% clean for JSON lines
    if parsed.json_mode {
        let _ = tracing_subscriber::fmt().with_writer(io::stderr).try_init();
    } else {
        let _ = tracing_subscriber::fmt().try_init();
    }

    agendex_core::ui::init_terminal();

    let config = parsed.config;
    let prompt = parsed.prompt;

    let active_provider = if let Some(cfg) = agendex_core::config::load_default_config() {
        cfg.active_provider.unwrap_or_else(|| "nim".to_string())
    } else {
        "nim".to_string()
    };

    if !parsed.json_mode && prompt.is_none() {
        agendex_core::ui::print_banner(
            &config.workspace_dir,
            &config.model.model_name,
            &active_provider,
            config.model.base_url.as_deref(),
        );
    }

    let agent = AgendexAgent::init(config.clone()).await?;

    // Prompt for risky commands whenever a person is at the terminal, including
    // one-shot `-p` runs. Without a terminal (CI, pipes), risky commands are
    // refused unless `--yes` was passed.
    use std::io::IsTerminal;
    let (agent, approval_queue) =
        if !config.auto_approve_risky && std::io::stdin().is_terminal() && !parsed.json_mode {
            let queue = agendex_core::swarm::SharedApprovalQueue::new();
            (agent.with_approval_queue(Arc::clone(&queue)), Some(queue))
        } else {
            (agent, None)
        };

    let agent = if parsed.json_mode {
        agent.with_event_handler(Arc::new(|evt| {
            if let Ok(line) = serde_json::to_string(evt) {
                println!("{}", line);
                let _ = io::stdout().flush();
            }
        }))
    } else {
        agent
            .with_token_stream_handler(Arc::new(|token| {
                print!("{}", token);
                let _ = io::stdout().flush();
            }))
            .with_tool_progress_handler(Arc::new(|msg| {
                if let Some(clean) = agendex_core::ui::format_tool_notification(msg) {
                    println!("{}", clean);
                }
            }))
    };

    // Helper: switch or create branch if requested
    if let Some(ref branch) = parsed.git_branch {
        match agent.switch_or_create_branch(branch).await {
            Ok(msg) => {
                if !parsed.json_mode {
                    println!("[Agendex] {}", msg);
                }
            }
            Err(err) => {
                if !parsed.json_mode {
                    eprintln!("[Error] Failed to switch branch: {}", err);
                } else {
                    let err_evt = agendex_core::events::AgentEvent::Error {
                        message: format!("Git branch error: {}", err),
                    };
                    if let Ok(l) = serde_json::to_string(&err_evt) {
                        println!("{}", l);
                    }
                }
            }
        }
    }

    if let Some(p) = prompt {
        let turn_res = agent.process_turn(&p).await;
        match turn_res {
            Ok(resp) => {
                if !parsed.json_mode {
                    if agent.streamed_tokens_in_last_turn() {
                        println!();
                    } else {
                        println!("{}", resp);
                    }
                    if let Some(telemetry) = agent.format_telemetry().await {
                        println!("{}", agendex_core::ui::format_telemetry_footer(&telemetry));
                    }
                }

                // Automated Git Task Commit helper
                if parsed.git_commit {
                    match agent.commit_workspace_changes(&p).await {
                        Ok(Some(commit)) => {
                            if !parsed.json_mode {
                                println!(
                                    "[Agendex] Committed workspace changes: {} - {}",
                                    commit.commit_sha, commit.message
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            if !parsed.json_mode {
                                eprintln!("[Error] Automated git commit failed: {}", err);
                            }
                        }
                    }
                }
            }
            Err(err) => {
                if !parsed.json_mode {
                    eprintln!("\n[Error]: {}\n", err);
                }
                return Err(err);
            }
        }
    } else {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() && !parsed.json_mode {
            let mut commands = vec![
                agendex_cli::editor::SlashCommandInfo {
                    name: "/help".into(),
                    description: "Show all built-in commands and discovered skills".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/model".into(),
                    description: "Open interactive model and provider selector".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/provider".into(),
                    description: "Switch model or view provider list".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/diff".into(),
                    description: "View git diff of agent modifications this session".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/undo".into(),
                    description: "Undo the last turn and restore snapshot".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/revert".into(),
                    description: "Undo the last turn and restore snapshot".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/yolo".into(),
                    description: "Toggle auto-approval mode on/off dynamically".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/cost".into(),
                    description: "Show session token cost breakdown in USD".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/tokens".into(),
                    description: "Show detailed token usage breakdown".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/context".into(),
                    description: "Display visual context window capacity gauge".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/clear".into(),
                    description: "Clear terminal screen".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/new".into(),
                    description: "Start a fresh conversation session".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/resume".into(),
                    description: "List or resume past session trajectories".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/config".into(),
                    description: "Display current configuration parameters".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/skills".into(),
                    description: "List all discovered skills and their source paths".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/mcp".into(),
                    description: "List connected MCP servers and available tools".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/tools".into(),
                    description: "List all available native harness and MCP tools".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/init".into(),
                    description: "Scan project layout and bootstrap AGENTS.md".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/compact".into(),
                    description: "Manually trigger context compaction".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/def".into(),
                    description: "Find AST symbol definition in codebase".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/exit".into(),
                    description: "Exit interactive session".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/quit".into(),
                    description: "Exit interactive session".into(),
                },
            ];

            if let Ok(reg) = agent.skill_registry.read() {
                for s in reg.list_skills() {
                    commands.push(agendex_cli::editor::SlashCommandInfo {
                        name: s.command.clone(),
                        description: s.description.clone(),
                    });
                }
            }

            let mut line_editor = agendex_cli::editor::create_reedline_editor(
                &config.workspace_dir,
                commands,
            )?;

            loop {
                let prompt = agendex_cli::editor::AgendexPrompt {
                    model_name: agent.active_model_config.read().await.model_name.clone(),
                    git_branch: agendex_cli::editor::detect_git_branch(&config.workspace_dir),
                    yolo_mode: agent.auto_approve_yolo.load(std::sync::atomic::Ordering::SeqCst),
                };

                if let Some(ref queue) = approval_queue {
                    queue.pause();
                }

                let sig = line_editor.read_line(&prompt);

                if let Some(ref queue) = approval_queue {
                    queue.resume();
                }

                match sig {
                    Ok(reedline::Signal::Success(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if trimmed == "exit"
                            || trimmed == "quit"
                            || trimmed == "/exit"
                            || trimmed == "/quit"
                        {
                            println!("Exiting Agendex session.");
                            break;
                        }

                        if trimmed == "/clear" {
                            let _ = crossterm::execute!(
                                std::io::stdout(),
                                crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
                                crossterm::cursor::MoveTo(0, 0)
                            );
                            continue;
                        }

                        if trimmed == "/model"
                            || trimmed == "/models"
                            || trimmed == "/provider"
                            || trimmed == "/providers"
                            || trimmed == "/model select"
                            || trimmed == "/provider select"
                        {
                            if let Some(ref queue) = approval_queue {
                                queue.pause();
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));

                            let active_prov = agent.active_provider_name.read().await.clone();
                            let active_mdl = agent.active_model_config.read().await.model_name.clone();

                            let selection_res = agendex_cli::model_selector::launch_model_selector(
                                &active_prov,
                                &active_mdl,
                            );

                            if let Some(ref queue) = approval_queue {
                                queue.resume();
                            }

                            match selection_res {
                                Ok(Some(choice)) => {
                                    match agent
                                        .switch_provider(&choice.provider_key, Some(&choice.model_name))
                                        .await
                                    {
                                        Ok(_) => {
                                            println!(
                                                "  {}✔{} Switched to {}{}{} {}({}){}",
                                                agendex_core::ui::ACCENT_GREEN,
                                                agendex_core::ui::RESET,
                                                agendex_core::ui::TEXT_PRIMARY,
                                                choice.model_name,
                                                agendex_core::ui::RESET,
                                                agendex_core::ui::ACCENT_CYAN,
                                                choice.provider_tag,
                                                agendex_core::ui::RESET
                                            );
                                            if let Some(ref url) = choice.base_url {
                                                println!(
                                                    "    {}Endpoint: {}{}",
                                                    agendex_core::ui::TEXT_MUTED,
                                                    url,
                                                    agendex_core::ui::RESET
                                                );
                                            }
                                            println!();
                                        }
                                        Err(err) => {
                                            eprintln!("\n[Error] Failed to switch model: {}\n", err);
                                        }
                                    }
                                }
                                Ok(None) => {
                                    println!(
                                        "  {}Selection cancelled.{}",
                                        agendex_core::ui::TEXT_MUTED,
                                        agendex_core::ui::RESET
                                    );
                                }
                                Err(err) => {
                                    eprintln!("\n[Error] Selector error: {}\n", err);
                                }
                            }
                            continue;
                        }

                        let expanded = agendex_cli::editor::expand_file_mentions(
                            &config.workspace_dir,
                            trimmed,
                        );

                        tokio::select! {
                            turn_res = agent.process_turn(&expanded) => {
                                match turn_res {
                                    Ok(reply) => {
                                        if agent.streamed_tokens_in_last_turn() {
                                            println!();
                                        } else {
                                            let skin = termimad::MadSkin::default();
                                            skin.print_text(&reply);
                                        }
                                        if let Some(telemetry) = agent.format_telemetry().await {
                                            println!(
                                                "{}",
                                                agendex_core::ui::format_telemetry_footer(&telemetry)
                                            );
                                        }
                                        if parsed.git_commit {
                                            let _ = agent.commit_workspace_changes(trimmed).await;
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!("\n[Error]: {}\n", err);
                                    }
                                }
                            }
                            _ = tokio::signal::ctrl_c() => {
                                println!(
                                    "\n  {}⚡ Turn interrupted by user (Ctrl-C). Trajectory preserved.{}",
                                    agendex_core::ui::ACCENT_AMBER,
                                    agendex_core::ui::RESET
                                );
                            }
                        }
                    }
                    Ok(reedline::Signal::CtrlC) => {
                        println!();
                        continue;
                    }
                    Ok(reedline::Signal::CtrlD) => {
                        println!("Exiting Agendex session.");
                        break;
                    }
                    Ok(_) => {
                        continue;
                    }
                    Err(err) => {
                        eprintln!("[Editor Error]: {}", err);
                        break;
                    }
                }
            }
        } else {
            let _prompt_str = agendex_core::ui::prompt_string();
            let mut line = String::new();
            while std::io::stdin().read_line(&mut line).is_ok() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    line.clear();
                    continue;
                }
                if trimmed == "exit" || trimmed == "quit" {
                    break;
                }
                match agent.process_turn(trimmed).await {
                    Ok(reply) => {
                        if !parsed.json_mode {
                            println!("{}", reply);
                        }
                    }
                    Err(err) => {
                        if !parsed.json_mode {
                            eprintln!("[Error]: {}", err);
                        }
                    }
                }
                line.clear();
            }
        }
    }

    Ok(())
}

fn print_help() {
    println!(
        r#"Usage: agendex [OPTIONS]

Options:
  -p, --prompt <TEXT>       Execute a single prompt non-interactively
  --format <gemini|openai|anthropic> Wire protocol format (default: openai)
  --model <NAME>            Model name (e.g. gemini-2.5-flash, gpt-4o, claude-3-5-sonnet)
  --base-url <URL>          Base URL (for local gateways e.g. http://localhost:8045)
  --api-key <KEY>           API key for upstream provider
  --conversation <ID>       Resume or target a specific conversation trajectory
  --tier <32k|64k|128k|200k|256k|840k|1m|2m|custom> Compaction token threshold ceiling (default: 128k)
  --no-auto-compact         Disable automatic trajectory compaction
  --mcp-config <PATH>       Path to JSON configuration declaring MCP servers
  -y, --auto-approve        Auto-approve risky commands without interactive prompt
  --json                    Enable headless JSON lines event streaming
  --git-commit              Automatically commit workspace changes upon task completion
  --git-branch <BRANCH>     Create or checkout specified Git branch before execution
  -h, --help                Show this help message
"#
    );
}
