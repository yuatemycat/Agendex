use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, BufReader};

use agendex_core::agent::AgendexAgent;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    println!("=======================================================");
    println!(" Agendex Agent Runtime v0.1.0");
    println!("=======================================================");

    let parsed = agendex_cli::parse_cli_args(&args, current_dir);
    if parsed.show_help {
        print_help();
        return Ok(());
    }

    let config = parsed.config;
    let prompt = parsed.prompt;

    println!("[Agendex] Workspace: {}", config.workspace_dir.display());
    println!("[Agendex] Session ID: {}", config.session_id);
    println!(
        "[Agendex] Model Format: {:?} ({})",
        config.model.format, config.model.model_name
    );
    println!("[Agendex] Compaction Ceiling: {:?}", config.model_tier);

    let agent = AgendexAgent::init(config.clone()).await?;
    // Prompt for risky commands whenever a person is at the terminal, including
    // one-shot `-p` runs. Without a terminal (CI, pipes), risky commands are
    // refused unless `--yes` was passed.
    use std::io::IsTerminal;
    let agent = if !config.auto_approve_risky && std::io::stdin().is_terminal() {
        agent.with_approval_handler(std::sync::Arc::new(|cmd, reason| {
            print!(
                "\n[Safety Gate] Command is classified as risky ({}):\n  {}\nAllow execution? [y/N]: ",
                reason, cmd
            );
            let _ = std::io::stdout().flush();
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).is_ok() {
                let trimmed = input.trim().to_lowercase();
                trimmed == "y" || trimmed == "yes"
            } else {
                false
            }
        }))
    } else {
        agent
    };
    println!("[Agendex] Ready. Skills, Rules, and Execution Core mounted.\n");

    if let Some(p) = prompt {
        println!("[User]: {}", p);
        let resp = agent.process_turn(&p).await?;
        println!("\n[Agendex]:\n{}", resp);
    } else {
        println!("Interactive REPL. Type '/help' for skills, '/compact' to compress, or 'exit' to quit.\n");
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();

        loop {
            print!("agendex> ");
            io::stdout().flush()?;

            match reader.next_line().await {
                Ok(Some(line)) => {
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

                    match agent.process_turn(trimmed).await {
                        Ok(reply) => {
                            println!("\n[Agendex]:\n{}\n", reply);
                        }
                        Err(err) => {
                            eprintln!("\n[Error]: {}\n", err);
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Error reading stdin: {}", e);
                    break;
                }
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
  -h, --help                Show this help message
"#
    );
}
