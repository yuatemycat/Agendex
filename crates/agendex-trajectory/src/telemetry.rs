use serde::{Deserialize, Serialize};

/// Token consumption breakdown for a single turn or model interaction.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    pub estimated_cost: f64,
}

impl TokenUsage {
    pub fn new(prompt_tokens: usize, completion_tokens: usize, estimated_cost: f64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            estimated_cost,
        }
    }
}

/// Cumulative token consumption and cost for an active agent session.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SessionUsage {
    pub total_prompt_tokens: usize,
    pub total_completion_tokens: usize,
    pub total_tokens: usize,
    pub total_cost: f64,
    pub turn_count: usize,
}

impl SessionUsage {
    pub fn record_turn(&mut self, turn: &TokenUsage) {
        self.total_prompt_tokens += turn.prompt_tokens;
        self.total_completion_tokens += turn.completion_tokens;
        self.total_tokens += turn.total_tokens;
        self.total_cost += turn.estimated_cost;
        self.turn_count += 1;
    }
}

/// Estimates dollar cost based on model name and token volume.
/// Unknown models (e.g. self-hosted NIM, Ollama, local models) default to $0.00.
pub fn estimate_cost(model_name: &str, prompt_tokens: usize, completion_tokens: usize) -> f64 {
    let lower = model_name.to_lowercase();
    let (prompt_rate, completion_rate) = match lower.as_str() {
        m if m.contains("gemini-2.5-flash")
            || m.contains("gemini-2.0-flash")
            || m.contains("gemini-1.5-flash") =>
        {
            // $0.15 / 1M prompt, $0.60 / 1M completion
            (0.00000015, 0.00000060)
        }
        m if m.contains("gemini-2.5-pro")
            || m.contains("gemini-1.5-pro")
            || m.contains("gemini-pro") =>
        {
            // $1.25 / 1M prompt, $5.00 / 1M completion
            (0.00000125, 0.00000500)
        }
        m if m.contains("gpt-4o-mini") => {
            // $0.15 / 1M prompt, $0.60 / 1M completion
            (0.00000015, 0.00000060)
        }
        m if m.contains("gpt-4o") || m.contains("gpt-4-turbo") || m.contains("gpt-4") => {
            // $2.50 / 1M prompt, $10.00 / 1M completion
            (0.00000250, 0.00001000)
        }
        m if m.contains("claude-3-5-sonnet")
            || m.contains("claude-3-7-sonnet")
            || m.contains("claude-3-sonnet") =>
        {
            // $3.00 / 1M prompt, $15.00 / 1M completion
            (0.00000300, 0.00001500)
        }
        m if m.contains("claude-3-5-haiku") || m.contains("claude-3-haiku") => {
            // $0.80 / 1M prompt, $4.00 / 1M completion
            (0.00000080, 0.00000400)
        }
        m if m.contains("claude-3-opus") => {
            // $15.00 / 1M prompt, $75.00 / 1M completion
            (0.00001500, 0.00007500)
        }
        _ => {
            // Unknown or self-hosted / local / NIM models default to $0.00 (not arbitrarily billed as GPT-4o)
            (0.0, 0.0)
        }
    };

    (prompt_tokens as f64 * prompt_rate) + (completion_tokens as f64 * completion_rate)
}

/// Formats an integer with standard thousands comma separators (e.g. 1230 -> "1,230").
pub fn format_comma(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let len = s.len();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

/// Formats turn and cumulative session token usage into the standard telemetry string:
/// e.g. "[Tokens: 412 in, 85 out | Session: 1,230 ($0.0018)]"
pub fn format_telemetry(turn_usage: &TokenUsage, session_usage: &SessionUsage) -> String {
    format!(
        "[Tokens: {} in, {} out | Session: {} (${:.4})]",
        format_comma(turn_usage.prompt_tokens),
        format_comma(turn_usage.completion_tokens),
        format_comma(session_usage.total_tokens),
        session_usage.total_cost
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_comma() {
        assert_eq!(format_comma(0), "0");
        assert_eq!(format_comma(412), "412");
        assert_eq!(format_comma(1230), "1,230");
        assert_eq!(format_comma(1234567), "1,234,567");
    }

    #[test]
    fn test_unknown_models_default_to_zero_cost() {
        let cost = estimate_cost("meta/llama-3.1-70b-instruct", 1000, 500);
        assert_eq!(cost, 0.0);
        let local_cost = estimate_cost("ollama/mistral", 2000, 1000);
        assert_eq!(local_cost, 0.0);
    }

    #[test]
    fn test_estimate_cost_and_format_telemetry() {
        let prompt_tokens = 412;
        let completion_tokens = 85;
        let cost = estimate_cost("gpt-4o", prompt_tokens, completion_tokens);
        // 412 * 0.0000025 + 85 * 0.000010 = 0.00103 + 0.00085 = 0.00188
        assert!((cost - 0.00188).abs() < 1e-6);

        let turn = TokenUsage::new(prompt_tokens, completion_tokens, cost);
        let session = SessionUsage {
            total_tokens: 1230,
            total_cost: cost,
            turn_count: 1,
            ..Default::default()
        };

        let display = format_telemetry(&turn, &session);
        assert_eq!(
            display,
            "[Tokens: 412 in, 85 out | Session: 1,230 ($0.0019)]"
        );
    }
}
