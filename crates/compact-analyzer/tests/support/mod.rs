//! Shared black-box LSP test harness: spawn the real binary, speak the
//! protocol over stdio, assert observable behavior. No internal APIs.
//!
//! Compiled independently by each integration-test binary
//! (`lsp_integration.rs`, `lsp_navigation.rs`), so any helper a given binary
//! doesn't use would otherwise trip clippy's dead-code lint under
//! `-D warnings`.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub struct Client {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next_id: i64,
}

impl Client {
    pub fn start() -> Client {
        Self::start_with_env(&[])
    }

    /// Spawns the server like [`Client::start`], but with `env` overriding
    /// (or adding to) the child's otherwise-inherited environment. Does NOT
    /// clear the inherited environment first — a wholesale `env_clear()`
    /// would drop vars like `HOME`/`TMPDIR` the server may need; only the
    /// listed keys are overridden.
    ///
    /// Used to deterministically hide the `compact` toolchain (override
    /// `PATH` to a directory with no `compact` binary on it) independent of
    /// whether the host actually has one installed — `Toolchain::discover`
    /// only scans `PATH` when no `toolchainPath` override is configured.
    pub fn start_with_env(env: &[(&str, &str)]) -> Client {
        let mut command = Command::new(env!("CARGO_BIN_EXE_compact-analyzer"));
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("failed to spawn compact-analyzer");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, incoming) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(msg) = read_message(&mut reader) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        Client {
            child,
            stdin,
            incoming,
            next_id: 0,
        }
    }

    pub fn initialize(&mut self) {
        let response = self.request("initialize", json!({"capabilities": {}}));
        assert!(
            response.get("result").is_some(),
            "initialize failed: {response}"
        );
        self.notify("initialized", json!({}));
    }

    /// Initialize with an explicit workspace root and client capabilities.
    pub fn initialize_root(&mut self, root: &lsp_types::Url, caps: Value) {
        let response = self.request(
            "initialize",
            json!({ "capabilities": caps, "rootUri": root }),
        );
        assert!(
            response.get("result").is_some(),
            "initialize failed: {response}"
        );
        self.notify("initialized", json!({}));
    }

    /// Initialize with `initializationOptions` (e.g. `{"compileOnSave": true}`)
    /// and default (empty) client capabilities. Returns the `capabilities`
    /// object the server advertised, so a test can additionally assert on it.
    pub fn initialize_with_options(&mut self, options: Value) -> Value {
        let response = self.request(
            "initialize",
            json!({ "capabilities": {}, "initializationOptions": options }),
        );
        assert!(
            response.get("result").is_some(),
            "initialize failed: {response}"
        );
        let capabilities = response["result"]["capabilities"].clone();
        self.notify("initialized", json!({}));
        capabilities
    }

    pub fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let msg = self.recv();
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                return msg;
            }
        }
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn recv(&mut self) -> Value {
        self.incoming
            .recv_timeout(Duration::from_secs(10))
            .expect("timed out waiting for a server message")
    }

    /// Skips unrelated messages until a notification with `method` arrives,
    /// returning its params.
    pub fn wait_for_notification(&mut self, method: &str) -> Value {
        loop {
            let msg = self.recv();
            if msg.get("method").and_then(Value::as_str) == Some(method) {
                return msg["params"].clone();
            }
        }
    }

    pub fn shutdown(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        let status = self.wait_with_timeout(Duration::from_secs(10));
        assert!(status.success(), "server exited with failure: {status:?}");
    }

    /// Waits for the child to exit, killing it and failing the test if it
    /// does not exit within `timeout` (prevents CI hangs on shutdown bugs).
    pub fn wait_with_timeout(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .expect("failed to poll server process")
            {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("server did not exit within {timeout:?} after shutdown/exit");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length: ") {
            content_length = rest.parse().ok();
        }
    }
    let mut buf = vec![0u8; content_length?];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

/// Creates a temp dir + file URI for a document (file need not exist on
/// disk — content arrives via didOpen).
pub fn temp_doc() -> (tempfile::TempDir, lsp_types::Url) {
    let dir = tempfile::tempdir().unwrap();
    let uri = lsp_types::Url::from_file_path(dir.path().join("doc.compact")).unwrap();
    (dir, uri)
}

pub fn did_open(client: &mut Client, uri: &lsp_types::Url, version: i64, text: &str) {
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": uri, "languageId": "compact", "version": version, "text": text,
        }}),
    );
}

/// 0-based (line, UTF-16 column) of the first occurrence of `needle`.
pub fn lsp_position(text: &str, needle: &str) -> (u32, u32) {
    let idx = text.find(needle).expect("needle not in fixture");
    let line = text[..idx].matches('\n').count() as u32;
    let line_start = text[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = text[line_start..idx].encode_utf16().count() as u32;
    (line, col)
}
