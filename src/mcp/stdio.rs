//! MCP stdio transport: a child process speaking newline-delimited JSON-RPC
//! 2.0 on stdin/stdout (stderr is captured for diagnostics).

use crate::config::McpServerConfig;
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Keep the last N bytes of server stderr for error messages.
const STDERR_TAIL: usize = 2048;

/// One-shot reply channel handed to the reader thread per in-flight request.
type Reply = Sender<std::result::Result<Value, String>>;

pub struct StdioClient {
    child: Child,
    stdin: Box<dyn Write + Send>,
    pending: Arc<Mutex<HashMap<u64, Reply>>>,
    next_id: u64,
    stderr_tail: Arc<Mutex<String>>,
    alive: Arc<std::sync::atomic::AtomicBool>,
}

fn build_command(cfg: &McpServerConfig) -> Command {
    #[cfg(windows)]
    {
        // npx/uvx on Windows are .cmd shims — only a shell finds them.
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&cfg.command);
        c.args(&cfg.args);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new(&cfg.command);
        c.args(&cfg.args);
        c
    }
}

impl StdioClient {
    pub fn spawn(cfg: &McpServerConfig) -> Result<Self> {
        let mut cmd = build_command(cfg);
        for (k, v) in &cfg.env {
            cmd.env(k, McpServerConfig::expand(v));
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().with_context(|| format!("spawning mcp server '{}' ({})", cfg.name, cfg.command))?;
        let stdin = Box::new(child.stdin.take().unwrap());
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let pending: Arc<Mutex<HashMap<u64, Reply>>> = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let stderr_tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

        // Reader: route responses by id; notifications are ignored.
        {
            let pending = pending.clone();
            let alive = alive.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let Ok(msg) = serde_json::from_str::<Value>(trimmed) else { continue };
                    if let Some(id) = msg.get("id").and_then(|i| i.as_u64()) {
                        let tx = pending.lock().unwrap().remove(&id);
                        if let Some(tx) = tx {
                            let out = if let Some(err) = msg.get("error") {
                                Err(format!("{}: {}", err["code"].as_i64().unwrap_or(-1), err["message"].as_str().unwrap_or("json-rpc error")))
                            } else {
                                Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                            };
                            let _ = tx.send(out);
                        }
                    }
                }
                alive.store(false, std::sync::atomic::Ordering::Relaxed);
                // Unblock everyone waiting on this dead pipe.
                let waiters: Vec<Reply> = pending.lock().unwrap().drain().map(|(_, tx)| tx).collect();
                for tx in waiters {
                    let _ = tx.send(Err("transport closed (server process exited)".into()));
                }
            });
        }
        // Stderr drain with a small tail buffer.
        {
            let tail = stderr_tail.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut buf = [0u8; 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut t = tail.lock().unwrap();
                            t.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if t.len() > STDERR_TAIL * 2 {
                                let cut = t.len() - STDERR_TAIL;
                                t.drain(..cut);
                            }
                        }
                    }
                }
            });
        }

        Ok(Self { child, stdin, pending, next_id: 1, stderr_tail, alive })
    }

    fn send_raw(&mut self, msg: &Value) -> Result<()> {
        if !self.alive.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(anyhow!("transport closed (server not running)"));
        }
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).map_err(|e| anyhow!("transport write failed: {e}"))?;
        self.stdin.flush().map_err(|e| anyhow!("transport write failed: {e}"))?;
        Ok(())
    }

    pub fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self.send_raw(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(msg)) => {
                let tail = self.stderr_tail.lock().unwrap().clone();
                if tail.trim().is_empty() {
                    Err(anyhow!("mcp server error: {msg}"))
                } else {
                    Err(anyhow!("mcp server error: {msg} (server stderr: {})", crate::providers::truncate(tail.trim(), 300)))
                }
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("mcp request '{method}' timed out after {}s", timeout.as_secs()))
            }
        }
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send_raw(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    pub fn kill(&mut self) {
        self.alive.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for StdioClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
