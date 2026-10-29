use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;

use agendex_core::agent::{AgendexAgent, AgentConfig};
use agendex_core::model::{ModelConfig, ModelFormat};
use agendex_trajectory::compactor::ModelTier;
use serde_json::json;

/// Lightweight local HTTP mock server for multi-turn model responses.
struct MockModelServer {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

impl MockModelServer {
    pub async fn start(responses: Vec<serde_json::Value>) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let response_queue = Arc::new(tokio::sync::Mutex::new(responses));

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept_res = listener.accept() => {
                        let (mut stream, _) = match accept_res {
                            Ok(conn) => conn,
                            Err(_) => break,
                        };

                        let q = Arc::clone(&response_queue);
                        tokio::spawn(async move {
                            use tokio::io::{AsyncReadExt, AsyncWriteExt};
                            let mut buf = vec![0u8; 16384];
                            let n = match stream.read(&mut buf).await {
                                Ok(n) if n > 0 => n,
                                _ => return,
                            };

                            let req_text = String::from_utf8_lossy(&buf[..n]);
                            if req_text.starts_with("POST") {
                                let mut queue = q.lock().await;
                                let resp_val = if !queue.is_empty() {
                                    queue.remove(0)
                                } else {
                                    json!({
                                        "choices": [{
                                            "message": {
                                                "role": "assistant",
                                                "content": "All requested changes are complete."
                                            }
                                        }],
                                        "usage": { "prompt_tokens": 100, "completion_tokens": 20 }
                                    })
                                };

                                let body = serde_json::to_string(&resp_val).unwrap();
                                let resp = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                );
                                let _ = stream.write_all(resp.as_bytes()).await;
                            }
                        });
                    }
                }
            }
        });

        Ok(Self {
            addr,
            shutdown: shutdown_tx,
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }
}

impl Drop for MockModelServer {
    fn drop(&mut self) {
        // Send shutdown signal
        let _ = self.shutdown;
    }
}

/// Helper to scaffold a medium-sized repository structure.
async fn create_medium_repo_fixture(root: &Path) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(root.join("src")).await?;
    tokio::fs::create_dir_all(root.join("tests")).await?;
    tokio::fs::create_dir_all(root.join("config")).await?;

    tokio::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"auth-service\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    )
    .await?;

    tokio::fs::write(
        root.join("src/lib.rs"),
        "pub mod auth;\npub mod config;\npub mod db;\npub mod api;\n",
    )
    .await?;

    tokio::fs::write(
        root.join("src/auth.rs"),
        "// Auth module implementation\npub fn verify_token(token: &str) -> bool {\n    token == \"secret_admin_token\"\n}\n",
    )
    .await?;

    tokio::fs::write(
        root.join("src/config.rs"),
        "pub struct AppConfig {\n    pub port: u16,\n}\n",
    )
    .await?;

    tokio::fs::write(
        root.join("src/db.rs"),
        "pub struct DatabaseClient {\n    pub connection_url: String,\n}\n",
    )
    .await?;

    tokio::fs::write(
        root.join("src/api.rs"),
        "pub fn router() -> &'static str {\n    \"/api/v1/auth\"\n}\n",
    )
    .await?;

    tokio::fs::write(
        root.join("tests/integration.rs"),
        "#[test]\nfn test_auth() {\n    assert!(true);\n}\n",
    )
    .await?;

    // Commit baseline in git
    let _ = tokio::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .output()
        .await?;
    let _ = tokio::process::Command::new("git")
        .args(["config", "user.name", "Tester"])
        .current_dir(root)
        .output()
        .await?;
    let _ = tokio::process::Command::new("git")
        .args(["config", "user.email", "test@test.local"])
        .current_dir(root)
        .output()
        .await?;
    let _ = tokio::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(root)
        .output()
        .await?;
    let _ = tokio::process::Command::new("git")
        .args(["commit", "-m", "Initial baseline commit"])
        .current_dir(root)
        .output()
        .await?;

    Ok(())
}

#[tokio::test]
async fn test_medium_repo_execution_with_context_compaction_and_revert() -> anyhow::Result<()> {
    let test_dir =
        std::env::temp_dir().join(format!("agendex_test_medium_repo_{}", uuid::Uuid::new_v4()));
    create_medium_repo_fixture(&test_dir).await?;

    // Define multi-turn model responses:
    // Turn 1: Model calls glob to scan rust files
    let resp1 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Finding all Rust source files in repository...",
                "tool_calls": [{
                    "id": "tc_glob_1",
                    "type": "function",
                    "function": {
                        "name": "glob",
                        "arguments": json!({ "pattern": "**/*.rs" }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 120, "completion_tokens": 25 }
    });

    // Turn 2: Model calls grep to find verify_token in auth.rs
    let resp2 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Searching for token verification logic...",
                "tool_calls": [{
                    "id": "tc_grep_1",
                    "type": "function",
                    "function": {
                        "name": "grep",
                        "arguments": json!({
                            "pattern": "verify_token",
                            "glob": "*.rs"
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 250, "completion_tokens": 30 }
    });

    // Turn 3: Model edits src/auth.rs to patch the token check
    let resp3 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Updating verify_token function in src/auth.rs...",
                "tool_calls": [{
                    "id": "tc_edit_1",
                    "type": "function",
                    "function": {
                        "name": "edit_file",
                        "arguments": json!({
                            "path": "src/auth.rs",
                            "target": "    token == \"secret_admin_token\"",
                            "replacement": "    token == \"patched_secure_token\""
                        }).to_string()
                    }
                }]
            }
        }],
        // Notice: High token count here triggers compaction ceiling (ModelTier::Custom(500))
        "usage": { "prompt_tokens": 480, "completion_tokens": 40 }
    });

    // Turn 4: Final completion message
    let resp4 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Security patch applied to verify_token successfully."
            }
        }],
        "usage": { "prompt_tokens": 200, "completion_tokens": 20 }
    });

    let server = MockModelServer::start(vec![resp1, resp2, resp3, resp4]).await?;

    let session_id = format!("medium_repo_session_{}", uuid::Uuid::new_v4());
    let config = AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: session_id.clone(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "test_key".to_string(),
            base_url: Some(server.base_url()),
            temperature: Some(0.0),
            max_output_tokens: Some(4096),
        },
        // Custom token ceiling at 500 tokens (effective threshold 425 tokens) to trigger compaction
        model_tier: ModelTier::Custom(500),
        max_tool_iterations: 10,
        auto_compaction: true,
        mcp_config: None,
        auto_approve_risky: true,
    };

    let agent = AgendexAgent::init(config.clone()).await?;

    // Execute multi-turn goal
    let reply = agent
        .process_turn("Locate the auth module and patch token verification")
        .await?;
    assert!(reply.contains("Security patch applied to verify_token"));

    // 1. Verify file was surgically patched in working tree
    let auth_content = tokio::fs::read_to_string(test_dir.join("src/auth.rs")).await?;
    assert!(auth_content.contains("patched_secure_token"));

    // 2. Verify state storage location is .git/agendex (OUTSIDE working tree)
    let state_dir = test_dir.join(".git").join("agendex");
    assert!(state_dir.exists(), ".git/agendex must exist");
    assert!(
        state_dir.join("snapshots.json").exists(),
        "snapshots.json must exist in .git/agendex"
    );

    let log_dir = state_dir.join("logs").join(&session_id);
    assert!(
        log_dir.join("trajectory.jsonl").exists(),
        "trajectory.jsonl must exist in .git/agendex/logs"
    );

    // 3. Verify context compaction was triggered and recorded in trajectory
    let compact_lines = tokio::fs::read_to_string(log_dir.join("trajectory.jsonl")).await?;
    assert!(
        compact_lines.contains("COMPACTION"),
        "Trajectory must contain COMPACTION record"
    );
    assert!(
        compact_lines.contains("Task Overview"),
        "Trajectory must contain structured continuation anchor"
    );

    // 4. Verify working tree is NOT polluted with .agendex or .agenda logs
    assert!(
        !test_dir.join(".agendex").exists(),
        "Working tree must not contain .agendex"
    );
    assert!(
        !test_dir.join(".agenda").exists(),
        "Working tree must not contain .agenda"
    );

    // 5. Test /revert rollback to restore repository cleanly
    let revert_reply = agent.process_turn("/revert").await?;
    assert!(revert_reply.contains("Reverted workspace to latest snapshot checkpoint"));

    // Verify src/auth.rs was restored to pre-turn baseline
    let auth_reverted = tokio::fs::read_to_string(test_dir.join("src/auth.rs")).await?;
    assert!(auth_reverted.contains("secret_admin_token"));
    assert!(!auth_reverted.contains("patched_secure_token"));

    // Verify logs and snapshots survived the /revert without damage
    assert!(
        state_dir.join("snapshots.json").exists(),
        "snapshots.json must survive /revert"
    );
    assert!(
        log_dir.join("trajectory.jsonl").exists(),
        "trajectory.jsonl must survive /revert"
    );

    let _ = tokio::fs::remove_dir_all(&test_dir).await;
    Ok(())
}
