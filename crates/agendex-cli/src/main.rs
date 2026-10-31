use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use agendex_core::agent::AgendexAgent;

#[derive(Clone)]
struct SpinnerController {
    running: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}

impl SpinnerController {
    fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new(AtomicBool::new(true)),
        }
    }

    fn start(&self, message: &'static str) -> tokio::task::JoinHandle<()> {
        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.stopped
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let r_clone = self.running.clone();
        let s_clone = self.stopped.clone();
        let is_term = std::io::stdout().is_terminal() && env::var("NO_COLOR").is_err();

        tokio::spawn(async move {
            if !is_term {
                s_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut idx = 0;
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            while r_clone.load(std::sync::atomic::Ordering::Relaxed) {
                let frame = frames[idx % frames.len()];
                eprint!(
                    "\r  \x1b[38;2;129;140;248m{}\x1b[0m \x1b[38;2;148;163;184m{}\x1b[0m",
                    frame, message
                );
                let _ = io::stderr().flush();
                idx += 1;
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            }
            eprint!("\r\x1b[2K");
            let _ = io::stderr().flush();
            s_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    }

    fn stop(&self) {
        if self
            .running
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            let start = std::time::Instant::now();
            while !self.stopped.load(std::sync::atomic::Ordering::SeqCst)
                && start.elapsed().as_millis() < 200
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            eprint!("\r\x1b[2K");
            let _ = io::stderr().flush();
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agendex_core::config::load_agendex_env();
    let args: Vec<String> = env::args().collect();
    let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let parsed = agendex_cli::parse_cli_args(&args, current_dir);
    if parsed.show_help {
        print_help();
        return Ok(());
    }

    let is_one_shot = parsed.prompt.is_some();
    // Direct tracing logs to stderr to keep stdout 100% clean for output/JSON
    if parsed.json_mode || is_one_shot {
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(io::stderr)
            .try_init();
    } else {
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(io::stderr)
            .try_init();
    }

    agendex_core::ui::init_terminal();

    let config = parsed.config;
    let prompt = parsed.prompt;

    let active_provider = parsed.provider.unwrap_or_else(|| {
        if let Some(cfg) = agendex_core::config::load_default_config() {
            cfg.active_provider.unwrap_or_else(|| "nim".to_string())
        } else {
            "nim".to_string()
        }
    });

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

    let spinner_ctrl = SpinnerController::new();
    if let Some(ref queue) = approval_queue {
        let sc_q = spinner_ctrl.clone();
        queue.set_activity_hook(Arc::new(move || {
            sc_q.stop();
        }));
    }
    let stream_filter = Arc::new(std::sync::Mutex::new(
        agendex_trajectory::StreamingSecretFilter::new(),
    ));

    let agent = if parsed.json_mode {
        agent.with_event_handler(Arc::new(|evt| {
            if let Ok(line) = serde_json::to_string(evt) {
                println!("{}", line);
                let _ = io::stdout().flush();
            }
        }))
    } else {
        let sc_token = spinner_ctrl.clone();
        let sf_token = stream_filter.clone();
        let sc_tool = spinner_ctrl.clone();
        let sc_act = spinner_ctrl.clone();
        agent
            .with_activity_hook(Arc::new(move || {
                sc_act.stop();
            }))
            .with_token_stream_handler(Arc::new(move |token| {
                sc_token.stop();
                let mut filter = sf_token.lock().unwrap();
                let clean = filter.push(token);
                if !clean.is_empty() {
                    print!("{}", clean);
                    let _ = io::stdout().flush();
                }
            }))
            .with_tool_progress_handler(Arc::new(move |msg| {
                sc_tool.stop();
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
        let expanded_prompt = agendex_cli::editor::expand_file_mentions(&config.workspace_dir, &p);
        let spinner_handle = if !parsed.json_mode {
            Some(spinner_ctrl.start("Thinking..."))
        } else {
            None
        };
        let turn_res = agent.process_turn(&expanded_prompt).await;
        if let Some(sh) = spinner_handle {
            spinner_ctrl.stop();
            let _ = sh.await;
        }
        if !parsed.json_mode {
            let flushed = stream_filter.lock().unwrap().flush();
            if !flushed.is_empty() {
                print!("{}", flushed);
                let _ = io::stdout().flush();
            }
        }
        match turn_res {
            Ok(resp) => {
                if !parsed.json_mode {
                    let clean_resp = agendex_trajectory::mask_secrets(&resp);
                    if agent.streamed_tokens_in_last_turn() {
                        println!();
                    } else {
                        println!("{}", clean_resp);
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
                std::process::exit(1);
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
                    name: "/swarm".into(),
                    description: "Spawn parallel autonomous subagent fleet".into(),
                },
                agendex_cli::editor::SlashCommandInfo {
                    name: "/battle".into(),
                    description: "Run adversarial duel between dual personas in worktrees".into(),
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

            let mut line_editor =
                agendex_cli::editor::create_reedline_editor(&config.workspace_dir, commands)?;

            loop {
                let prompt = agendex_cli::editor::AgendexPrompt {
                    model_name: agent.active_model_config.read().await.model_name.clone(),
                    git_branch: agendex_cli::editor::detect_git_branch(&config.workspace_dir),
                    yolo_mode: agent
                        .auto_approve_yolo
                        .load(std::sync::atomic::Ordering::SeqCst),
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

                        let sanitized = agendex_cli::editor::sanitize_history_line(trimmed);
                        if sanitized != trimmed && std::io::stdout().is_terminal() {
                            use crossterm::{cursor, terminal, ExecutableCommand};
                            use reedline::Prompt;
                            let _ =
                                std::io::stdout()
                                    .execute(cursor::MoveUp(1))
                                    .and_then(|stdout| {
                                        stdout.execute(terminal::Clear(
                                            terminal::ClearType::CurrentLine,
                                        ))
                                    });
                            let p_str = prompt.render_prompt_left();
                            println!("{}{}", p_str, sanitized);
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
                            let active_mdl =
                                agent.active_model_config.read().await.model_name.clone();

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
                                        .switch_provider(
                                            &choice.provider_key,
                                            Some(&choice.model_name),
                                        )
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
                                            eprintln!(
                                                "\n[Error] Failed to switch model: {}\n",
                                                err
                                            );
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

                        let spinner_handle = if !parsed.json_mode {
                            Some(spinner_ctrl.start("Thinking..."))
                        } else {
                            None
                        };
                        tokio::select! {
                            turn_res = agent.process_turn(&expanded) => {
                                if let Some(sh) = spinner_handle {
                                    spinner_ctrl.stop();
                                    let _ = sh.await;
                                }
                                let flushed = stream_filter.lock().unwrap().flush();
                                if !flushed.is_empty() {
                                    print!("{}", flushed);
                                    let _ = io::stdout().flush();
                                }
                                match turn_res {
                                    Ok(reply) => {
                                        let clean_reply = agendex_trajectory::mask_secrets(&reply);
                                        if agent.streamed_tokens_in_last_turn() {
                                            println!();
                                        } else if std::io::stdout().is_terminal() && env::var("NO_COLOR").is_err() {
                                            let skin = termimad::MadSkin::default();
                                            skin.print_text(&clean_reply);
                                        } else {
                                            println!("{}", clean_reply);
                                        }
                                        if !trimmed.starts_with('/') {
                                            if let Some(telemetry) = agent.format_telemetry().await {
                                                println!(
                                                    "{}",
                                                    agendex_core::ui::format_telemetry_footer(&telemetry)
                                                );
                                            }
                                        }
                                        if parsed.git_commit {
                                            let _ = agent.commit_workspace_changes(trimmed).await;
                                        }
                                    }
                                    Err(err) => {
                                        spinner_ctrl.stop();
                                        eprintln!("\n[Error]: {}\n", err);
                                    }
                                }
                            }
                            _ = tokio::signal::ctrl_c() => {
                                if let Some(sh) = spinner_handle {
                                    spinner_ctrl.stop();
                                    let _ = sh.await;
                                }
                                let flushed = stream_filter.lock().unwrap().flush();
                                if !flushed.is_empty() {
                                    print!("{}", flushed);
                                    let _ = io::stdout().flush();
                                }
                                if let Some(ref queue) = approval_queue {
                                    queue.cancel_pending();
                                }
                                agent.record_interrupted_turn().await;
                                println!(
                                    "\n  {}⚡ Turn interrupted by user (Ctrl-C). Trajectory and token cost preserved.{}",
                                    agendex_core::ui::ACCENT_AMBER,
                                    agendex_core::ui::RESET
                                );
                            }
                        }
                    }
                    Ok(reedline::Signal::CtrlC) => {
                        if let Some(ref queue) = approval_queue {
                            queue.cancel_pending();
                        }
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
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) => break, // EOF reached cleanly in pipe mode
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if trimmed == "exit"
                            || trimmed == "quit"
                            || trimmed == "/exit"
                            || trimmed == "/quit"
                        {
                            break;
                        }
                        match agent.process_turn(trimmed).await {
                            Ok(reply) => {
                                if !parsed.json_mode {
                                    println!("{}", agendex_trajectory::mask_secrets(&reply));
                                }
                            }
                            Err(err) => {
                                if !parsed.json_mode {
                                    eprintln!("[Error]: {}", err);
                                }
                            }
                        }
                    }
                    Err(_) => break,
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
  --json                    Enable headless JSON lines event streaming
  --git-commit              Automatically commit workspace changes upon task completion
  --git-branch <BRANCH>     Create or checkout specified Git branch before execution
  -h, --help                Show this help message
"#
    );
}
