use agendex_harness::tasks::{ManageTaskAction, RunCommandResult, TaskManager, TaskStatus};
use agendex_harness::terminal::{TerminalConfig, TerminalManager};
use agendex_harness::tools::{
    ManageTaskArgs, ReadFileArgs, RunCommandArgs, ToolRegistry, WriteFileArgs,
};
use std::sync::Arc;

#[tokio::test]
async fn test_terminal_multiple_sessions_and_drain() {
    let term_mgr = TerminalManager::new();

    let session_1 = term_mgr
        .create_session(TerminalConfig {
            id: Some("session-alpha".to_string()),
            cols: 80,
            rows: 24,
            ..Default::default()
        })
        .await
        .unwrap();

    let session_2 = term_mgr
        .create_session(TerminalConfig {
            id: Some("session-beta".to_string()),
            cols: 120,
            rows: 40,
            ..Default::default()
        })
        .await
        .unwrap();

    let list = term_mgr.list_sessions().await;
    assert_eq!(list.len(), 2);

    #[cfg(target_os = "windows")]
    term_mgr
        .write_str(&session_1, "echo SESSION_1_READY\r\n")
        .await
        .unwrap();
    #[cfg(not(target_os = "windows"))]
    term_mgr
        .write_str(&session_1, "echo SESSION_1_READY\n")
        .await
        .unwrap();

    let out1 = term_mgr
        .wait_for_pattern(&session_1, "SESSION_1_READY", 5000)
        .await
        .unwrap();
    assert!(out1.contains("SESSION_1_READY"));

    // Draining session 1 should clear buffer
    let drained = term_mgr.read_drain(&session_1).await.unwrap();
    assert!(drained.contains("SESSION_1_READY"));
    let snapshot_after = term_mgr.read_snapshot(&session_1).await.unwrap();
    assert_eq!(snapshot_after, "");

    term_mgr.kill(&session_1).await.unwrap();
    term_mgr.kill(&session_2).await.unwrap();
}

#[tokio::test]
async fn test_task_failed_exit_code() {
    let tmp = std::env::temp_dir().join(format!(
        "agenda_int_tasks_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let manager = TaskManager::new(&tmp);

    let cmd = "exit 42";

    let res = manager
        .run_command(cmd, 3000, None, false, Some("Failing task"))
        .await
        .unwrap();

    match res {
        RunCommandResult::Synchronous { exit_code, .. } => {
            assert_eq!(exit_code, Some(42));
        }
        RunCommandResult::Async { .. } => panic!("Expected sync completion"),
    }

    let tasks = manager.list_tasks().await;
    assert_eq!(tasks.len(), 1);
    match &tasks[0].status {
        TaskStatus::Failed { exit_code, .. } => {
            assert_eq!(*exit_code, Some(42));
        }
        other => panic!("Expected TaskStatus::Failed, got {:?}", other),
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn test_tools_deep_write_and_tail_management() {
    let tmp = std::env::temp_dir().join(format!(
        "agenda_int_tools_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let task_mgr = Arc::new(TaskManager::new(tmp.join("tasks")));
    let registry = ToolRegistry::new(tmp.clone(), Arc::clone(&task_mgr));

    // Deep write auto-creating nested dirs
    let deep_path = tmp.join("deep").join("nested").join("sub").join("data.txt");
    let write_res = registry
        .write_file(WriteFileArgs {
            path: deep_path.clone(),
            content: "Alpha\nBeta\nGamma\nDelta\n".to_string(),
            overwrite: false,
            append: false,
        })
        .await
        .unwrap();
    assert!(write_res.created);

    // Read with byte_offset
    let read_res = registry
        .read_file(ReadFileArgs {
            path: deep_path,
            start_line: Some(1),
            end_line: Some(4),
            byte_offset: Some(6), // Skip "Alpha\n"
            max_bytes: None,
        })
        .await
        .unwrap();
    assert_eq!(read_res.content, "Beta\nGamma\nDelta");

    // Run command via tool registry and query tail via manage_task
    let run_res = registry
        .run_command(RunCommandArgs {
            command: "echo LOG_LINE_1; echo LOG_LINE_2; echo LOG_LINE_3".to_string(),
            wait_ms: Some(4000),
            cwd: None,
            is_daemon: Some(false),
            summary: Some("Multi-echo".to_string()),
        })
        .await
        .unwrap();

    match run_res {
        RunCommandResult::Synchronous {
            stdout, exit_code, ..
        } => {
            assert_eq!(exit_code, Some(0));
            assert!(stdout.contains("LOG_LINE_1"));
        }
        RunCommandResult::Async { .. } => panic!("Expected sync"),
    }

    let tasks = task_mgr.list_tasks().await;
    assert_eq!(tasks.len(), 1);
    let task_id = &tasks[0].task_id;

    // Test manage_task action "tail"
    let tail_val = registry
        .manage_task(ManageTaskArgs {
            action: ManageTaskAction::Tail,
            task_id: Some(task_id.clone()),
            input: None,
            tail_lines: Some(2),
        })
        .await
        .unwrap();

    assert!(tail_val["lines"].is_array());

    let _ = std::fs::remove_dir_all(&tmp);
}
