use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, RwLock};

/// Configuration for creating a persistent terminal session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    pub id: Option<String>,
    pub shell: Option<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    pub buffer_capacity: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            id: None,
            shell: None,
            cwd: None,
            env: HashMap::new(),
            cols: 120,
            rows: 30,
            buffer_capacity: 256 * 1024, // 256 KB ring buffer
        }
    }
}

/// Metadata and state snapshot of a terminal session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSessionInfo {
    pub id: String,
    pub shell: String,
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
    pub pid: Option<u32>,
    pub is_alive: bool,
    pub created_at: DateTime<Utc>,
    pub total_bytes_received: usize,
}

/// Circular buffer for capturing terminal output without blocking the PTY.
#[derive(Debug)]
pub struct TerminalOutputBuffer {
    buffer: VecDeque<u8>,
    capacity: usize,
    total_bytes_received: usize,
}

impl TerminalOutputBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity.min(64 * 1024)),
            capacity,
            total_bytes_received: 0,
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.total_bytes_received += data.len();
        for &byte in data {
            if self.buffer.len() >= self.capacity {
                self.buffer.pop_front();
            }
            self.buffer.push_back(byte);
        }
    }

    pub fn drain(&mut self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.buffer.len());
        out.extend(self.buffer.drain(..));
        out
    }

    pub fn drain_string(&mut self) -> String {
        let bytes = self.drain();
        String::from_utf8_lossy(&bytes).to_string()
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.buffer.iter().copied().collect()
    }

    pub fn snapshot_string(&self) -> String {
        let bytes = self.snapshot();
        String::from_utf8_lossy(&bytes).to_string()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes_received
    }
}

/// A persistent terminal session backed by ConPTY (Windows) or PTY (Unix).
pub struct TerminalSession {
    pub id: String,
    pub shell: String,
    pub cwd: Option<PathBuf>,
    pub created_at: DateTime<Utc>,
    cols: Mutex<u16>,
    rows: Mutex<u16>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    buffer: Arc<Mutex<TerminalOutputBuffer>>,
    notify: Arc<Notify>,
    is_alive: Arc<AtomicBool>,
}

impl TerminalSession {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.lock().ok().and_then(|c| c.process_id())
    }

    pub fn is_alive(&self) -> bool {
        if !self.is_alive.load(Ordering::SeqCst) {
            return false;
        }
        if let Ok(mut child) = self.child.lock() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.is_alive.store(false, Ordering::SeqCst);
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    self.is_alive.store(false, Ordering::SeqCst);
                    false
                }
            }
        } else {
            false
        }
    }

    pub fn write_bytes(&self, data: &[u8]) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("Failed to acquire terminal writer lock"))?;
        writer
            .write_all(data)
            .context("Failed to write to terminal")?;
        writer.flush().context("Failed to flush terminal writer")?;
        Ok(())
    }

    pub fn write_str(&self, text: &str) -> Result<()> {
        self.write_bytes(text.as_bytes())
    }

    pub fn read_drain(&self) -> Result<String> {
        let mut buf = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("Failed to acquire output buffer lock"))?;
        Ok(buf.drain_string())
    }

    pub fn read_snapshot(&self) -> Result<String> {
        let buf = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("Failed to acquire output buffer lock"))?;
        Ok(buf.snapshot_string())
    }

    pub async fn wait_for_pattern(&self, pattern: &str, timeout: Duration) -> Result<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let current = self.read_snapshot()?;
            if current.contains(pattern) {
                return Ok(current);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "Timed out waiting for pattern '{}' in terminal '{}'. Snapshot:\n{}",
                    pattern,
                    self.id,
                    current
                ));
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            tokio::select! {
                _ = self.notify.notified() => {},
                _ = tokio::time::sleep(remaining.min(Duration::from_millis(50))) => {},
            }
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let master = self
            .master
            .lock()
            .map_err(|_| anyhow!("Failed to acquire terminal master lock"))?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")?;
        if let Ok(mut c) = self.cols.lock() {
            *c = cols;
        }
        if let Ok(mut r) = self.rows.lock() {
            *r = rows;
        }
        Ok(())
    }

    pub fn kill(&self) -> Result<()> {
        self.is_alive.store(false, Ordering::SeqCst);
        let mut child = self
            .child
            .lock()
            .map_err(|_| anyhow!("Failed to acquire terminal child lock"))?;
        let _ = child.kill();
        Ok(())
    }

    pub fn info(&self) -> TerminalSessionInfo {
        let cols = self.cols.lock().map(|c| *c).unwrap_or(80);
        let rows = self.rows.lock().map(|r| *r).unwrap_or(24);
        let total_bytes = self.buffer.lock().map(|b| b.total_bytes()).unwrap_or(0);
        TerminalSessionInfo {
            id: self.id.clone(),
            shell: self.shell.clone(),
            cwd: self.cwd.clone(),
            cols,
            rows,
            pid: self.pid(),
            is_alive: self.is_alive(),
            created_at: self.created_at,
            total_bytes_received: total_bytes,
        }
    }
}

/// Central manager orchestrating persistent terminal sessions across agent turns.
pub struct TerminalManager {
    sessions: Arc<RwLock<HashMap<String, Arc<TerminalSession>>>>,
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create_session(&self, config: TerminalConfig) -> Result<String> {
        let session_id = config
            .id
            .unwrap_or_else(|| format!("term-{}", uuid::Uuid::new_v4().simple()));

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: config.rows,
                cols: config.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open native PTY pair")?;

        let shell_cmd = if let Some(s) = config.shell {
            s
        } else {
            #[cfg(target_os = "windows")]
            {
                std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
            }
            #[cfg(not(target_os = "windows"))]
            {
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
            }
        };

        let mut cmd = CommandBuilder::new(&shell_cmd);
        if let Some(ref cwd) = config.cwd {
            cmd.cwd(cwd);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("Failed to spawn shell '{}' in PTY slave", shell_cmd))?;
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .context("Failed to take PTY master writer")?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY master reader")?;

        let buffer = Arc::new(Mutex::new(TerminalOutputBuffer::new(
            config.buffer_capacity,
        )));
        let notify = Arc::new(Notify::new());
        let is_alive = Arc::new(AtomicBool::new(true));

        // Background reader thread to continuously drain PTY output into ring buffer
        {
            let buffer_clone = Arc::clone(&buffer);
            let notify_clone = Arc::clone(&notify);
            let is_alive_clone = Arc::clone(&is_alive);

            std::thread::Builder::new()
                .name(format!("pty-reader-{}", session_id))
                .spawn(move || {
                    let mut chunk = [0u8; 4096];
                    loop {
                        match reader.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => {
                                if let Ok(mut b) = buffer_clone.lock() {
                                    b.push(&chunk[..n]);
                                }
                                notify_clone.notify_waiters();
                            }
                            Err(_) => break,
                        }
                    }
                    is_alive_clone.store(false, Ordering::SeqCst);
                    notify_clone.notify_waiters();
                })
                .context("Failed to spawn PTY reader thread")?;
        }

        let session = Arc::new(TerminalSession {
            id: session_id.clone(),
            shell: shell_cmd,
            cwd: config.cwd,
            created_at: Utc::now(),
            cols: Mutex::new(config.cols),
            rows: Mutex::new(config.rows),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            buffer,
            notify,
            is_alive,
        });

        {
            let mut guard = self.sessions.write().await;
            guard.insert(session_id.clone(), session);
        }

        tracing::info!("Created persistent terminal session: {}", session_id);
        Ok(session_id)
    }

    pub async fn get_session(&self, id: &str) -> Result<Arc<TerminalSession>> {
        let guard = self.sessions.read().await;
        guard
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("Terminal session '{}' not found", id))
    }

    pub async fn write(&self, id: &str, data: &[u8]) -> Result<()> {
        let session = self.get_session(id).await?;
        session.write_bytes(data)
    }

    pub async fn write_str(&self, id: &str, text: &str) -> Result<()> {
        let session = self.get_session(id).await?;
        session.write_str(text)
    }

    pub async fn read_drain(&self, id: &str) -> Result<String> {
        let session = self.get_session(id).await?;
        session.read_drain()
    }

    pub async fn read_snapshot(&self, id: &str) -> Result<String> {
        let session = self.get_session(id).await?;
        session.read_snapshot()
    }

    pub async fn wait_for_pattern(
        &self,
        id: &str,
        pattern: &str,
        timeout_ms: u64,
    ) -> Result<String> {
        let session = self.get_session(id).await?;
        session
            .wait_for_pattern(pattern, Duration::from_millis(timeout_ms))
            .await
    }

    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        let session = self.get_session(id).await?;
        session.resize(cols, rows)
    }

    pub async fn kill(&self, id: &str) -> Result<()> {
        let session = self.get_session(id).await?;
        session.kill()
    }

    pub async fn list_sessions(&self) -> Vec<TerminalSessionInfo> {
        let guard = self.sessions.read().await;
        guard.values().map(|s| s.info()).collect()
    }

    pub async fn is_alive(&self, id: &str) -> Result<bool> {
        let session = self.get_session(id).await?;
        Ok(session.is_alive())
    }

    pub async fn remove_session(&self, id: &str) -> Result<()> {
        let session = {
            let mut guard = self.sessions.write().await;
            guard.remove(id)
        };
        if let Some(s) = session {
            let _ = s.kill();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_terminal_manager_lifecycle() {
        let manager = TerminalManager::new();
        let session_id = manager
            .create_session(TerminalConfig {
                cols: 80,
                rows: 24,
                ..Default::default()
            })
            .await
            .expect("Failed to create terminal session");

        assert!(manager.is_alive(&session_id).await.unwrap());

        // Send a simple echo command followed by newline
        #[cfg(target_os = "windows")]
        let cmd = "echo HELLO_AGENDA_TERMINAL\r\n";
        #[cfg(not(target_os = "windows"))]
        let cmd = "echo HELLO_AGENDA_TERMINAL\n";

        manager.write_str(&session_id, cmd).await.unwrap();

        // Wait for output pattern
        let output = manager
            .wait_for_pattern(&session_id, "HELLO_AGENDA_TERMINAL", 5000)
            .await
            .expect("Did not see expected pattern");

        assert!(output.contains("HELLO_AGENDA_TERMINAL"));

        // Resize terminal
        manager.resize(&session_id, 100, 40).await.unwrap();

        // List sessions
        let list = manager.list_sessions().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, session_id);
        assert_eq!(list[0].cols, 100);
        assert_eq!(list[0].rows, 40);

        // Kill session
        manager.kill(&session_id).await.unwrap();
        manager.remove_session(&session_id).await.unwrap();
        assert_eq!(manager.list_sessions().await.len(), 0);
    }

    #[test]
    fn test_output_buffer_ring_capacity() {
        let mut buf = TerminalOutputBuffer::new(10);
        buf.push(b"12345");
        assert_eq!(buf.snapshot_string(), "12345");
        assert_eq!(buf.total_bytes(), 5);

        buf.push(b"67890");
        assert_eq!(buf.snapshot_string(), "1234567890");
        assert_eq!(buf.total_bytes(), 10);

        // Overflow: pushing 3 more bytes should drop '123'
        buf.push(b"ABC");
        assert_eq!(buf.snapshot_string(), "4567890ABC");
        assert_eq!(buf.total_bytes(), 13);

        // Drain
        let drained = buf.drain_string();
        assert_eq!(drained, "4567890ABC");
        assert_eq!(buf.snapshot_string(), "");
    }
}
