use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// Masks known API keys, tokens, credentials, and .env secrets from string content.
pub fn mask_secrets(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let mut result = input.to_string();

    // 1. Specific API key patterns (OpenAI, Anthropic, Nvidia NIM, Google, GitHub, AWS, Slack, Bearer)
    static KEY_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    let key_regexes = KEY_REGEXES.get_or_init(|| {
        vec![
            // Anthropic keys
            Regex::new(r"(?i)\b(sk-ant-[a-zA-Z0-9_\-]{8,})\b").unwrap(),
            // OpenAI & general sk- keys
            Regex::new(r"(?i)\b(sk-[a-zA-Z0-9_\-]{12,})\b").unwrap(),
            // Nvidia NIM keys
            Regex::new(r"(?i)\b(nvapi-[a-zA-Z0-9_\-]{8,})\b").unwrap(),
            // Google AIza keys
            Regex::new(r"\b(AIza[0-9A-Za-z\-_]{35})\b").unwrap(),
            // GitHub tokens
            Regex::new(r"\b(gh[pousr]_[A-Za-z0-9_]{16,})\b").unwrap(),
            Regex::new(r"\b(github_pat_[A-Za-z0-9_]{20,})\b").unwrap(),
            // AWS Access Keys
            Regex::new(r"\b(AKIA[0-9A-Z]{16})\b").unwrap(),
            // Slack tokens
            Regex::new(r"\b(xox[baprs]-[0-9a-zA-Z\-]{10,})\b").unwrap(),
        ]
    });

    for re in key_regexes {
        result = re.replace_all(&result, "[REDACTED]").to_string();
    }

    // Bearer token patterns
    static BEARER_REGEX: OnceLock<Regex> = OnceLock::new();
    let bearer_re = BEARER_REGEX
        .get_or_init(|| Regex::new(r"(?i)\b(bearer\s+)([a-zA-Z0-9_\-\.]{16,})\b").unwrap());
    result = bearer_re.replace_all(&result, "${1}[REDACTED]").to_string();

    // 2. .env and key-value pair secret assignments (e.g., API_KEY=..., SECRET: ...)
    static ENV_REGEX: OnceLock<Regex> = OnceLock::new();
    let env_re = ENV_REGEX.get_or_init(|| {
        Regex::new(
            r#"(?im)^(\s*(?:export\s+)?(?:[A-Z0-9_]*(?:KEY|SECRET|TOKEN|PASSWORD|PASSWD|AUTH|CREDENTIAL|PRIVATE)[A-Z0-9_]*)\s*[:=]\s*["']?)([^"'\r\n\s]{4,})(["']?\s*)$"#,
        )
        .unwrap()
    });

    result = env_re
        .replace_all(&result, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let val = &caps[2];
            let suffix = &caps[3];
            // Preserve environment variable placeholders like $OPENAI_KEY or ${VAR}
            if val.starts_with('$')
                || val.eq_ignore_ascii_case("true")
                || val.eq_ignore_ascii_case("false")
                || val.eq_ignore_ascii_case("none")
                || val.eq_ignore_ascii_case("null")
            {
                format!("{}{}{}", prefix, val, suffix)
            } else {
                format!("{}[REDACTED]{}", prefix, suffix)
            }
        })
        .to_string();

    // 3. JSON secret fields (e.g., "api_key": "...")
    static JSON_FIELD_REGEX: OnceLock<Regex> = OnceLock::new();
    let json_re = JSON_FIELD_REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)"((?:api_?)?key|secret|token|password|passwd|auth|credential)"\s*:\s*"([^"]{4,})""#,
        )
        .unwrap()
    });

    result = json_re
        .replace_all(&result, |caps: &regex::Captures| {
            let field = &caps[1];
            let val = &caps[2];
            if val.starts_with('$') || val.eq_ignore_ascii_case("[REDACTED]") {
                format!(r#""{}": "{}""#, field, val)
            } else {
                format!(r#""{}": "[REDACTED]""#, field)
            }
        })
        .to_string();

    result
}

/// Recursively traverses and redacts secrets from a serde_json Value.
pub fn redact_json_value(val: &Value) -> Value {
    match val {
        Value::String(s) => Value::String(mask_secrets(s)),
        Value::Array(arr) => Value::Array(arr.iter().map(redact_json_value).collect()),
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            for (k, v) in map {
                let k_lower = k.to_lowercase();
                if k_lower.contains("key")
                    || k_lower.contains("secret")
                    || k_lower.contains("token")
                    || k_lower.contains("password")
                    || k_lower.contains("auth")
                {
                    if let Value::String(s) = v {
                        if s.starts_with('$') {
                            new_map.insert(k.clone(), Value::String(s.clone()));
                        } else {
                            new_map.insert(k.clone(), Value::String("[REDACTED]".to_string()));
                        }
                        continue;
                    }
                }
                new_map.insert(k.clone(), redact_json_value(v));
            }
            Value::Object(new_map)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_openai_and_anthropic_and_nim_keys() {
        let text = "Here are keys: sk-abc1234567890abcdef and sk-ant-api03-abcdef123456 and nvapi-9988776655443322";
        let masked = mask_secrets(text);
        assert!(!masked.contains("sk-abc1234567890abcdef"));
        assert!(!masked.contains("sk-ant-api03-abcdef123456"));
        assert!(!masked.contains("nvapi-9988776655443322"));
        assert_eq!(
            masked,
            "Here are keys: [REDACTED] and [REDACTED] and [REDACTED]"
        );
    }

    #[test]
    fn test_mask_env_file_assignments() {
        let env_content = r#"
# Database config
DB_HOST=127.0.0.1
DB_PORT=5432
DB_PASSWORD=my_super_secret_db_pass_123

# API keys
OPENAI_API_KEY=sk-live-secret-openai-key-999
NVIDIA_API_KEY=nvapi-production-token-888
SAFE_REF=$ALREADY_AN_ENV_VAR
EMPTY_VAL=
"#;
        let masked = mask_secrets(env_content);
        assert!(!masked.contains("my_super_secret_db_pass_123"));
        assert!(!masked.contains("sk-live-secret-openai-key-999"));
        assert!(!masked.contains("nvapi-production-token-888"));
        assert!(masked.contains("DB_PASSWORD=[REDACTED]"));
        assert!(masked.contains("OPENAI_API_KEY=[REDACTED]"));
        assert!(masked.contains("NVIDIA_API_KEY=[REDACTED]"));
        assert!(masked.contains("SAFE_REF=$ALREADY_AN_ENV_VAR"));
        assert!(masked.contains("DB_HOST=127.0.0.1"));
    }

    #[test]
    fn test_redact_json_value() {
        let val = serde_json::json!({
            "model": "gpt-4o",
            "api_key": "sk-12345678901234567890",
            "nested": {
                "secret_token": "my-secret-val",
                "env_ref": "$SOME_REF",
                "normal": "hello sk-12345678901234567890 world"
            }
        });

        let redacted = redact_json_value(&val);
        assert_eq!(redacted["api_key"], "[REDACTED]");
        assert_eq!(redacted["nested"]["secret_token"], "[REDACTED]");
        assert_eq!(redacted["nested"]["env_ref"], "$SOME_REF");
        assert_eq!(redacted["nested"]["normal"], "hello [REDACTED] world");
    }
}
