//! Black-box LSP tests: spawn the real binary, speak the protocol over
//! stdio, assert observable behavior. No internal APIs.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next_id: i64,
}

impl Client {
    fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_compact-analyzer"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to spawn compact-analyzer");
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

    fn initialize(&mut self) {
        let response = self.request("initialize", json!({"capabilities": {}}));
        assert!(
            response.get("result").is_some(),
            "initialize failed: {response}"
        );
        self.notify("initialized", json!({}));
    }

    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
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

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn recv(&mut self) -> Value {
        self.incoming
            .recv_timeout(Duration::from_secs(10))
            .expect("timed out waiting for a server message")
    }

    /// Skips unrelated messages until a notification with `method` arrives,
    /// returning its params.
    fn wait_for_notification(&mut self, method: &str) -> Value {
        loop {
            let msg = self.recv();
            if msg.get("method").and_then(Value::as_str) == Some(method) {
                return msg["params"].clone();
            }
        }
    }

    fn shutdown(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        let status = self.wait_with_timeout(Duration::from_secs(10));
        assert!(status.success(), "server exited with failure: {status:?}");
    }

    /// Waits for the child to exit, killing it and failing the test if it
    /// does not exit within `timeout` (prevents CI hangs on shutdown bugs).
    fn wait_with_timeout(&mut self, timeout: Duration) -> std::process::ExitStatus {
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

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
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
fn temp_doc() -> (tempfile::TempDir, lsp_types::Url) {
    let dir = tempfile::tempdir().unwrap();
    let uri = lsp_types::Url::from_file_path(dir.path().join("doc.compact")).unwrap();
    (dir, uri)
}

fn did_open(client: &mut Client, uri: &lsp_types::Url, version: i64, text: &str) {
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": uri, "languageId": "compact", "version": version, "text": text,
        }}),
    );
}

#[test]
fn initialize_reports_server_info() {
    let mut client = Client::start();
    let response = client.request("initialize", json!({"capabilities": {}}));
    assert_eq!(response["result"]["serverInfo"]["name"], "compact-analyzer");
    client.notify("initialized", json!({}));
    client.shutdown();
}

#[test]
fn publishes_diagnostics_then_clears_after_fix() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    // Verified fixture: missing colon → "expected COLON" at offset 12
    did_open(&mut client, &uri, 1, "ledger count Field;");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert_eq!(params["uri"], json!(uri));
    assert_eq!(params["version"], 1);
    let diags = params["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["message"], "expected COLON");
    assert_eq!(diags[0]["source"], "compact-analyzer");
    assert_eq!(diags[0]["severity"], 1); // Error
    assert_eq!(diags[0]["code"], "E0001");
    assert_eq!(
        diags[0]["range"]["start"],
        json!({"line": 0, "character": 12})
    );

    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"text": "ledger count: Field;"}],
        }),
    );
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert_eq!(params["version"], 2);
    assert!(params["diagnostics"].as_array().unwrap().is_empty());

    client.shutdown();
}

#[test]
fn positions_are_utf16_code_units() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    // Verified fixture: error at byte offset 23 == UTF-16 column 21
    did_open(&mut client, &uri, 1, "/* \u{1F600} */ ledger count Field;");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0]["range"]["start"],
        json!({"line": 0, "character": 21})
    );

    client.shutdown();
}

#[test]
fn clears_diagnostics_on_close() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    did_open(&mut client, &uri, 1, "@@@");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert!(!params["diagnostics"].as_array().unwrap().is_empty());

    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": uri}}),
    );
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert!(params["diagnostics"].as_array().unwrap().is_empty());

    client.shutdown();
}

#[test]
fn unknown_requests_get_an_error_not_a_crash() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    let response = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 0},
        }),
    );
    assert!(
        response.get("error").is_some(),
        "expected error response: {response}"
    );

    // Server is still alive and functional afterwards
    did_open(&mut client, &uri, 1, "ledger count: Field;");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    assert!(params["diagnostics"].as_array().unwrap().is_empty());

    client.shutdown();
}
