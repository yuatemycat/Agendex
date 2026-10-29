use agendex_core::agent::{AgendexAgent, AgentConfig};
use std::sync::{Arc, Mutex};
use tokio::process::Command;

#[tokio::test]
async fn test_headless_json_event_stream_and_git_commit() {
    let tmp = std::env::temp_dir().join(format!(
        "agendex_json_git_test_{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&tmp).unwrap();

    // 1. Initialize git repo in workspace
    let init_st = Command::new("git")
        .arg("init")
        .current_dir(&tmp)
        .status()
        .await
        .unwrap();
    assert!(init_st.success());

    let config = AgentConfig {
        workspace_dir: tmp.clone(),
        session_id: "test-session-json-git".to_string(),
        auto_approve_risky: true,
        ..Default::default()
    };

    let agent = AgendexAgent::init(config).await.unwrap();

    // Set up captured JSON lines
    let captured_events = Arc::new(Mutex::new(Vec::<String>::new()));
    let events_clone = Arc::clone(&captured_events);

    let agent = agent.with_event_handler(Arc::new(move |evt| {
        if let Ok(line) = serde_json::to_string(evt) {
            events_clone.lock().unwrap().push(line);
        }
    }));

    // 2. Test Git branch helper
    let br_result = agent
        .switch_or_create_branch("feature/auto-branch")
        .await
        .unwrap();
    assert!(br_result.contains("feature/auto-branch"));

    // Check that GitBranch event was emitted
    {
        let lines = captured_events.lock().unwrap();
        assert!(lines
            .iter()
            .any(|l| l.contains(r#""event":"git_branch""#) && l.contains("feature/auto-branch")));
    }

    // 3. Process turn (slash command /help or /def)
    let reply = agent.process_turn("/def Query").await.unwrap();
    assert!(reply.contains("No definitions found") || reply.contains("Found"));

    // 4. Modify a file in workspace and test automated git commit
    std::fs::write(tmp.join("solution.rs"), "pub fn solution() -> i32 { 42 }\n").unwrap();
    let commit_res = agent
        .commit_workspace_changes("feat: add solution")
        .await
        .unwrap();
    assert!(commit_res.is_some());
    let commit = commit_res.unwrap();
    assert!(!commit.commit_sha.is_empty());
    assert_eq!(commit.files_changed, vec!["solution.rs".to_string()]);

    // Check that TurnStart and GitCommit events were emitted in JSON format
    {
        let lines = captured_events.lock().unwrap();
        assert!(lines.iter().any(|l| l.contains(r#""event":"turn_start""#)));
        assert!(lines
            .iter()
            .any(|l| l.contains(r#""event":"git_commit""#) && l.contains(&commit.commit_sha)));

        // Verify all lines are valid JSON
        for line in lines.iter() {
            let parsed_json: serde_json::Value =
                serde_json::from_str(line).expect("Must be valid JSON");
            assert!(parsed_json.get("event").is_some());
        }
    }

    let _ = std::fs::remove_dir_all(&tmp);
}
