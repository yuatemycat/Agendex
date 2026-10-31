use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelFormat {
    GoogleGemini,
    OpenAi,
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub format: ModelFormat,
    pub model_name: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        // Defaults to OpenAI format pointing to local gateway (e.g. Proxymateur or Ollama) or Gemini if key present
        if let Ok(key) = std::env::var("GEMINI_API_KEY") {
            Self {
                format: ModelFormat::GoogleGemini,
                model_name: "gemini-2.5-flash".to_string(),
                api_key: key,
                base_url: None,
                temperature: Some(0.2),
                max_output_tokens: Some(8192),
            }
        } else if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            Self {
                format: ModelFormat::Anthropic,
                model_name: "claude-3-5-sonnet-20241022".to_string(),
                api_key: key,
                base_url: None,
                temperature: Some(0.2),
                max_output_tokens: Some(8192),
            }
        } else if let Ok(key) = std::env::var("NVIDIA_API_KEY") {
            Self {
                format: ModelFormat::OpenAi,
                model_name: "meta/llama-3.1-70b-instruct".to_string(),
                api_key: key,
                base_url: Some("https://integrate.api.nvidia.com/v1".to_string()),
                temperature: Some(0.2),
                max_output_tokens: Some(4096),
            }
        } else {
            let key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "dummy_key".to_string());
            let base_url = std::env::var("OPENAI_BASE_URL").ok();
            Self {
                format: ModelFormat::OpenAi,
                model_name: "gpt-4o".to_string(),
                api_key: key,
                base_url,
                temperature: Some(0.2),
                max_output_tokens: Some(8192),
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "system", "user", "assistant", "tool"
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Option<Vec<ToolCall>>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.unwrap_or_default(),
            tool_calls,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            tool_name: Some(tool_name.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
}

/// Resolves endpoint URL for OpenAI wire format, avoiding double `/v1/v1`.
pub fn resolve_openai_endpoint(base: &str) -> String {
    let base_trimmed = base.trim_end_matches('/');
    if base_trimmed.ends_with("/v1") {
        format!("{base_trimmed}/chat/completions")
    } else {
        format!("{base_trimmed}/v1/chat/completions")
    }
}

/// Resolves endpoint URL for Anthropic wire format, avoiding double `/v1/v1`.
pub fn resolve_anthropic_endpoint(base: &str) -> String {
    let base_trimmed = base.trim_end_matches('/');
    if base_trimmed.ends_with("/v1") {
        format!("{base_trimmed}/messages")
    } else {
        format!("{base_trimmed}/v1/messages")
    }
}

/// Resolves endpoint URL for Google Gemini wire format, avoiding double `/v1beta/v1beta`.
pub fn resolve_gemini_endpoint(base: &str, model: &str, api_key: &str) -> String {
    let base_trimmed = base.trim_end_matches('/');
    if base_trimmed.ends_with("/v1beta") {
        format!("{base_trimmed}/models/{model}:generateContent?key={api_key}")
    } else {
        format!("{base_trimmed}/v1beta/models/{model}:generateContent?key={api_key}")
    }
}

pub type TokenStreamHandler = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

pub struct ModelClient {
    client: reqwest::Client,
    pub config: ModelConfig,
    pub token_callback: Option<TokenStreamHandler>,
    pub timeout_secs: u64,
}

async fn next_chunk_with_timeout(
    resp: &mut reqwest::Response,
    gap_timeout: std::time::Duration,
) -> anyhow::Result<Option<Vec<u8>>> {
    match tokio::time::timeout(gap_timeout, resp.chunk()).await {
        Ok(res) => match res? {
            Some(b) => Ok(Some(b.to_vec())),
            None => Ok(None),
        },
        Err(_) => Err(anyhow::anyhow!(
            "Stream read timed out: no response chunk received from model provider for {} seconds",
            gap_timeout.as_secs()
        )),
    }
}

async fn execute_request_with_retry<F>(
    build_req: F,
    ttfb_timeout: std::time::Duration,
) -> anyhow::Result<reqwest::Response>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempts = 0;
    let max_attempts = 5;
    let mut backoff = std::time::Duration::from_secs(1);

    loop {
        attempts += 1;
        let req = build_req();
        let resp_res = tokio::time::timeout(ttfb_timeout, req.send()).await;

        match resp_res {
            Ok(Ok(resp)) => {
                let status = resp.status();
                if (status == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || status == reqwest::StatusCode::BAD_GATEWAY
                    || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                    || status == reqwest::StatusCode::GATEWAY_TIMEOUT
                    || status == reqwest::StatusCode::INTERNAL_SERVER_ERROR)
                    && attempts < max_attempts
                {
                    let sleep_duration = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|h| h.to_str().ok())
                        .and_then(|s| {
                            if let Ok(secs) = s.trim().parse::<u64>() {
                                Some(std::time::Duration::from_secs(secs))
                            } else if let Ok(secs_f) = s.trim().parse::<f64>() {
                                Some(std::time::Duration::from_millis((secs_f * 1000.0) as u64))
                            } else {
                                None
                            }
                        })
                        .unwrap_or(backoff)
                        .clamp(
                            std::time::Duration::from_millis(500),
                            std::time::Duration::from_secs(60),
                        );

                    tracing::warn!(
                        "Provider returned HTTP {} (attempt {}/{}). Retrying after {:?}...",
                        status,
                        attempts,
                        max_attempts,
                        sleep_duration
                    );
                    tokio::time::sleep(sleep_duration).await;
                    backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                    continue;
                }
                return Ok(resp);
            }
            Ok(Err(e)) => {
                if e.is_timeout() {
                    return Err(anyhow::anyhow!(
                        "Request timed out waiting for initial response from provider (exceeded {} seconds)",
                        ttfb_timeout.as_secs()
                    ));
                }
                if attempts < max_attempts {
                    tracing::warn!(
                        "Provider request transport error (attempt {}/{}): {}. Retrying in {:?}...",
                        attempts,
                        max_attempts,
                        e,
                        backoff
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                    continue;
                }
                return Err(e.into());
            }
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Request timed out waiting for initial response from provider (exceeded {} seconds)",
                    ttfb_timeout.as_secs()
                ));
            }
        }
    }
}

impl ModelClient {
    pub fn new(config: ModelConfig) -> Self {
        let timeout_secs = std::env::var("AGENDEX_REQUEST_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(120);

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            config,
            token_callback: None,
            timeout_secs,
        }
    }

    pub fn with_token_callback(mut self, callback: TokenStreamHandler) -> Self {
        self.token_callback = Some(callback);
        self
    }

    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
    ) -> anyhow::Result<ModelResponse> {
        self.chat_with_stream(messages, tools, self.token_callback.clone())
            .await
    }

    pub async fn chat_with_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        on_token: Option<TokenStreamHandler>,
    ) -> anyhow::Result<ModelResponse> {
        let mut stream_attempts = 0;
        let max_stream_attempts = 5;
        let mut stream_backoff = std::time::Duration::from_millis(1500);

        loop {
            stream_attempts += 1;
            let res = match self.config.format {
                ModelFormat::GoogleGemini => {
                    self.chat_gemini_streaming(messages, tools, on_token.clone())
                        .await
                }
                ModelFormat::OpenAi => {
                    self.chat_openai_streaming(messages, tools, on_token.clone())
                        .await
                }
                ModelFormat::Anthropic => {
                    self.chat_anthropic_streaming(messages, tools, on_token.clone())
                        .await
                }
            };

            match res {
                Ok(resp) => return Ok(resp),
                Err(err) => {
                    let err_msg = err.to_string();
                    let is_broken_stream = (err_msg.contains("connection closed")
                        || err_msg.contains("channel closed")
                        || err_msg.contains("broken pipe")
                        || err_msg.contains("stream ended prematurely")
                        || err_msg.contains("Connection reset by peer")
                        || err_msg.contains("error reading a body from connection")
                        || err_msg.contains("error decoding response body")
                        || err_msg.contains("connection error")
                        || err_msg.contains("hyper::Error")
                        || err_msg.contains("chunked transfer")
                        || err_msg.contains("premature EOF")
                        || err_msg.contains("unexpected end of file")
                        || err_msg.contains("incomplete message")
                        || err_msg.contains("stream error"))
                        && !err_msg.contains("Stream read timed out");

                    if is_broken_stream && stream_attempts < max_stream_attempts {
                        tracing::warn!(
                            "Streaming connection interrupted halfway (attempt {}/{}): {}. Retrying request...",
                            stream_attempts,
                            max_stream_attempts,
                            err_msg
                        );
                        tokio::time::sleep(stream_backoff).await;
                        stream_backoff *= 2;
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }

    async fn chat_gemini_streaming(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        on_token: Option<TokenStreamHandler>,
    ) -> anyhow::Result<ModelResponse> {
        let base = self
            .config
            .base_url
            .as_deref()
            .unwrap_or("https://generativelanguage.googleapis.com");
        let url = resolve_gemini_endpoint(base, &self.config.model_name, &self.config.api_key);

        let mut system_parts = Vec::new();
        let mut contents = Vec::new();

        for msg in messages {
            if msg.role == "system" {
                system_parts.push(msg.content.clone());
                continue;
            }

            if msg.role == "tool" || msg.tool_call_id.is_some() {
                let name = msg.tool_name.as_deref().unwrap_or("tool");
                let response_content = serde_json::from_str::<Value>(&msg.content)
                    .unwrap_or_else(|_| serde_json::json!({ "output": msg.content }));

                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": name,
                            "response": {
                                "name": name,
                                "content": response_content
                            }
                        }
                    }]
                }));
                continue;
            }

            let role = if msg.role == "assistant" {
                "model"
            } else {
                "user"
            };

            let mut parts = Vec::new();
            if !msg.content.is_empty() {
                parts.push(serde_json::json!({ "text": msg.content }));
            }
            if let Some(tcs) = &msg.tool_calls {
                for tc in tcs {
                    parts.push(serde_json::json!({
                        "functionCall": {
                            "name": tc.name,
                            "args": tc.arguments,
                        }
                    }));
                }
            }

            if !parts.is_empty() {
                contents.push(serde_json::json!({
                    "role": role,
                    "parts": parts,
                }));
            }
        }

        let mut payload = serde_json::json!({ "contents": contents });
        if !system_parts.is_empty() {
            let combined = system_parts.join("\n\n");
            payload["systemInstruction"] = serde_json::json!({
                "parts": [{ "text": combined }]
            });
        }
        if !tools.is_empty() {
            let function_declarations: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.get("name").unwrap_or(&Value::Null),
                        "description": t.get("description").unwrap_or(&Value::Null),
                        "parameters": t.get("parameters").unwrap_or(&Value::Null),
                    })
                })
                .collect();
            payload["tools"] =
                serde_json::json!([{ "functionDeclarations": function_declarations }]);
        }

        let client = self.client.clone();
        let url_clone = url.clone();
        let payload_clone = payload.clone();
        let resp = execute_request_with_retry(
            move || client.post(&url_clone).json(&payload_clone),
            std::time::Duration::from_secs(self.timeout_secs),
        )
        .await?;

        if !resp.status().is_success() {
            let err_text = resp.text().await?;
            return Err(anyhow::anyhow!("Gemini API error: {}", err_text));
        }

        let body: Value = resp.json().await?;
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        if let Some(candidates) = body.get("candidates").and_then(|c| c.as_array()) {
            if let Some(first) = candidates.first() {
                if let Some(parts) = first.pointer("/content/parts").and_then(|p| p.as_array()) {
                    for part in parts {
                        if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                            text_parts.push(txt.to_string());
                        }
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args = fc.get("args").cloned().unwrap_or(Value::Null);
                            tool_calls.push(ToolCall {
                                id: uuid::Uuid::new_v4().to_string(),
                                name,
                                arguments: args,
                            });
                        }
                    }
                }
            }
        }

        let prompt_tokens = body
            .pointer("/usageMetadata/promptTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let completion_tokens = body
            .pointer("/usageMetadata/candidatesTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let full_text = if text_parts.is_empty() {
            None
        } else {
            let combined = text_parts.join("\n");
            if let Some(ref cb) = on_token {
                cb(&combined);
            }
            Some(combined)
        };

        Ok(ModelResponse {
            text: full_text,
            tool_calls,
            prompt_tokens,
            completion_tokens,
        })
    }

    async fn chat_openai_streaming(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        on_token: Option<TokenStreamHandler>,
    ) -> anyhow::Result<ModelResponse> {
        let base = self
            .config
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com");
        let url = resolve_openai_endpoint(base);

        let mut payload_messages = Vec::new();
        for msg in messages {
            let mut obj = serde_json::json!({
                "role": msg.role,
                "content": msg.content,
            });
            if let Some(tcs) = &msg.tool_calls {
                let openai_tcs: Vec<Value> = tcs.iter().map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                        }
                    })
                }).collect();
                obj["tool_calls"] = serde_json::json!(openai_tcs);
            }
            if let Some(tid) = &msg.tool_call_id {
                obj["tool_call_id"] = serde_json::json!(tid);
            }
            payload_messages.push(obj);
        }

        let mut payload = serde_json::json!({
            "model": self.config.model_name,
            "messages": payload_messages,
        });

        if on_token.is_some() {
            payload["stream"] = serde_json::json!(true);
            payload["stream_options"] = serde_json::json!({ "include_usage": true });
        }

        if !tools.is_empty() {
            let openai_tools: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": t
                    })
                })
                .collect();
            payload["tools"] = serde_json::json!(openai_tools);
        }

        let client = self.client.clone();
        let url_clone = url.clone();
        let payload_clone = payload.clone();
        let api_key = self.config.api_key.clone();

        let mut resp = execute_request_with_retry(
            move || {
                let mut r = client.post(&url_clone).json(&payload_clone);
                if !api_key.is_empty() {
                    r = r.bearer_auth(&api_key);
                }
                r
            },
            std::time::Duration::from_secs(self.timeout_secs),
        )
        .await?;
        if !resp.status().is_success() {
            let err_text = resp.text().await?;
            return Err(anyhow::anyhow!("OpenAI API error: {}", err_text));
        }

        let is_event_stream = resp
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.contains("text/event-stream"))
            .unwrap_or(false);

        if is_event_stream {
            let mut full_text = String::new();
            let mut tool_calls_map: std::collections::BTreeMap<usize, (String, String, String)> =
                std::collections::BTreeMap::new();
            let mut prompt_tokens = 0usize;
            let mut completion_tokens = 0usize;

            let mut buffer = String::new();
            while let Some(chunk) = next_chunk_with_timeout(
                &mut resp,
                std::time::Duration::from_secs(self.timeout_secs),
            )
            .await?
            {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim().to_string();
                    buffer = buffer[pos + 1..].to_string();
                    if line.is_empty() {
                        continue;
                    }
                    if let Some(data) = line.strip_prefix("data:") {
                        let data = data.trim();
                        if data == "[DONE]" {
                            break;
                        }
                        if let Ok(val) = serde_json::from_str::<Value>(data) {
                            if let Some(pt) =
                                val.pointer("/usage/prompt_tokens").and_then(|v| v.as_u64())
                            {
                                prompt_tokens = pt as usize;
                            }
                            if let Some(ct) = val
                                .pointer("/usage/completion_tokens")
                                .and_then(|v| v.as_u64())
                            {
                                completion_tokens = ct as usize;
                            }
                            if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
                                if let Some(first) = choices.first() {
                                    if let Some(delta) = first.get("delta") {
                                        if let Some(content) =
                                            delta.get("content").and_then(|c| c.as_str())
                                        {
                                            if !content.is_empty() {
                                                full_text.push_str(content);
                                                if let Some(ref cb) = on_token {
                                                    cb(content);
                                                }
                                            }
                                        }
                                        if let Some(tcs) =
                                            delta.get("tool_calls").and_then(|t| t.as_array())
                                        {
                                            for tc in tcs {
                                                let idx = tc
                                                    .get("index")
                                                    .and_then(|i| i.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let entry = tool_calls_map
                                                    .entry(idx)
                                                    .or_insert_with(|| {
                                                        (
                                                            String::new(),
                                                            String::new(),
                                                            String::new(),
                                                        )
                                                    });
                                                if let Some(id) =
                                                    tc.get("id").and_then(|i| i.as_str())
                                                {
                                                    entry.0.push_str(id);
                                                }
                                                if let Some(name) = tc
                                                    .pointer("/function/name")
                                                    .and_then(|n| n.as_str())
                                                {
                                                    entry.1.push_str(name);
                                                }
                                                if let Some(args) = tc
                                                    .pointer("/function/arguments")
                                                    .and_then(|a| a.as_str())
                                                {
                                                    entry.2.push_str(args);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let mut tool_calls = Vec::new();
            for (_, (id, name, args_str)) in tool_calls_map {
                let args: Value = serde_json::from_str(&args_str).unwrap_or(Value::Null);
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments: args,
                });
            }

            return Ok(ModelResponse {
                text: if full_text.is_empty() {
                    None
                } else {
                    Some(full_text)
                },
                tool_calls,
                prompt_tokens,
                completion_tokens,
            });
        }

        let body: Value = resp.json().await?;
        let choice = body
            .pointer("/choices/0/message")
            .ok_or_else(|| anyhow::anyhow!("Invalid OpenAI response structure"))?;

        let text = choice
            .get("content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        if let Some(ref t) = text {
            if let Some(ref cb) = on_token {
                cb(t);
            }
        }

        let mut tool_calls = Vec::new();

        if let Some(tcs) = choice.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let id = tc
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = tc
                    .pointer("/function/name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let args_str = tc
                    .pointer("/function/arguments")
                    .and_then(|a| a.as_str())
                    .unwrap_or("{}");
                let args: Value = serde_json::from_str(args_str).unwrap_or(Value::Null);
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments: args,
                });
            }
        }

        let prompt_tokens = body
            .pointer("/usage/prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let completion_tokens = body
            .pointer("/usage/completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        Ok(ModelResponse {
            text,
            tool_calls,
            prompt_tokens,
            completion_tokens,
        })
    }

    async fn chat_anthropic_streaming(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        on_token: Option<TokenStreamHandler>,
    ) -> anyhow::Result<ModelResponse> {
        let base = self
            .config
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com");
        let url = resolve_anthropic_endpoint(base);

        let mut system_prompt = String::new();
        let mut anthropic_messages = Vec::new();

        for msg in messages {
            if msg.role == "system" {
                system_prompt.push_str(&msg.content);
                system_prompt.push('\n');
                continue;
            }

            if msg.role == "tool" || msg.tool_call_id.is_some() {
                let tid = msg.tool_call_id.as_deref().unwrap_or("tool_call");
                anthropic_messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tid,
                        "content": msg.content,
                    }]
                }));
                continue;
            }

            let role = if msg.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            let mut contents = Vec::new();

            if !msg.content.is_empty() {
                contents.push(serde_json::json!({
                    "type": "text",
                    "text": msg.content
                }));
            }

            if let Some(tcs) = &msg.tool_calls {
                for tc in tcs {
                    contents.push(serde_json::json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": tc.arguments,
                    }));
                }
            }

            if !contents.is_empty() {
                anthropic_messages.push(serde_json::json!({
                    "role": role,
                    "content": contents
                }));
            }
        }

        let mut payload = serde_json::json!({
            "model": self.config.model_name,
            "max_tokens": self.config.max_output_tokens.unwrap_or(4096),
            "messages": anthropic_messages,
        });

        if on_token.is_some() {
            payload["stream"] = serde_json::json!(true);
        }

        if !system_prompt.is_empty() {
            payload["system"] = serde_json::json!(system_prompt.trim());
        }

        if !tools.is_empty() {
            let anthropic_tools: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.get("name").unwrap_or(&Value::Null),
                        "description": t.get("description").unwrap_or(&Value::Null),
                        "input_schema": t.get("parameters").unwrap_or(&Value::Null),
                    })
                })
                .collect();
            payload["tools"] = serde_json::json!(anthropic_tools);
        }

        let client = self.client.clone();
        let url_clone = url.clone();
        let payload_clone = payload.clone();
        let api_key = self.config.api_key.clone();

        let mut resp = execute_request_with_retry(
            move || {
                client
                    .post(&url_clone)
                    .header("x-api-key", &api_key)
                    .header("anthropic-version", "2023-06-01")
                    .json(&payload_clone)
            },
            std::time::Duration::from_secs(self.timeout_secs),
        )
        .await?;

        if !resp.status().is_success() {
            let err_text = resp.text().await?;
            return Err(anyhow::anyhow!("Anthropic API error: {}", err_text));
        }

        let is_event_stream = resp
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.contains("text/event-stream"))
            .unwrap_or(false);

        if is_event_stream {
            let mut full_text = String::new();
            let mut tool_calls = Vec::new();
            let mut current_tool_call: Option<(String, String, String)> = None;
            let mut prompt_tokens = 0usize;
            let mut completion_tokens = 0usize;

            let mut buffer = String::new();
            while let Some(chunk) = next_chunk_with_timeout(
                &mut resp,
                std::time::Duration::from_secs(self.timeout_secs),
            )
            .await?
            {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim().to_string();
                    buffer = buffer[pos + 1..].to_string();
                    if line.is_empty() {
                        continue;
                    }
                    if let Some(data) = line.strip_prefix("data:") {
                        let data = data.trim();
                        if let Ok(val) = serde_json::from_str::<Value>(data) {
                            let event_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            match event_type {
                                "content_block_start" => {
                                    if let Some(cb) = val.get("content_block") {
                                        if cb.get("type").and_then(|t| t.as_str())
                                            == Some("tool_use")
                                        {
                                            let id = cb
                                                .get("id")
                                                .and_then(|i| i.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let name = cb
                                                .get("name")
                                                .and_then(|n| n.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            current_tool_call = Some((id, name, String::new()));
                                        }
                                    }
                                }
                                "content_block_delta" => {
                                    if let Some(delta) = val.get("delta") {
                                        let delta_type = delta
                                            .get("type")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("");
                                        if delta_type == "text_delta" {
                                            if let Some(txt) =
                                                delta.get("text").and_then(|t| t.as_str())
                                            {
                                                full_text.push_str(txt);
                                                if let Some(ref cb) = on_token {
                                                    cb(txt);
                                                }
                                            }
                                        } else if delta_type == "input_json_delta" {
                                            if let Some(partial) =
                                                delta.get("partial_json").and_then(|p| p.as_str())
                                            {
                                                if let Some(ref mut tc) = current_tool_call {
                                                    tc.2.push_str(partial);
                                                }
                                            }
                                        }
                                    }
                                }
                                "content_block_stop" => {
                                    if let Some((id, name, args_str)) = current_tool_call.take() {
                                        let arguments: Value =
                                            serde_json::from_str(&args_str).unwrap_or(Value::Null);
                                        tool_calls.push(ToolCall {
                                            id,
                                            name,
                                            arguments,
                                        });
                                    }
                                }
                                "message_start" => {
                                    if let Some(it) = val
                                        .pointer("/message/usage/input_tokens")
                                        .and_then(|v| v.as_u64())
                                    {
                                        prompt_tokens = it as usize;
                                    }
                                }
                                "message_delta" => {
                                    if let Some(ot) =
                                        val.pointer("/usage/output_tokens").and_then(|v| v.as_u64())
                                    {
                                        completion_tokens = ot as usize;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            if let Some((id, name, args_str)) = current_tool_call.take() {
                let arguments: Value = serde_json::from_str(&args_str).unwrap_or(Value::Null);
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }

            return Ok(ModelResponse {
                text: if full_text.is_empty() {
                    None
                } else {
                    Some(full_text)
                },
                tool_calls,
                prompt_tokens,
                completion_tokens,
            });
        }

        let body: Value = resp.json().await?;
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        if let Some(content_blocks) = body.get("content").and_then(|c| c.as_array()) {
            for block in content_blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(|s| s.as_str()) {
                            text_parts.push(t.to_string());
                        }
                    }
                    Some("tool_use") => {
                        let id = block
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let input = block.get("input").cloned().unwrap_or(Value::Null);
                        tool_calls.push(ToolCall {
                            id,
                            name,
                            arguments: input,
                        });
                    }
                    _ => {}
                }
            }
        }

        let full_text = if text_parts.is_empty() {
            None
        } else {
            let combined = text_parts.join("\n");
            if let Some(ref cb) = on_token {
                cb(&combined);
            }
            Some(combined)
        };

        let prompt_tokens = body
            .pointer("/usage/input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let completion_tokens = body
            .pointer("/usage/output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        Ok(ModelResponse {
            text: full_text,
            tool_calls,
            prompt_tokens,
            completion_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_openai_endpoint_v1_handling() {
        // When provider base URL ends with /v1
        assert_eq!(
            resolve_openai_endpoint("https://integrate.api.nvidia.com/v1"),
            "https://integrate.api.nvidia.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_openai_endpoint("https://integrate.api.nvidia.com/v1/"),
            "https://integrate.api.nvidia.com/v1/chat/completions"
        );
        // When provider base URL does not have /v1
        assert_eq!(
            resolve_openai_endpoint("https://api.openai.com"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_openai_endpoint("https://api.openai.com/"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            resolve_openai_endpoint("http://localhost:11434/v1"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn test_resolve_anthropic_endpoint_v1_handling() {
        assert_eq!(
            resolve_anthropic_endpoint("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            resolve_anthropic_endpoint("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn test_resolve_gemini_endpoint_v1beta_handling() {
        assert_eq!(
            resolve_gemini_endpoint(
                "https://generativelanguage.googleapis.com/v1beta",
                "gemini-2.5-flash",
                "fake_key"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key=fake_key"
        );
        assert_eq!(
            resolve_gemini_endpoint(
                "https://generativelanguage.googleapis.com",
                "gemini-2.5-flash",
                "fake_key"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key=fake_key"
        );
    }
}
