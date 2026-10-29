use crossterm::{
    cursor::{Hide, MoveUp, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use std::io::{stdout, Write};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ModelOption {
    pub provider_key: String,
    pub provider_tag: String,
    pub model_name: String,
    pub description: String,
    pub base_url: Option<String>,
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut out = stdout();
        let _ = execute!(out, Show);
        let _ = out.flush();
    }
}

/// Builds the curated, compact list of model options.
pub fn build_model_options() -> Vec<ModelOption> {
    let cfg = agendex_core::config::load_default_config()
        .unwrap_or_else(agendex_core::config::create_default_config_with_presets);

    let mut options = vec![
        ModelOption {
            provider_key: "nim".to_string(),
            provider_tag: "Nvidia NIM".to_string(),
            model_name: "meta/llama-3.1-70b-instruct".to_string(),
            description: "Llama 3.1 70B · Fast reasoning & tool calling".to_string(),
            base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
        },
        ModelOption {
            provider_key: "nim".to_string(),
            provider_tag: "Nvidia NIM".to_string(),
            model_name: "mistralai/mistral-large-2-instruct".to_string(),
            description: "Mistral Large 2 · 128k context window".to_string(),
            base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
        },
        ModelOption {
            provider_key: "strata".to_string(),
            provider_tag: "Strata Local MoE".to_string(),
            model_name: "huihui-swift-1.5".to_string(),
            description: "125B MoE Local Engine · Zero Cloud Egress".to_string(),
            base_url: Some("http://127.0.0.1:8080/v1".to_string()),
        },
        ModelOption {
            provider_key: "proxymateur".to_string(),
            provider_tag: "Proxymateur Gateway".to_string(),
            model_name: "gemini-2.5-flash".to_string(),
            description: "Gemini 2.5 Flash via Local Gateway (8045)".to_string(),
            base_url: Some("http://127.0.0.1:8045/v1".to_string()),
        },
        ModelOption {
            provider_key: "ollama".to_string(),
            provider_tag: "Ollama Local".to_string(),
            model_name: "qwen2.5-coder:7b".to_string(),
            description: "Qwen 2.5 Coder 7B · Local Coding Daemon".to_string(),
            base_url: Some("http://localhost:11434/v1".to_string()),
        },
        ModelOption {
            provider_key: "claude".to_string(),
            provider_tag: "Anthropic API".to_string(),
            model_name: "claude-3-7-sonnet-20250219".to_string(),
            description: "Claude 3.7 Sonnet · Hybrid reasoning".to_string(),
            base_url: None,
        },
        ModelOption {
            provider_key: "gemini".to_string(),
            provider_tag: "Google Gemini API".to_string(),
            model_name: "gemini-2.5-flash".to_string(),
            description: "Gemini 2.5 Flash · 1M token context".to_string(),
            base_url: None,
        },
        ModelOption {
            provider_key: "openai".to_string(),
            provider_tag: "OpenAI API".to_string(),
            model_name: "gpt-4o".to_string(),
            description: "GPT-4o · Frontier multimodal flagship".to_string(),
            base_url: None,
        },
    ];

    // Append custom providers if any
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
                provider_tag: format!("Custom: {}", key),
                model_name: entry
                    .model
                    .clone()
                    .unwrap_or_else(|| "custom-model".to_string()),
                description: format!(
                    "Endpoint: {}",
                    entry.base_url.as_deref().unwrap_or("<default>")
                ),
                base_url: entry.base_url.clone(),
            });
        }
    }

    options
}

/// Launches a compact, inline model selector directly beneath the prompt without clearing the screen.
///
/// Navigated using ↑/↓ or j/k; confirmed with Enter; cancelled with Esc or q.
pub fn launch_model_selector(
    active_provider: &str,
    active_model: &str,
) -> anyhow::Result<Option<ModelOption>> {
    let options = build_model_options();
    if options.is_empty() {
        return Ok(None);
    }

    // Default to the currently active provider/model if matched
    let mut selected_idx = 0;
    for (i, opt) in options.iter().enumerate() {
        if opt.provider_key.eq_ignore_ascii_case(active_provider)
            && opt.model_name.eq_ignore_ascii_case(active_model)
        {
            selected_idx = i;
            break;
        }
    }

    enable_raw_mode()?;
    let _guard = RawModeGuard;
    let mut out = stdout();
    execute!(out, Hide)?;

    let total_lines = options.len() + 1;

    // Render initial inline frame
    render_inline(
        &mut out,
        &options,
        selected_idx,
        active_provider,
        active_model,
    )?;

    loop {
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }

                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        if selected_idx > 0 {
                            selected_idx -= 1;
                        } else {
                            selected_idx = options.len().saturating_sub(1);
                        }
                        clear_inline(&mut out, total_lines)?;
                        render_inline(
                            &mut out,
                            &options,
                            selected_idx,
                            active_provider,
                            active_model,
                        )?;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if selected_idx + 1 < options.len() {
                            selected_idx += 1;
                        } else {
                            selected_idx = 0;
                        }
                        clear_inline(&mut out, total_lines)?;
                        render_inline(
                            &mut out,
                            &options,
                            selected_idx,
                            active_provider,
                            active_model,
                        )?;
                    }
                    KeyCode::Home => {
                        selected_idx = 0;
                        clear_inline(&mut out, total_lines)?;
                        render_inline(
                            &mut out,
                            &options,
                            selected_idx,
                            active_provider,
                            active_model,
                        )?;
                    }
                    KeyCode::End => {
                        selected_idx = options.len().saturating_sub(1);
                        clear_inline(&mut out, total_lines)?;
                        render_inline(
                            &mut out,
                            &options,
                            selected_idx,
                            active_provider,
                            active_model,
                        )?;
                    }
                    KeyCode::Enter => {
                        clear_inline(&mut out, total_lines)?;
                        execute!(out, Show)?;
                        let _ = disable_raw_mode();
                        return Ok(Some(options[selected_idx].clone()));
                    }
                    KeyCode::Esc | KeyCode::Char('q') => {
                        clear_inline(&mut out, total_lines)?;
                        execute!(out, Show)?;
                        let _ = disable_raw_mode();
                        return Ok(None);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn render_inline<W: Write>(
    w: &mut W,
    options: &[ModelOption],
    selected_idx: usize,
    active_provider: &str,
    active_model: &str,
) -> anyhow::Result<()> {
    // Header line
    execute!(
        w,
        SetForegroundColor(Color::Rgb {
            r: 56,
            g: 189,
            b: 248
        }),
        Print("  ? "),
        SetForegroundColor(Color::Rgb {
            r: 241,
            g: 245,
            b: 249
        }),
        Print("Select model"),
        SetForegroundColor(Color::Rgb {
            r: 100,
            g: 116,
            b: 139
        }),
        Print(" (↑/↓ arrows, Enter to confirm, Esc to cancel):\r\n"),
        ResetColor
    )?;

    // Option rows
    for (i, opt) in options.iter().enumerate() {
        let is_selected = i == selected_idx;
        let is_active = opt.provider_key.eq_ignore_ascii_case(active_provider)
            && opt.model_name.eq_ignore_ascii_case(active_model);

        if is_selected {
            execute!(
                w,
                SetForegroundColor(Color::Rgb {
                    r: 56,
                    g: 189,
                    b: 248
                }),
                Print("  ❯ "),
                SetForegroundColor(Color::Rgb {
                    r: 255,
                    g: 255,
                    b: 255
                }),
                Print(&opt.model_name),
                SetForegroundColor(Color::Rgb {
                    r: 148,
                    g: 163,
                    b: 184
                }),
                Print(format!(" ({})", opt.provider_tag)),
                ResetColor
            )?;
            if is_active {
                execute!(
                    w,
                    SetForegroundColor(Color::Rgb {
                        r: 52,
                        g: 211,
                        b: 153
                    }),
                    Print(" [Active]"),
                    ResetColor
                )?;
            }
            execute!(w, Print("\r\n"))?;
        } else {
            execute!(
                w,
                Print("    "),
                SetForegroundColor(Color::Rgb {
                    r: 203,
                    g: 213,
                    b: 225
                }),
                Print(&opt.model_name),
                SetForegroundColor(Color::Rgb {
                    r: 100,
                    g: 116,
                    b: 139
                }),
                Print(format!(" ({})", opt.provider_tag)),
                ResetColor
            )?;
            if is_active {
                execute!(
                    w,
                    SetForegroundColor(Color::Rgb {
                        r: 52,
                        g: 211,
                        b: 153
                    }),
                    Print(" [Active]"),
                    ResetColor
                )?;
            }
            execute!(w, Print("\r\n"))?;
        }
    }

    w.flush()?;
    Ok(())
}

fn clear_inline<W: Write>(w: &mut W, line_count: usize) -> anyhow::Result<()> {
    execute!(
        w,
        crossterm::cursor::MoveToColumn(0),
        MoveUp(line_count as u16),
        Clear(ClearType::FromCursorDown)
    )?;
    w.flush()?;
    Ok(())
}
