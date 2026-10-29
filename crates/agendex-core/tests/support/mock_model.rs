use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// Lightweight mock HTTP server providing scripted OpenAI/Gemini/Anthropic responses
/// and recording all incoming request payloads.
#[allow(dead_code)]
pub struct MockModelServer {
    pub addr: SocketAddr,
    pub responses: Arc<Mutex<Vec<Value>>>,
    pub recorded_requests: Arc<Mutex<Vec<Value>>>,
    _abort_handle: tokio::task::AbortHandle,
}

#[allow(dead_code)]
impl MockModelServer {
    pub async fn start(scripted_responses: Vec<Value>) -> anyhow::Result<Self> {
        Self::start_with_status(200, scripted_responses).await
    }

    pub async fn start_with_status(
        status_code: u16,
        scripted_responses: Vec<Value>,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let responses = Arc::new(Mutex::new(scripted_responses));
        let recorded_requests = Arc::new(Mutex::new(Vec::new()));

        let resp_clone = Arc::clone(&responses);
        let req_clone = Arc::clone(&recorded_requests);

        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => break,
                };

                let resp_arc = Arc::clone(&resp_clone);
                let req_arc = Arc::clone(&req_clone);

                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16384];
                    let mut received = Vec::new();
                    loop {
                        let n = match socket.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        received.extend_from_slice(&buf[..n]);

                        if let Some(pos) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                            let header_str = String::from_utf8_lossy(&received[..pos]);
                            let mut content_len = 0;
                            for line in header_str.lines() {
                                if line.to_lowercase().starts_with("content-length:") {
                                    if let Some(len_str) = line.split(':').nth(1) {
                                        content_len = len_str.trim().parse::<usize>().unwrap_or(0);
                                    }
                                }
                            }
                            if received.len() >= pos + 4 + content_len {
                                let body_bytes = &received[pos + 4..pos + 4 + content_len];
                                if let Ok(val) = serde_json::from_slice::<Value>(body_bytes) {
                                    let mut reqs = req_arc.lock().await;
                                    reqs.push(val);
                                }
                                break;
                            }
                        }
                    }

                    let resp_body = {
                        let mut resps = resp_arc.lock().await;
                        if !resps.is_empty() {
                            resps.remove(0)
                        } else {
                            serde_json::json!({
                                "choices": [{
                                    "message": { "role": "assistant", "content": "Default mock response" }
                                }]
                            })
                        }
                    };

                    let body_str = resp_body.to_string();
                    let status_line = match status_code {
                        401 => "HTTP/1.1 401 Unauthorized",
                        400 => "HTTP/1.1 400 Bad Request",
                        403 => "HTTP/1.1 403 Forbidden",
                        500 => "HTTP/1.1 500 Internal Server Error",
                        _ => "HTTP/1.1 200 OK",
                    };
                    let http_resp = format!(
                        "{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        status_line,
                        body_str.len(),
                        body_str
                    );

                    let _ = socket.write_all(http_resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });

        Ok(Self {
            addr,
            responses,
            recorded_requests,
            _abort_handle: handle.abort_handle(),
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub async fn requests(&self) -> Vec<Value> {
        let reqs = self.recorded_requests.lock().await;
        reqs.clone()
    }
}
