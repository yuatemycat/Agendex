use std::io::IsTerminal;
use std::path::Path;

/// Returns true if stdout is an interactive terminal and NO_COLOR is not set.
pub fn is_color_enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var("NO_COLOR").is_err()
}

/// Strips ANSI escape sequences from text.
pub fn strip_ansi(input: &str) -> String {
    static ANSI_REGEX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = ANSI_REGEX.get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap());
    re.replace_all(input, "").to_string()
}

/// Returns text with ANSI styling if stdout is a terminal, or plain text if not.
pub fn sanitize_output_color(input: &str) -> String {
    if is_color_enabled() {
        input.to_string()
    } else {
        strip_ansi(input)
    }
}

/// Initialize console on Windows to support 24-bit ANSI color escape codes.
#[cfg(windows)]
pub fn init_terminal() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        fn GetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, lpMode: *mut u32) -> i32;
        fn SetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, dwMode: u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = 0xFFFFFFF5;
    const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    unsafe {
        for handle_id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(handle_id);
            if !handle.is_null() && handle != (-1isize as *mut std::ffi::c_void) {
                let mut mode: u32 = 0;
                if GetConsoleMode(handle, &mut mode) != 0 {
                    let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
                }
            }
        }
    }
}

#[cfg(not(windows))]
pub fn init_terminal() {}

// Styling constants using 24-bit RGB and modern ANSI palette
pub const BRAND_VIOLET: &str = "\x1b[1;38;2;129;140;248m"; // #818cf8 (Indigo/Violet)
pub const ACCENT_CYAN: &str = "\x1b[38;2;56;189;248m"; // #38bdf8 (Sky Cyan)
pub const ACCENT_AMBER: &str = "\x1b[38;2;251;191;36m"; // #fbbf24 (Warm Amber)
pub const ACCENT_GREEN: &str = "\x1b[38;2;74;222;128m"; // #4ade80 (Emerald)
pub const ACCENT_RED: &str = "\x1b[38;2;248;113;113m"; // #f87171 (Coral Red)
pub const TEXT_PRIMARY: &str = "\x1b[1;38;2;241;245;249m"; // #f1f5f9 (Near White)
pub const TEXT_SECONDARY: &str = "\x1b[38;2;148;163;184m"; // #94a3b8 (Slate 400)
pub const TEXT_MUTED: &str = "\x1b[38;2;100;116;139m"; // #64748b (Slate 500)
pub const RESET: &str = "\x1b[0m";

/// Prints a clean, modern startup header matching the aesthetic of AGY CLI, Claude Code, and Hermes.
pub fn print_banner(
    workspace: &Path,
    model_name: &str,
    provider_name: &str,
    base_url: Option<&str>,
) {
    let workspace_str = workspace.display().to_string();
    let display_model = if model_name.is_empty() {
        "default"
    } else {
        model_name
    };
    let lower = provider_name.to_lowercase();
    let is_local_endpoint = base_url
        .map(|u| {
            let u_low = u.to_lowercase();
            u_low.contains("127.0.0.1")
                || u_low.contains("localhost")
                || u_low.contains("0.0.0.0")
                || u_low.starts_with("http://192.168.")
                || u_low.starts_with("http://10.")
        })
        .unwrap_or(false);

    let is_nim_endpoint = base_url
        .map(|u| {
            let u_low = u.to_lowercase();
            u_low.contains("nvidia.com") || u_low.contains("nim")
        })
        .unwrap_or(false)
        || display_model.to_lowercase().contains("nemotron");

    let provider_display = if lower == "nim" || lower == "nvidia" || is_nim_endpoint {
        "Nvidia NIM"
    } else {
        match lower.as_str() {
            "strata" => "Strata (125B MoE Local)",
            "proxymateur" => "Proxymateur Gateway",
            "ollama" => "Ollama Local",
            "gemini" => "Google Gemini",
            "claude" => "Anthropic Claude",
            "openai" => {
                if is_local_endpoint {
                    "Custom Local Endpoint"
                } else {
                    "OpenAI"
                }
            }
            _ => {
                if is_local_endpoint && (provider_name.is_empty() || lower == "custom") {
                    "Custom Local Endpoint"
                } else {
                    provider_name
                }
            }
        }
    };

    println!();
    println!(
        "{}",
        sanitize_output_color(&format!(
            "  {}✦ Agendex{} {}v0.1.0{} {}(local-first autonomous agent){}",
            BRAND_VIOLET, RESET, TEXT_MUTED, RESET, TEXT_MUTED, RESET,
        ))
    );
    println!(
        "{}",
        sanitize_output_color(&format!(
            "  {}Model:     {} {}{}{} {}({}){}",
            TEXT_MUTED,
            RESET,
            TEXT_PRIMARY,
            display_model,
            RESET,
            ACCENT_CYAN,
            provider_display,
            RESET,
        ))
    );
    if let Some(url) = base_url {
        if !url.is_empty() && !url.contains("googleapis") && !url.contains("anthropic") {
            println!(
                "{}",
                sanitize_output_color(&format!(
                    "  {}Endpoint:  {} {}{}{}",
                    TEXT_MUTED, RESET, TEXT_SECONDARY, url, RESET,
                ))
            );
        }
    }
    println!(
        "{}",
        sanitize_output_color(&format!(
            "  {}Workspace: {} {}{}{}",
            TEXT_MUTED, RESET, TEXT_SECONDARY, workspace_str, RESET,
        ))
    );
    println!();
    println!(
        "{}",
        sanitize_output_color(&format!(
            "  {}Type {}/model{} to switch models, {}/help{} for skills, or ask a question.{}",
            TEXT_MUTED, ACCENT_CYAN, TEXT_MUTED, ACCENT_CYAN, TEXT_MUTED, RESET,
        ))
    );
    println!();
}

/// Returns the styled interactive REPL prompt string.
pub fn prompt_string() -> String {
    sanitize_output_color(&format!(
        "{}agendex{} {}❯{} ",
        BRAND_VIOLET, RESET, ACCENT_CYAN, RESET
    ))
}

/// Formats a tool progress notification cleanly, suppressing internal runtime noise.
pub fn format_tool_notification(raw_msg: &str) -> Option<String> {
    let trimmed = raw_msg.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("Generating response")
        || trimmed.starts_with("Model requested")
        || trimmed.ends_with("Completed.")
    {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix("[Tool: run_command] Executing '") {
        let cmd = rest.strip_suffix('\'').unwrap_or(rest);
        return Some(sanitize_output_color(&format!(
            "  {}⚡{} {}run_command{} {}{}{}",
            ACCENT_AMBER, RESET, TEXT_PRIMARY, RESET, TEXT_SECONDARY, cmd, RESET
        )));
    }

    if let Some(rest) = trimmed.strip_prefix("[Tool: edit_file] Patching '") {
        let path = rest.strip_suffix('\'').unwrap_or(rest);
        return Some(sanitize_output_color(&format!(
            "  {}📝{} {}edit_file{}   {}{}{}",
            ACCENT_CYAN, RESET, TEXT_PRIMARY, RESET, TEXT_SECONDARY, path, RESET
        )));
    }

    if let Some(rest) = trimmed.strip_prefix("[Tool: write_file] Writing '") {
        let path = rest.strip_suffix('\'').unwrap_or(rest);
        return Some(sanitize_output_color(&format!(
            "  {}📝{} {}write_file{}  {}{}{}",
            ACCENT_CYAN, RESET, TEXT_PRIMARY, RESET, TEXT_SECONDARY, path, RESET
        )));
    }

    if let Some(rest) = trimmed.strip_prefix("[Tool: read_file] Reading '") {
        let path = rest.strip_suffix('\'').unwrap_or(rest);
        return Some(sanitize_output_color(&format!(
            "  {}📖{} {}read_file{}   {}{}{}",
            TEXT_MUTED, RESET, TEXT_PRIMARY, RESET, TEXT_SECONDARY, path, RESET
        )));
    }

    if let Some(rest) = trimmed.strip_prefix("[Tool: ") {
        if let Some((tool_name, rem)) = rest.split_once(']') {
            let action = rem.trim();
            return Some(sanitize_output_color(&format!(
                "  {}⚡{} {}{}{} {}{}{}",
                ACCENT_AMBER, RESET, TEXT_PRIMARY, tool_name, RESET, TEXT_SECONDARY, action, RESET
            )));
        }
    }

    Some(sanitize_output_color(&format!(
        "  {}⚡{} {}{}{}",
        ACCENT_AMBER, RESET, TEXT_SECONDARY, trimmed, RESET
    )))
}

/// Formats a git diff with color highlights (red for deletions, green for additions).
pub fn format_diff_colored(path: &str, diff: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\n{}╭─ 📝 Proposed change: {}{}{}\n",
        TEXT_MUTED, ACCENT_CYAN, path, TEXT_MUTED
    ));

    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            out.push_str(&format!(
                "{}│{} {}{}{}\n",
                TEXT_MUTED, RESET, ACCENT_GREEN, line, RESET
            ));
        } else if line.starts_with('-') && !line.starts_with("---") {
            out.push_str(&format!(
                "{}│{} {}{}{}\n",
                TEXT_MUTED, RESET, ACCENT_RED, line, RESET
            ));
        } else if line.starts_with("@@") {
            out.push_str(&format!(
                "{}│{} {}{}{}\n",
                TEXT_MUTED, RESET, ACCENT_CYAN, line, RESET
            ));
        } else {
            out.push_str(&format!(
                "{}│{} {}{}{}\n",
                TEXT_MUTED, RESET, TEXT_SECONDARY, line, RESET
            ));
        }
    }
    out.push_str(&format!(
        "{}╰────────────────────────────────────────────────────────────{}\n",
        TEXT_MUTED, RESET
    ));
    sanitize_output_color(&out)
}

/// Formats safety warning card for risky shell command executions.
pub fn format_safety_gate_card(command: &str, reason: &str, subagent_id: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\n{}╭─ ⚠️  Permission Request ────────────────────────────────────{}\n",
        ACCENT_AMBER, RESET
    ));
    if let Some(id) = subagent_id {
        if !id.is_empty() && id != "parent" {
            out.push_str(&format!(
                "{}│{} {}Agent:  {} {}{}{}\n",
                ACCENT_AMBER, RESET, TEXT_MUTED, RESET, TEXT_PRIMARY, id, RESET
            ));
        }
    }
    out.push_str(&format!(
        "{}│{} {}Command:{} {}{}{}\n",
        ACCENT_AMBER, RESET, TEXT_MUTED, RESET, TEXT_PRIMARY, command, RESET
    ));
    out.push_str(&format!(
        "{}│{} {}Reason: {} {}{}{}\n",
        ACCENT_AMBER, RESET, TEXT_MUTED, RESET, TEXT_SECONDARY, reason, RESET
    ));
    out.push_str(&format!(
        "{}╰────────────────────────────────────────────────────────────{}\n",
        ACCENT_AMBER, RESET
    ));
    sanitize_output_color(&out)
}

/// Formats the final turn telemetry footer cleanly.
pub fn format_telemetry_footer(telemetry: &str) -> String {
    sanitize_output_color(&format!(
        "\n  {}────────────────────────────────────────────────────{}\n  {}{}{}\n",
        TEXT_MUTED, RESET, TEXT_MUTED, telemetry, RESET
    ))
}
