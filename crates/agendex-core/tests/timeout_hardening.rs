use agendex_core::model::{ChatMessage, ModelClient, ModelConfig, ModelFormat};
use std::sync::Mutex;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

static TIMEOUT_ENV_MUTEX: Mutex<()> = Mutex::new(());

#[tokio::test]
async fn test_first_byte_timeout_does_not_retry() -> anyhow::Result<()> {
    let _lock = TIMEOUT_ENV_MUTEX.lock().unwrap();
    std::env::set_var("AGENDEX_REQUEST_TIMEOUT_SECS", "1");

    // Bind a listener that accepts the connection and sleeps (hung provider)
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                // Sleep longer than timeout to simulate hung provider
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
            });
        }
    });

    let config = ModelConfig {
        format: ModelFormat::OpenAi,
        base_url: Some(format!("http://{}", addr)),
        api_key: "sk-test".to_string(),
        model_name: "test-model".to_string(),
        ..Default::default()
    };

    let client = ModelClient::new(config);
    assert_eq!(client.timeout_secs, 1);

    let start = Instant::now();
    let res = client
        .chat(&[ChatMessage::user("Hello hung server")], &[])
        .await;

    let elapsed = start.elapsed();
    assert!(res.is_err(), "Hung server must return an error");
    let err_str = res.unwrap_err().to_string();

    assert!(
        err_str.contains("Request timed out waiting for initial response from provider"),
        "Unexpected error: {}",
        err_str
    );

    // If it had retried 3 times with backoff, it would take >= 3-4 seconds.
    // With immediate fail-fast timeout, it must complete in under 2.5 seconds.
    assert!(
        elapsed.as_millis() < 2500,
        "First-byte timeout should not retry: elapsed was {} ms",
        elapsed.as_millis()
    );

    std::env::remove_var("AGENDEX_REQUEST_TIMEOUT_SECS");
    Ok(())
}

#[tokio::test]
async fn test_mid_stream_stall_honors_request_timeout_setting() -> anyhow::Result<()> {
    let _lock = TIMEOUT_ENV_MUTEX.lock().unwrap();
    std::env::set_var("AGENDEX_REQUEST_TIMEOUT_SECS", "1");

    // Bind a server that sends SSE headers and one chunk, then stalls
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let initial = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n";
                let _ = socket.write_all(initial.as_bytes()).await;
                let _ = socket.flush().await;
                // Stall without sending [DONE] or closing
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            });
        }
    });

    let config = ModelConfig {
        format: ModelFormat::OpenAi,
        base_url: Some(format!("http://{}", addr)),
        api_key: "sk-test".to_string(),
        model_name: "test-model".to_string(),
        ..Default::default()
    };

    let client = ModelClient::new(config);
    assert_eq!(client.timeout_secs, 1);

    let start = Instant::now();
    let res = client
        .chat(&[ChatMessage::user("Hello streaming server")], &[])
        .await;

    let elapsed = start.elapsed();
    assert!(res.is_err(), "Mid-stream stall must return an error");
    let err_str = res.unwrap_err().to_string();

    assert!(
        err_str.contains(
            "Stream read timed out: no response chunk received from model provider for 1 seconds"
        ),
        "Expected 1-second timeout message, got: {}",
        err_str
    );

    // Stalling with timeout_secs=1 must error around 1 second, NOT 30 seconds
    assert!(
        elapsed.as_millis() < 2500,
        "Stream stall must honor setting (1s) instead of hardcoded 30s: elapsed was {} ms",
        elapsed.as_millis()
    );

    std::env::remove_var("AGENDEX_REQUEST_TIMEOUT_SECS");
    Ok(())
}
