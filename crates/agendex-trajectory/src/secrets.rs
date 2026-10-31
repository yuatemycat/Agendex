use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

fn is_safe_assignment(key: &str, val: &str) -> bool {
    let k_lower = key.to_lowercase();
    // Exclude token metrics and parameters
    if k_lower.contains("token")
        && (k_lower.contains("max_")
            || k_lower.contains("prompt_")
            || k_lower.contains("completion_")
            || k_lower.contains("total_")
            || k_lower.contains("num_")
            || k_lower == "tokens"
            || k_lower == "tokenizer")
    {
        return true;
    }
    if k_lower == "author"
        || k_lower == "authors"
        || k_lower == "oauth_url"
        || k_lower == "auth_url"
        || k_lower == "authorize_url"
        || k_lower == "temperature"
        || k_lower == "top_p"
        || k_lower == "top_k"
    {
        return true;
    }

    // Exclude standard non-secret environment variables
    if k_lower == "port"
        || k_lower == "host"
        || k_lower == "db_host"
        || k_lower == "db_port"
        || k_lower == "node_env"
        || k_lower == "env"
        || k_lower == "stage"
        || k_lower == "debug"
        || k_lower == "app_name"
        || k_lower == "log_level"
    {
        return true;
    }

    let trimmed_val = val.trim_matches(|c| c == '"' || c == '\'');
    if trimmed_val.starts_with('$')
        || trimmed_val == "[REDACTED]"
        || trimmed_val.eq_ignore_ascii_case("true")
        || trimmed_val.eq_ignore_ascii_case("false")
        || trimmed_val.eq_ignore_ascii_case("null")
        || trimmed_val.eq_ignore_ascii_case("none")
    {
        return true;
    }

    // Exclude pure numbers (e.g. 4096, 5432, 8080, 0, 1)
    if trimmed_val.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return true;
    }

    // Exclude IP addresses (e.g. 127.0.0.1, 0.0.0.0)
    if trimmed_val == "127.0.0.1" || trimmed_val == "localhost" || trimmed_val == "0.0.0.0" {
        return true;
    }

    // Exclude programming language type definitions (e.g. `String`, `Option<...>`, `Vec<...>`, `&str`, `str`, `usize`)
    let types = [
        "String", "str", "&str", "Option", "Vec", "Arc", "Box", "bool", "usize", "u32", "u64",
        "i32", "i64", "int", "float", "char", "Self", "Result", "PathBuf", "Value",
    ];
    if types.iter().any(|t| trimmed_val.starts_with(t)) {
        return true;
    }

    false
}

fn is_sensitive_value(val: &str) -> bool {
    let v_lower = val.to_lowercase();
    let trimmed = v_lower.trim_matches(|c| c == '"' || c == '\'');
    if trimmed.len() < 6 {
        return false;
    }
    trimmed.contains("secret") || trimmed.contains("password") || trimmed.contains("passwd")
}

fn is_sensitive_key_name(k: &str) -> bool {
    let k_lower = k.to_lowercase();
    // Exclusions
    if k_lower == "author"
        || k_lower == "authors"
        || k_lower == "tokenizer"
        || k_lower == "oauth_url"
        || k_lower == "auth_url"
        || k_lower == "authorize_url"
        || k_lower == "authentication_method"
        || k_lower == "key_bindings"
        || k_lower == "keybindings"
        || k_lower == "keycode"
        || k_lower == "tokens"
        || k_lower == "name"
        || k_lower == "version"
        || k_lower == "edition"
        || k_lower == "description"
        || k_lower == "title"
        || k_lower == "path"
        || k_lower == "file"
        || k_lower == "model"
        || k_lower == "url"
        || k_lower == "type"
        || k_lower == "layout"
        || k_lower.starts_with("keyboard")
        || k_lower.starts_with("keyword")
        || k_lower.contains("max_token")
        || k_lower.contains("prompt_token")
        || k_lower.contains("completion_token")
        || k_lower.contains("total_token")
        || k_lower.contains("num_token")
    {
        return false;
    }

    // Exact matches (note: generic "key" and "token" are intentionally excluded to avoid over-masking)
    let exact = [
        "api_key",
        "apikey",
        "secret",
        "secret_key",
        "client_secret",
        "access_token",
        "refresh_token",
        "auth_token",
        "session_token",
        "private_key",
        "password",
        "passwd",
        "db_password",
        "credential",
        "credentials",
    ];
    if exact.contains(&k_lower.as_str()) {
        return true;
    }

    // Suffix matches
    if k_lower.ends_with("_key")
        || k_lower.ends_with("_secret")
        || k_lower.ends_with("_token")
        || k_lower.ends_with("_password")
        || k_lower.ends_with("_passwd")
        || k_lower.ends_with("_credential")
        || k_lower.ends_with("_credentials")
    {
        return true;
    }

    // Prefix matches
    if k_lower.starts_with("api_key")
        || k_lower.starts_with("secret_")
        || k_lower.starts_with("token_")
        || k_lower.starts_with("password_")
        || k_lower.starts_with("private_key")
    {
        return true;
    }

    // Delimited matches
    if k_lower.contains("_key_")
        || k_lower.contains("_secret_")
        || k_lower.contains("_password_")
        || k_lower.contains("_token_")
    {
        return true;
    }

    false
}

fn is_secret_json_key(k: &str) -> bool {
    is_sensitive_key_name(k)
}

/// Masks known API keys, tokens, credentials, and .env secrets from string content.
pub fn mask_secrets(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let mut result = input.to_string();

    // 1. Specific API key patterns (OpenAI, Anthropic, Nvidia NIM, Google, GitHub, AWS, Slack, Bearer, JWT, PEM)
    // sk- keys (OpenAI & Anthropic & general, excluding package names like sk-learn or multi-hyphen tool names like sk-docs-generator-tool)
    static SK_REGEX: OnceLock<Regex> = OnceLock::new();
    let sk_re =
        SK_REGEX.get_or_init(|| Regex::new(r"(?i)\b(sk-(?:ant-)?[a-zA-Z0-9_\-]{8,})\b").unwrap());
    result = sk_re
        .replace_all(&result, |caps: &regex::Captures| {
            let tok = &caps[1];
            let lower = tok.to_lowercase();
            let after = if let Some(stripped) = lower.strip_prefix("sk-ant-") {
                stripped
            } else if let Some(stripped) = lower.strip_prefix("sk-") {
                stripped
            } else {
                &lower
            };

            // 1. Spared: well-known Python machine learning package prefixes
            if after.starts_with("learn")
                || after.starts_with("image")
                || after.starts_with("opt")
                || after.starts_with("spatial")
                || after.starts_with("build")
            {
                return tok.to_string();
            }

            // 2. Spared: multi-hyphen kebab-case tool/module identifiers with no digits and no uppercase (e.g. sk-docs-generator-tool)
            let has_digit = tok.chars().any(|c| c.is_ascii_digit());
            let has_uppercase = tok.chars().any(|c| c.is_ascii_uppercase());
            if !has_digit && !has_uppercase && after.contains('-') {
                return tok.to_string();
            }

            // Otherwise, bare random letters (e.g. sk-abcdefghij) or tokens with digits/mixed case are masked
            "[REDACTED]".to_string()
        })
        .to_string();

    static KEY_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    let key_regexes = KEY_REGEXES.get_or_init(|| {
        vec![
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
            // JWT tokens
            Regex::new(r"\beyJ[A-Za-z0-9-_=]{10,}\.[A-Za-z0-9-_=]{10,}\.[A-Za-z0-9-_=]{10,}\b")
                .unwrap(),
        ]
    });

    for re in key_regexes {
        result = re.replace_all(&result, "[REDACTED]").to_string();
    }

    // PEM Private Key blocks
    static PEM_REGEX: OnceLock<Regex> = OnceLock::new();
    let pem_re = PEM_REGEX.get_or_init(|| {
        Regex::new(
            r"-----BEGIN (?:[A-Z0-9_-]+ )?PRIVATE KEY-----[\s\S]*?-----END (?:[A-Z0-9_-]+ )?PRIVATE KEY-----",
        )
        .unwrap()
    });
    result = pem_re.replace_all(&result, "[REDACTED]").to_string();

    // Bearer token patterns
    static BEARER_REGEX: OnceLock<Regex> = OnceLock::new();
    let bearer_re = BEARER_REGEX
        .get_or_init(|| Regex::new(r"(?i)\b(bearer\s+)([a-zA-Z0-9_\-\.]{16,})\b").unwrap());
    result = bearer_re.replace_all(&result, "${1}[REDACTED]").to_string();

    // URI credentials (e.g. postgres://user:password@host:port/db)
    static URI_PW_REGEX: OnceLock<Regex> = OnceLock::new();
    let uri_pw_re = URI_PW_REGEX
        .get_or_init(|| Regex::new(r"(?i)\b([a-z0-9+]+://[^:\s@]+:)([^@\s]+)(@[^/\s]+)").unwrap());
    result = uri_pw_re
        .replace_all(&result, "${1}[REDACTED]${3}")
        .to_string();

    // 2. Unanchored inline secret key-value assignments (e.g. DB_PASSWORD=xxx, API_KEY=xxx)
    // Preserves whitespace and surrounding quotes verbatim to avoid reformatting display/logs
    static INLINE_ASSIGN_REGEX: OnceLock<Regex> = OnceLock::new();
    let inline_re = INLINE_ASSIGN_REGEX.get_or_init(|| {
        Regex::new(r#"(\b[A-Za-z0-9_]+\s*=\s*["']?)([^"'\s;&|\r\n]+)(["']?)"#).unwrap()
    });
    result = inline_re
        .replace_all(&result, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let val = &caps[2];
            let suffix = &caps[3];
            let key = prefix
                .trim_start()
                .trim_end_matches(|c| c == '=' || c == ' ' || c == '\t' || c == '"' || c == '\'')
                .trim();
            if (is_sensitive_key_name(key) || is_sensitive_value(val))
                && !is_safe_assignment(key, val)
            {
                format!("{}[REDACTED]{}", prefix, suffix)
            } else {
                caps[0].to_string()
            }
        })
        .to_string();

    // 3. .env file line assignments (e.g. MY_CUSTOM=abc123secretvalue, DB_PASSWORD=...)
    static ENV_REGEX: OnceLock<Regex> = OnceLock::new();
    let env_re = ENV_REGEX.get_or_init(|| {
        Regex::new(
            r#"(?m)^(\s*(?:export\s+)?([A-Za-z0-9_]{2,})\s*=\s*["']?)([^"'\r\n\s#]+)(["']?.*)$"#,
        )
        .unwrap()
    });

    result = env_re
        .replace_all(&result, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let key = &caps[2];
            let val = &caps[3];
            let suffix = &caps[4];
            if (is_sensitive_key_name(key) || is_sensitive_value(val))
                && !is_safe_assignment(key, val)
            {
                format!("{}[REDACTED]{}", prefix, suffix)
            } else {
                caps[0].to_string()
            }
        })
        .to_string();

    // 4. YAML colon assignments (e.g. password: hunter2pass, api_key: "xxx")
    static YAML_FIELD_REGEX: OnceLock<Regex> = OnceLock::new();
    let yaml_re = YAML_FIELD_REGEX.get_or_init(|| {
        Regex::new(r#"(?m)^(\s*["']?([A-Za-z0-9_\-]+)["']?\s*:\s*["']?)([^"'\r\n\s#]+)(["']?\s*)$"#)
            .unwrap()
    });

    result = yaml_re
        .replace_all(&result, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let key = &caps[2];
            let val = &caps[3];
            let suffix = &caps[4];
            if (is_sensitive_key_name(key) || is_sensitive_value(val))
                && !is_safe_assignment(key, val)
            {
                format!("{}[REDACTED]{}", prefix, suffix)
            } else {
                caps[0].to_string()
            }
        })
        .to_string();

    // 5. JSON secret fields (e.g., "api_key": "...")
    static JSON_FIELD_REGEX: OnceLock<Regex> = OnceLock::new();
    let json_re = JSON_FIELD_REGEX
        .get_or_init(|| Regex::new(r#"(?i)("([A-Za-z0-9_\-]+)"\s*:\s*")([^"]+)(")"#).unwrap());

    result = json_re
        .replace_all(&result, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let field = &caps[2];
            let val = &caps[3];
            let suffix = &caps[4];
            if (is_sensitive_key_name(field) || is_sensitive_value(val))
                && !is_safe_assignment(field, val)
            {
                format!("{}[REDACTED]{}", prefix, suffix)
            } else {
                caps[0].to_string()
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
                if is_secret_json_key(k) {
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

/// A streaming buffer that masks sensitive patterns before emitting to the terminal.
/// Buffers partial chunks until complete lines or safe token boundaries are reached.
#[derive(Default, Debug, Clone)]
pub struct StreamingSecretFilter {
    buffer: String,
}

impl StreamingSecretFilter {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Appends a new chunk and returns any masked text that is safe to output immediately.
    pub fn push(&mut self, chunk: &str) -> String {
        self.buffer.push_str(chunk);

        // If buffer contains newlines, mask and emit all completed lines
        if let Some(last_newline_idx) = self.buffer.rfind('\n') {
            let completed = &self.buffer[..=last_newline_idx];
            let masked = mask_secrets(completed);
            self.buffer = self.buffer[last_newline_idx + 1..].to_string();
            return masked;
        }

        // If buffer has spaces and is getting long (> 64 chars) with no secret-like prefix
        if self.buffer.len() > 64 {
            if let Some(last_space_idx) = self.buffer.rfind(' ') {
                let segment = &self.buffer[..=last_space_idx];
                let masked = mask_secrets(segment);
                self.buffer = self.buffer[last_space_idx + 1..].to_string();
                return masked;
            }
        }

        String::new()
    }

    /// Flushes any remaining buffered text, applying secret masking.
    pub fn flush(&mut self) -> String {
        if self.buffer.is_empty() {
            String::new()
        } else {
            let masked = mask_secrets(&self.buffer);
            self.buffer.clear();
            masked
        }
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

    #[test]
    fn test_secrets_overmasking_prevention() {
        // Normal Rust code lines and config parameters must NOT be masked
        let rust_code = "max_tokens: 4096,\napi_key: String,\ntotal_tokens: 1200,\n";
        let masked_code = mask_secrets(rust_code);
        assert!(masked_code.contains("max_tokens: 4096"));
        assert!(masked_code.contains("api_key: String"));
        assert!(masked_code.contains("total_tokens: 1200"));

        // JSON fields author, tokenizer, oauth_url must NOT be redacted
        let json_data = serde_json::json!({
            "author": "Alice Developer",
            "tokenizer": "cl100k_base",
            "oauth_url": "https://auth.example.com/oauth/token",
            "max_tokens": 4096,
            "api_key": "sk-12345678901234567890"
        });
        let redacted = redact_json_value(&json_data);
        assert_eq!(redacted["author"], "Alice Developer");
        assert_eq!(redacted["tokenizer"], "cl100k_base");
        assert_eq!(
            redacted["oauth_url"],
            "https://auth.example.com/oauth/token"
        );
        assert_eq!(redacted["max_tokens"], 4096);
        assert_eq!(redacted["api_key"], "[REDACTED]");
    }

    #[test]
    fn test_secrets_jwt_pem_database_uri_and_custom_env() {
        // JWT
        let jwt = "Token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c in header";
        assert_eq!(mask_secrets(jwt), "Token: [REDACTED] in header");

        // PEM private key
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA0Y1...\n-----END RSA PRIVATE KEY-----";
        assert_eq!(mask_secrets(pem), "[REDACTED]");

        // DB connection URL
        let db_url = "Connecting to postgres://admin:super_secret_pw@db.internal:5432/production";
        assert_eq!(
            mask_secrets(db_url),
            "Connecting to postgres://admin:[REDACTED]@db.internal:5432/production"
        );

        // Generic custom env variable in .env
        let env_file = "CUSTOM_SECRET=super_secret_val_123\nDB_PASSWORD=my_db_pass\nPORT=8080\n";
        let masked_env = mask_secrets(env_file);
        assert!(masked_env.contains("CUSTOM_SECRET=[REDACTED]"));
        assert!(masked_env.contains("DB_PASSWORD=[REDACTED]"));
        assert!(masked_env.contains("PORT=8080"));
    }

    #[test]
    fn test_normal_values_not_masked() {
        // Cargo.toml name and keyboard_layout must remain completely untouched
        let cargo_toml = r#"[package]
name = "mathy"
version = "0.1.0"
edition = "2021"
"#;
        assert_eq!(mask_secrets(cargo_toml), cargo_toml);

        let config_line = "keyboard_layout = \"qwerty\"";
        assert_eq!(mask_secrets(config_line), config_line);

        // Kebab-case tool name starting with sk-
        let tool_name = "Running sk-docs-generator-tool for documentation generation.";
        assert_eq!(mask_secrets(tool_name), tool_name);

        // JSON fields named "key" and "token"
        let json_data = serde_json::json!({
            "key": "user_id_column",
            "token": "identifier_token",
            "api_key": "sk-realFake1234567890",
        });
        let redacted = redact_json_value(&json_data);
        assert_eq!(redacted["key"], "user_id_column");
        assert_eq!(redacted["token"], "identifier_token");
        assert_eq!(redacted["api_key"], "[REDACTED]");
    }

    #[test]
    fn test_streaming_secret_filter() {
        let mut filter = StreamingSecretFilter::new();

        // Chunks arrive in small fragments
        let out1 = filter.push("OPENAI_");
        assert_eq!(out1, "");
        let out2 = filter.push("API_KEY=");
        assert_eq!(out2, "");
        let out3 = filter.push("sk-live-1234567890abcdef\n");
        assert_eq!(out3, "OPENAI_API_KEY=[REDACTED]\n");

        // Remaining partial string flushed at stream close
        let out4 = filter.push("Normal text without newline");
        assert_eq!(out4, "");
        let out5 = filter.flush();
        assert_eq!(out5, "Normal text without newline");
    }

    #[test]
    fn test_sk_learn_not_masked_while_keys_are_masked() {
        // Python libraries and package identifiers must NOT be redacted
        let pkg_text = "Install sk-learn and sk-image or scikit-learn for machine learning.";
        let pkg_masked = mask_secrets(pkg_text);
        assert_eq!(pkg_masked, pkg_text);

        // Real and mock API keys must still be properly redacted
        let key_text = "API keys: sk-FAKEsevenTestKey and nvapi-FAKEfiveTestKey";
        let key_masked = mask_secrets(key_text);
        assert!(key_masked.contains("[REDACTED]"));
        assert!(!key_masked.contains("sk-FAKEsevenTestKey"));
        assert!(!key_masked.contains("nvapi-FAKEfiveTestKey"));
    }

    #[test]
    fn test_specific_secret_patterns_and_format_preservation() {
        // 1. MY_CUSTOM=abc123secretvalue must be masked
        let custom_env = "MY_CUSTOM=abc123secretvalue\nPORT=8080\n";
        let masked_custom = mask_secrets(custom_env);
        assert!(masked_custom.contains("MY_CUSTOM=[REDACTED]"));
        assert!(masked_custom.contains("PORT=8080"));

        // 2. Bare sk-abcdefghij must be masked
        let bare_sk = "OpenAI key: sk-abcdefghij in config";
        let masked_bare = mask_secrets(bare_sk);
        assert_eq!(masked_bare, "OpenAI key: [REDACTED] in config");

        // 3. YAML password: hunter2pass must be masked
        let yaml_content = "username: admin\npassword: hunter2pass\n";
        let masked_yaml = mask_secrets(yaml_content);
        assert_eq!(masked_yaml, "username: admin\npassword: [REDACTED]\n");

        // 4. Whitespace and quotes preservation (no stripping)
        let quoted_line = "api_key = \"x\"";
        let masked_quoted = mask_secrets(quoted_line);
        assert_eq!(masked_quoted, "api_key = \"[REDACTED]\"");

        let single_quoted = "secret_key = 'super_secret'";
        let masked_single = mask_secrets(single_quoted);
        assert_eq!(masked_single, "secret_key = '[REDACTED]'");
    }
}
