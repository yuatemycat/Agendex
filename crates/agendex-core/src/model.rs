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

pub struct ModelClient {
    client: reqwest::Client,
    pub config: ModelConfig,
}

impl ModelClient {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
    ) -> anyhow::Result<ModelResponse> {
        match self.config.format {
            ModelFormat::GoogleGemini => self.chat_gemini(messages, tools).await,
            ModelFormat::OpenAi => self.chat_openai(messages, tools).await,
            ModelFormat::Anthropic => self.chat_anthropic(messages, tools).await,
        }
    }

    async fn chat_gemini(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
    ) -> anyhow::Result<ModelResponse> {
        let base = self
            .config
            .base_url
            .as_deref()
            .unwrap_or("https://generativelanguage.googleapis.com");
        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            base.trim_end_matches('/'),
            self.config.model_name,
            self.config.api_key
        );

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

        let resp = self.client.post(&url).json(&payload).send().await?;

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

        Ok(ModelResponse {
            text: if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            },
            tool_calls,
            prompt_tokens,
            completion_tokens,
        })
    }

    async fn chat_openai(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
    ) -> anyhow::Result<ModelResponse> {
        let base = self
            .config
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com");
        let url = format!("{}/v1/chat/completions", base.trim_end_matches('/'));

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

        let mut req = self.client.post(&url).json(&payload);
        if !self.config.api_key.is_empty() {
            req = req.bearer_auth(&self.config.api_key);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let err_text = resp.text().await?;
            return Err(anyhow::anyhow!("OpenAI API error: {}", err_text));
        }

        let body: Value = resp.json().await?;
        let choice = body
            .pointer("/choices/0/message")
            .ok_or_else(|| anyhow::anyhow!("Invalid OpenAI response structure"))?;

        let text = choice
            .get("content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());
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

    async fn chat_anthropic(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
    ) -> anyhow::Result<ModelResponse> {
        let base = self
            .config
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com");
        let url = format!("{}/v1/messages", base.trim_end_matches('/'));

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

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err_text = resp.text().await?;
            return Err(anyhow::anyhow!("Anthropic API error: {}", err_text));
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

        let prompt_tokens = body
            .pointer("/usage/input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let completion_tokens = body
            .pointer("/usage/output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        Ok(ModelResponse {
            text: if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            },
            tool_calls,
            prompt_tokens,
            completion_tokens,
        })
    }
}
