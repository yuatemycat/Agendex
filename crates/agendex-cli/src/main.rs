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

    if !parsed.json_mode {
        println!("=======================================================");
        println!(" Agendex Agent Runtime v0.1.0");
        println!("=======================================================");
    }

    let config = parsed.config;
    let prompt = parsed.prompt;

    if !parsed.json_mode {
        println!("[Agendex] Workspace: {}", config.workspace_dir.display());
        println!("[Agendex] Session ID: {}", config.session_id);
        println!(
            "[Agendex] Model Format: {:?} ({})",
            config.model.format, config.model.model_name
        );
        println!("[Agendex] Compaction Ceiling: {:?}", config.model_tier);
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
                println!("{}", msg);
            }))
    };

    if !parsed.json_mode {
        println!("[Agendex] Ready. Skills, Rules, and Execution Core mounted.\n");
    }

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
        if !parsed.json_mode {
            println!("[User]: {}", p);
        }
        let turn_res = agent.process_turn(&p).await;
        match turn_res {
            Ok(resp) => {
                if !parsed.json_mode {
                    println!("\n[Agendex]:\n{}", resp);
                    if let Some(telemetry) = agent.format_telemetry().await {
                        println!("{}", telemetry);
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
        if !parsed.json_mode {
            println!("Interactive REPL. Type '/help' for skills, '/compact' to compress, or 'exit' to quit.\n");
        }

        loop {
            let next_line = if let Some(queue) = &approval_queue {
                queue.read_line("agendex> ").await
            } else {
                if !parsed.json_mode {
                    print!("agendex> ");
                    io::stdout().flush()?;
                }
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line).is_ok() {
                    Some(line)
                } else {
                    None
                }
            };

            match next_line {
                Some(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if trimmed == "exit"
                        || trimmed == "quit"
                        || trimmed == "/exit"
                        || trimmed == "/quit"
                    {
                        if !parsed.json_mode {
                            println!("Exiting Agendex session.");
                        }
                        break;
                    }

                    match agent.process_turn(trimmed).await {
                        Ok(reply) => {
                            if !parsed.json_mode {
                                println!("\n[Agendex]:\n{}\n", reply);
                                if let Some(telemetry) = agent.format_telemetry().await {
                                    println!("{}\n", telemetry);
                                }
                            }
                            // Automated Git Commit helper in REPL
                            if parsed.git_commit {
                                match agent.commit_workspace_changes(trimmed).await {
                                    Ok(Some(commit)) => {
                                        if !parsed.json_mode {
                                            println!(
                                                "[Agendex] Committed: {} ({})",
                                                commit.commit_sha, commit.message
                                            );
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(err) => {
                                        if !parsed.json_mode {
                                            eprintln!("[Error] Git commit failed: {}", err);
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            if !parsed.json_mode {
                                eprintln!("\n[Error]: {}\n", err);
                            }
                        }
                    }
                }
                None => break,
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
