use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    #[serde(default)]
    pub server_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfigFile {
    #[serde(default, alias = "mcpServers")]
    pub servers: HashMap<String, McpServerConfigEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfigEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

pub struct McpClient {
    pub server_name: String,
    child: Option<Child>,
    stdin: Option<tokio::process::ChildStdin>,
    reader: Option<tokio::io::Lines<BufReader<tokio::process::ChildStdout>>>,
}

impl McpClient {
    pub fn new(server_name: String) -> Self {
        Self {
            server_name,
            child: None,
            stdin: None,
            reader: None,
        }
    }

    pub async fn spawn(&mut self, config: McpServerConfig) -> anyhow::Result<()> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to acquire stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to acquire stdout"))?;
        let reader = BufReader::new(stdout).lines();

        self.child = Some(child);
        self.stdin = Some(stdin);
        self.reader = Some(reader);

        // Perform MCP protocol startup handshake
        self.handshake().await?;

        Ok(())
    }

    /// Perform official MCP protocol initialization handshake:
    /// 1. Send `initialize` JSON-RPC request.
    /// 2. Read server capabilities and info.
    /// 3. Send `notifications/initialized` notification.
    pub async fn handshake(&mut self) -> anyhow::Result<serde_json::Value> {
        let init_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "agendex",
                    "version": "0.1.0"
                }
            }
        });

        self.send_json(&init_req).await?;
        let resp = self.read_json().await?;

        // Send notifications/initialized
        let notify = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.send_json(&notify).await?;

        Ok(resp)
    }

    /// Request available tools from the server via `tools/list`
    pub async fn list_tools_from_server(&mut self) -> anyhow::Result<Vec<McpToolDefinition>> {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        });

        self.send_json(&req).await?;
        let resp = self.read_json().await?;

        let mut definitions = Vec::new();
        if let Some(tools) = resp.pointer("/result/tools").and_then(|t| t.as_array()) {
            for tool in tools {
                let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let desc = tool
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string());
                let schema = tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(serde_json::json!({"type": "object"}));

                // Qualify tool name with server prefix: mcp_<server>_<tool>
                let qualified_name = format!("mcp_{}_{}", self.server_name, name);
                definitions.push(McpToolDefinition {
                    name: qualified_name,
                    description: desc,
                    input_schema: schema,
                    server_name: Some(self.server_name.clone()),
                });
            }
        }

        Ok(definitions)
    }

    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        // Strip mcp_<server>_ prefix if present
        let clean_name =
            if let Some(stripped) = tool_name.strip_prefix(&format!("mcp_{}_", self.server_name)) {
                stripped
            } else {
                tool_name
            };

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": uuid::Uuid::new_v4().to_string(),
            "method": "tools/call",
            "params": {
                "name": clean_name,
                "arguments": arguments,
            }
        });

        self.send_json(&req).await?;
        let resp = self.read_json().await?;

        if let Some(res) = resp.get("result") {
            Ok(res.clone())
        } else if let Some(err) = resp.get("error") {
            Err(anyhow::anyhow!("MCP error: {}", err))
        } else {
            Ok(serde_json::json!({"status": "executed"}))
        }
    }

    async fn send_json(&mut self, val: &serde_json::Value) -> anyhow::Result<()> {
        if let Some(stdin) = &mut self.stdin {
            let line = serde_json::to_string(val)? + "\n";
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("MCP client stdin not connected"))
        }
    }

    async fn read_json(&mut self) -> anyhow::Result<serde_json::Value> {
        if let Some(reader) = &mut self.reader {
            if let Some(line) = reader.next_line().await? {
                let resp: serde_json::Value = serde_json::from_str(&line)?;
                Ok(resp)
            } else {
                Err(anyhow::anyhow!("MCP server closed output stream"))
            }
        } else {
            Err(anyhow::anyhow!("MCP client stdout reader not connected"))
        }
    }
}

pub struct McpRegistry {
    pub clients: Arc<Mutex<HashMap<String, McpClient>>>,
    pub tools: Arc<Mutex<HashMap<String, McpToolDefinition>>>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            tools: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn register_tool(&self, def: McpToolDefinition) {
        let mut tools = self.tools.lock().await;
        tools.insert(def.name.clone(), def);
    }

    pub async fn list_tools(&self) -> Vec<McpToolDefinition> {
        let tools = self.tools.lock().await;
        tools.values().cloned().collect()
    }

    /// Spawns an MCP server, runs the protocol handshake, discovers tools, and registers them
    pub async fn spawn_server(
        &self,
        server_name: &str,
        config: McpServerConfig,
    ) -> anyhow::Result<usize> {
        let mut client = McpClient::new(server_name.to_string());
        client.spawn(config).await?;

        let server_tools = client.list_tools_from_server().await?;
        let count = server_tools.len();

        for tool in server_tools {
            self.register_tool(tool).await;
        }

        let mut clients = self.clients.lock().await;
        clients.insert(server_name.to_string(), client);

        Ok(count)
    }

    /// Call an MCP tool by its qualified name (e.g. mcp_server_tool)
    pub async fn call_tool(
        &self,
        qualified_tool_name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let (server_name, _) = {
            let tools = self.tools.lock().await;
            let def = tools
                .get(qualified_tool_name)
                .ok_or_else(|| anyhow::anyhow!("Tool '{}' not registered", qualified_tool_name))?;
            let sname = def
                .server_name
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Tool has no server association"))?;
            (sname, def.name.clone())
        };

        let mut clients = self.clients.lock().await;
        let client = clients
            .get_mut(&server_name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not connected", server_name))?;

        client.call_tool(qualified_tool_name, arguments).await
    }

    /// Loads an MCP configuration JSON file and spawns all defined servers
    pub async fn load_config_file(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<usize> {
        let content = tokio::fs::read_to_string(path).await?;
        let config: McpConfigFile = serde_json::from_str(&content)?;
        let mut total_tools = 0;
        for (name, entry) in config.servers {
            let server_cfg = McpServerConfig {
                command: entry.command,
                args: entry.args,
                env: entry.env,
            };
            match self.spawn_server(&name, server_cfg).await {
                Ok(count) => {
                    tracing::info!("Spawned MCP server '{}' with {} tools", name, count);
                    total_tools += count;
                }
                Err(e) => {
                    tracing::warn!("Failed to spawn MCP server '{}': {}", name, e);
                }
            }
        }
        Ok(total_tools)
    }
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_registry_basics() {
        let registry = McpRegistry::new();
        registry
            .register_tool(McpToolDefinition {
                name: "mcp_test_query".to_string(),
                description: Some("A test MCP tool".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
                server_name: Some("test".to_string()),
            })
            .await;

        let tools = registry.list_tools().await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "mcp_test_query");
    }

    #[test]
    fn test_mcp_config_deserialization() {
        let json_raw = r#"{
            "mcpServers": {
                "sqlite": {
                    "command": "uvx",
                    "args": ["mcp-server-sqlite", "--db-path", "test.db"],
                    "env": { "DEBUG": "1" }
                }
            }
        }"#;

        let parsed: McpConfigFile = serde_json::from_str(json_raw).unwrap();
        assert_eq!(parsed.servers.len(), 1);
        let s = parsed.servers.get("sqlite").unwrap();
        assert_eq!(s.command, "uvx");
        assert_eq!(s.args, vec!["mcp-server-sqlite", "--db-path", "test.db"]);
        assert_eq!(s.env.get("DEBUG").unwrap(), "1");
    }
}
