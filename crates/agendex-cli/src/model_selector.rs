use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use std::io::{stdout, Write};

#[derive(Debug, Clone)]
pub struct ModelOption {
    pub provider_key: String,
    pub provider_title: String,
    pub model_name: String,
    pub description: String,
    pub base_url: Option<String>,
    pub format: String,
    pub is_category_header: bool,
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut out = stdout();
        let _ = execute!(out, Show, LeaveAlternateScreen);
        let _ = out.flush();
    }
}

/// Builds the comprehensive list of available models and providers.
#[allow(clippy::vec_init_then_push)]
pub fn build_model_options() -> Vec<ModelOption> {
    let cfg = agendex_core::config::load_default_config()
        .unwrap_or_else(agendex_core::config::create_default_config_with_presets);

    let mut options = Vec::new();

    // 1. Nvidia NIM
    options.push(ModelOption {
        provider_key: "nim".to_string(),
        provider_title: "Nvidia NIM (Cloud Acceleration)".to_string(),
        model_name: "".to_string(),
        description: "".to_string(),
        base_url: None,
        format: "".to_string(),
        is_category_header: true,
    });
    options.push(ModelOption {
        provider_key: "nim".to_string(),
        provider_title: "Nvidia NIM".to_string(),
        model_name: "meta/llama-3.1-70b-instruct".to_string(),
        description: "Llama 3.1 70B · High-speed reasoning & tool calling".to_string(),
        base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
        format: "openai".to_string(),
        is_category_header: false,
    });
    options.push(ModelOption {
        provider_key: "nim".to_string(),
        provider_title: "Nvidia NIM".to_string(),
        model_name: "mistralai/mistral-large-2-instruct".to_string(),
        description: "Mistral Large 2 · 128k context, strong multilingual".to_string(),
        base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
        format: "openai".to_string(),
        is_category_header: false,
    });
    options.push(ModelOption {
        provider_key: "nim".to_string(),
        provider_title: "Nvidia NIM".to_string(),
        model_name: "nvidia/llama-3.1-nemotron-70b-instruct".to_string(),
        description: "Nemotron 70B · Specialized code alignment & math".to_string(),
        base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
        format: "openai".to_string(),
        is_category_header: false,
    });

    // 2. Strata Local 125B MoE
    options.push(ModelOption {
        provider_key: "strata".to_string(),
        provider_title: "Strata Local MoE (On-Device Inference)".to_string(),
        model_name: "".to_string(),
        description: "".to_string(),
        base_url: None,
        format: "".to_string(),
        is_category_header: true,
    });
    options.push(ModelOption {
        provider_key: "strata".to_string(),
        provider_title: "Strata".to_string(),
        model_name: "huihui-swift-1.5".to_string(),
        description: "125B MoE Local Engine · Zero cloud egress · 127.0.0.1:8080".to_string(),
        base_url: Some("http://127.0.0.1:8080/v1".to_string()),
        format: "openai".to_string(),
        is_category_header: false,
    });

    // 3. Proxymateur Multi-Model Gateway
    options.push(ModelOption {
        provider_key: "proxymateur".to_string(),
        provider_title: "Proxymateur Gateway (Antigravity Proxy)".to_string(),
        model_name: "".to_string(),
        description: "".to_string(),
        base_url: None,
        format: "".to_string(),
        is_category_header: true,
    });
    options.push(ModelOption {
        provider_key: "proxymateur".to_string(),
        provider_title: "Proxymateur".to_string(),
        model_name: "gemini-2.5-flash".to_string(),
        description: "Gemini 2.5 Flash via Local Gateway · 127.0.0.1:8045".to_string(),
        base_url: Some("http://127.0.0.1:8045/v1".to_string()),
        format: "openai".to_string(),
        is_category_header: false,
    });
    options.push(ModelOption {
        provider_key: "proxymateur".to_string(),
        provider_title: "Proxymateur".to_string(),
        model_name: "claude-3-7-sonnet".to_string(),
        description: "Claude 3.7 Sonnet routed through Proxymateur".to_string(),
        base_url: Some("http://127.0.0.1:8045/v1".to_string()),
        format: "openai".to_string(),
        is_category_header: false,
    });

    // 4. Ollama Local
    options.push(ModelOption {
        provider_key: "ollama".to_string(),
        provider_title: "Ollama (Local Open-Source LLMs)".to_string(),
        model_name: "".to_string(),
        description: "".to_string(),
        base_url: None,
        format: "".to_string(),
        is_category_header: true,
    });
    options.push(ModelOption {
        provider_key: "ollama".to_string(),
        provider_title: "Ollama".to_string(),
        model_name: "qwen2.5-coder:7b".to_string(),
        description: "Qwen 2.5 Coder 7B · Fast local code autocomplete & edits".to_string(),
        base_url: Some("http://localhost:11434/v1".to_string()),
        format: "openai".to_string(),
        is_category_header: false,
    });
    options.push(ModelOption {
        provider_key: "ollama".to_string(),
        provider_title: "Ollama".to_string(),
        model_name: "deepseek-coder-v2:16b".to_string(),
        description: "DeepSeek Coder V2 16B · High precision local coding".to_string(),
        base_url: Some("http://localhost:11434/v1".to_string()),
        format: "openai".to_string(),
        is_category_header: false,
    });

    // 5. Cloud APIs Direct
    options.push(ModelOption {
        provider_key: "cloud".to_string(),
        provider_title: "Direct Cloud Provider APIs".to_string(),
        model_name: "".to_string(),
        description: "".to_string(),
        base_url: None,
        format: "".to_string(),
        is_category_header: true,
    });
    options.push(ModelOption {
        provider_key: "claude".to_string(),
        provider_title: "Anthropic API".to_string(),
        model_name: "claude-3-7-sonnet-20250219".to_string(),
        description: "Claude 3.7 Sonnet · Hybrid reasoning & thinking tokens".to_string(),
        base_url: None,
        format: "anthropic".to_string(),
        is_category_header: false,
    });
    options.push(ModelOption {
        provider_key: "gemini".to_string(),
        provider_title: "Google Gemini API".to_string(),
        model_name: "gemini-2.5-flash".to_string(),
        description: "Gemini 2.5 Flash · 1M token context window".to_string(),
        base_url: None,
        format: "gemini".to_string(),
        is_category_header: false,
    });
    options.push(ModelOption {
        provider_key: "openai".to_string(),
        provider_title: "OpenAI API".to_string(),
        model_name: "gpt-4o".to_string(),
        description: "GPT-4o · Frontier multimodal flagship".to_string(),
        base_url: None,
        format: "openai".to_string(),
        is_category_header: false,
    });

    // 6. Any custom providers defined in ~/.agendex/config.toml
    let standard_keys = [
        "nim",
        "nvidia",
        "strata",
        "proxymateur",
        "ollama",
        "gemini",
        "claude",
        "openai",
    ];
    for (key, entry) in &cfg.providers {
        if !standard_keys.contains(&key.as_str()) {
            options.push(ModelOption {
                provider_key: key.clone(),
                provider_title: format!("Custom Provider: {}", key),
                model_name: entry
                    .model
                    .clone()
                    .unwrap_or_else(|| "custom-model".to_string()),
                description: format!(
                    "Custom endpoint: {}",
                    entry.base_url.as_deref().unwrap_or("<default>")
                ),
                base_url: entry.base_url.clone(),
                format: entry.format.clone().unwrap_or_else(|| "openai".to_string()),
                is_category_header: false,
            });
        }
    }

    options
}

/// Launches the interactive, fancy model & provider selector screen.
///
/// Returns `Some(ModelOption)` if a selection was confirmed with Enter, or `None` if cancelled with Esc/q.
pub fn launch_model_selector(
    active_provider: &str,
    active_model: &str,
) -> anyhow::Result<Option<ModelOption>> {
    let options = build_model_options();
    if options.is_empty() {
        return Ok(None);
    }

    // Determine initial selected index (matching active provider/model if possible)
    let mut selected_index = 0;
    for (i, opt) in options.iter().enumerate() {
        if !opt.is_category_header {
            if opt.provider_key.eq_ignore_ascii_case(active_provider)
                && (opt.model_name.eq_ignore_ascii_case(active_model) || selected_index == 0)
            {
                selected_index = i;
                break;
            }
            if selected_index == 0 {
                selected_index = i;
            }
        }
    }

    // Enter raw mode and alternate screen
    enable_raw_mode()?;
    let _guard = RawModeGuard;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, Hide)?;

    let mut scroll_offset: usize = 0;
    const VISIBLE_ROWS: usize = 14;

    loop {
        // Adjust scroll offset so selected_index is always visible
        if selected_index < scroll_offset {
            scroll_offset = selected_index;
        } else if selected_index >= scroll_offset + VISIBLE_ROWS {
            scroll_offset = selected_index - VISIBLE_ROWS + 1;
        }

        render_screen(
            &mut out,
            &options,
            selected_index,
            scroll_offset,
            VISIBLE_ROWS,
            active_provider,
            active_model,
        )?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    // Navigate to previous non-header option
                    let mut prev = selected_index;
                    while prev > 0 {
                        prev -= 1;
                        if !options[prev].is_category_header {
                            selected_index = prev;
                            break;
                        }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    // Navigate to next non-header option
                    let mut next = selected_index;
                    while next + 1 < options.len() {
                        next += 1;
                        if !options[next].is_category_header {
                            selected_index = next;
                            break;
                        }
                    }
                }
                KeyCode::Home => {
                    if let Some(first) = options.iter().position(|o| !o.is_category_header) {
                        selected_index = first;
                    }
                }
                KeyCode::End => {
                    if let Some(last) = options.iter().rposition(|o| !o.is_category_header) {
                        selected_index = last;
                    }
                }
                KeyCode::Enter => {
                    if !options[selected_index].is_category_header {
                        return Ok(Some(options[selected_index].clone()));
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    return Ok(None);
                }
                _ => {}
            }
        }
    }
}

fn render_screen<W: Write>(
    w: &mut W,
    options: &[ModelOption],
    selected_idx: usize,
    scroll_offset: usize,
    visible_rows: usize,
    active_provider: &str,
    active_model: &str,
) -> anyhow::Result<()> {
    execute!(w, MoveTo(0, 0), Clear(ClearType::All))?;

    // 1. Header Banner
    execute!(
        w,
        SetForegroundColor(Color::Rgb {
            r: 129,
            g: 140,
            b: 248
        }),
        Print("  ✦ Agendex Model & Provider Selector\n"),
        SetForegroundColor(Color::Rgb {
            r: 100,
            g: 116,
            b: 139
        }),
        Print("  Use ↑/↓ or j/k to navigate · Enter to select · Esc to return\n"),
        Print("  ──────────────────────────────────────────────────────────────────────────\n"),
        ResetColor
    )?;

    // 2. Options List
    let end_idx = (scroll_offset + visible_rows).min(options.len());
    for (i, opt) in options.iter().enumerate().take(end_idx).skip(scroll_offset) {
        if opt.is_category_header {
            execute!(
                w,
                SetForegroundColor(Color::Rgb {
                    r: 100,
                    g: 116,
                    b: 139
                }),
                Print(format!("\n  ── {} ──\n", opt.provider_title)),
                ResetColor
            )?;
        } else {
            let is_selected = i == selected_idx;
            let is_active = opt.provider_key.eq_ignore_ascii_case(active_provider)
                && opt.model_name.eq_ignore_ascii_case(active_model);

            let cursor = if is_selected { "  ❯ " } else { "    " };
            let active_badge = if is_active { " [Active]" } else { "" };

            if is_selected {
                execute!(
                    w,
                    SetBackgroundColor(Color::Rgb {
                        r: 30,
                        g: 41,
                        b: 59
                    }),
                    SetForegroundColor(Color::Rgb {
                        r: 56,
                        g: 189,
                        b: 248
                    }),
                    Print(cursor),
                    SetForegroundColor(Color::Rgb {
                        r: 241,
                        g: 245,
                        b: 249
                    }),
                    Print(format!("{:<38}", opt.model_name)),
                    SetForegroundColor(Color::Rgb {
                        r: 52,
                        g: 211,
                        b: 153
                    }),
                    Print(format!("{:<10}", active_badge)),
                    SetForegroundColor(Color::Rgb {
                        r: 148,
                        g: 163,
                        b: 184
                    }),
                    Print(format!(" {:<20}", opt.provider_title)),
                    ResetColor,
                    Print("\n")
                )?;
            } else {
                execute!(
                    w,
                    SetForegroundColor(Color::Rgb {
                        r: 100,
                        g: 116,
                        b: 139
                    }),
                    Print(cursor),
                    SetForegroundColor(Color::Rgb {
                        r: 226,
                        g: 232,
                        b: 240
                    }),
                    Print(format!("{:<38}", opt.model_name)),
                    SetForegroundColor(Color::Rgb {
                        r: 52,
                        g: 211,
                        b: 153
                    }),
                    Print(format!("{:<10}", active_badge)),
                    SetForegroundColor(Color::Rgb {
                        r: 100,
                        g: 116,
                        b: 139
                    }),
                    Print(format!(" {:<20}", opt.provider_title)),
                    ResetColor,
                    Print("\n")
                )?;
            }
        }
    }

    // 3. Details Card Footer
    let selected_opt = &options[selected_idx];
    let url_display = selected_opt
        .base_url
        .as_deref()
        .unwrap_or("<upstream cloud api>");

    execute!(
        w,
        SetForegroundColor(Color::Rgb {
            r: 100,
            g: 116,
            b: 139
        }),
        Print("\n  ──────────────────────────────────────────────────────────────────────────\n"),
        Print("  ╭─ Model Details ─────────────────────────────────────────────────────────╮\n"),
        Print(format!(
            "  │  Description: {:<57} │\n",
            if selected_opt.description.len() > 57 {
                &selected_opt.description[..57]
            } else {
                &selected_opt.description
            }
        )),
        Print(format!(
            "  │  Provider:    {:<20} Format: {:<8} Ceiling: 128k context  │\n",
            selected_opt.provider_title, selected_opt.format
        )),
        Print(format!(
            "  │  Endpoint:    {:<57} │\n",
            if url_display.len() > 57 {
                &url_display[..57]
            } else {
                url_display
            }
        )),
        Print("  ╰─────────────────────────────────────────────────────────────────────────╯\n"),
        ResetColor
    )?;

    w.flush()?;
    Ok(())
}
