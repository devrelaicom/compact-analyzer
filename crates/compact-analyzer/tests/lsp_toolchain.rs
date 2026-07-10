//! Black-box tests for M4 toolchain integration: compile-on-save (`didSave` →
//! real `compact` compiler → merged, tagged diagnostics) and
//! `textDocument/formatting` (→ real `compact format`). The compiler-driving
//! tests are **gated**: they are a no-op unless a real `compact` toolchain is
//! discoverable, because they drive the actual compiler as their oracle.

mod support;

use serde_json::{Value, json};
use support::{Client, did_open, temp_doc};

/// A self-contained contract whose ONLY error is a compiler-only semantic
/// error (`incremen` is a one-letter typo of the `Counter` ADT's `increment`
/// method). The native analyzer parses this file cleanly and finds no
/// import/include problems, so it emits ZERO native diagnostics — which is the
/// point: the diagnostic under test can only come from the real compiler, and
/// nothing native coincides with it to dedup it away.
///
/// Empirically verified against `compact` 0.31.1 / language 0.23.0
/// (2026-07-10): `compact compile --skip-zk --vscode` reports
/// `Exception: <file> line 8 char 8: operation incremen undefined for ledger
/// field type Counter` (exit 255). Line 8 (1-based) == LSP line 7 (0-based).
const COMPILER_ONLY_ERROR: &str = "\
pragma language_version >= 0.16;

import CompactStandardLibrary;

export ledger round: Counter;

export circuit bump(): [] {
  round.incremen(1);
}
";

/// LSP (0-based) line of the compiler-only error above.
const COMPILER_ERROR_LSP_LINE: i64 = 7;

fn diagnostics(params: &Value) -> &Vec<Value> {
    params["diagnostics"].as_array().unwrap()
}

#[test]
fn compile_on_save_publishes_merged_compactc_diagnostic() {
    if analyzer_toolchain::Toolchain::discover(None).is_none() {
        eprintln!("compact toolchain not present; skipping gated compile-on-save test");
        return;
    }

    // The file must exist on disk: didSave means the on-disk contents are
    // compiled, so we write the fixture to the doc's real path.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.compact");
    std::fs::write(&path, COMPILER_ONLY_ERROR).unwrap();
    let uri = lsp_types::Url::from_file_path(&path).unwrap();

    let mut client = Client::start();
    client.initialize_with_options(json!({ "compileOnSave": true }));

    did_open(&mut client, &uri, 1, COMPILER_ONLY_ERROR);
    client.notify(
        "textDocument/didSave",
        json!({ "textDocument": { "uri": uri } }),
    );

    // The didOpen debounce may publish an (empty) native set first; keep
    // reading publishes until the compiler-sourced diagnostic arrives.
    let mut found = None;
    for _ in 0..10 {
        let params = client.wait_for_notification("textDocument/publishDiagnostics");
        assert_eq!(params["uri"], json!(uri));
        if let Some(d) = diagnostics(&params)
            .iter()
            .find(|d| d["source"] == "compactc")
            .cloned()
        {
            found = Some(d);
            break;
        }
    }

    let diag = found.expect("expected a compactc-sourced diagnostic after didSave");
    assert_eq!(diag["source"], "compactc");
    assert_eq!(diag["severity"], 1, "compiler diagnostics are errors");
    assert_eq!(
        diag["range"]["start"]["line"],
        json!(COMPILER_ERROR_LSP_LINE),
        "compiler diagnostic should land on the offending line"
    );
    assert!(
        diag["message"].as_str().unwrap().contains("incremen"),
        "diagnostic should carry the compiler's message, got: {}",
        diag["message"]
    );

    client.shutdown();
}

/// DoD (OQ2 FAST-EXIT): shutdown must complete PROMPTLY even with a compile
/// literally in flight against a hung/adversarial compiler. A `compact` shim
/// answers the version probes `Toolchain::discover` makes at initialize (so the
/// server accepts it as the toolchain) but SLEEPS on the real compile
/// invocation — a stand-in for a compiler that never returns. Its 8s sleep
/// exceeds this test's 5s patience; absent the cancel-kill, the in-flight
/// compile would only end at `min(8s shim sleep, 30s COMPILE_TIMEOUT) = 8s`,
/// still overshooting the 5s bound, so a pass still proves the in-flight
/// child was actively killed rather than merely outrun by a short sleep. The
/// mechanism (Task 6b): the worker threads `GlobalState.shutdown` into
/// `compile_file` as a cancel token, the poll loop observes it within a tick,
/// kills the child's process group, and drops its `sender` so `io_threads.join()`
/// returns. Without the kill, `io_threads.join()` would block ~8s and
/// `wait_with_timeout(5s)` would fail. `#[cfg(unix)]` (shell shim + chmod).
#[cfg(unix)]
#[test]
fn shutdown_is_prompt_with_a_hung_compile_in_flight() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let shim_dir = tempfile::tempdir().unwrap();
    let shim_path = shim_dir.path().join("compact");
    let script = concat!(
        "#!/bin/sh\n",
        "if [ \"$1\" = \"compile\" ] && [ \"$2\" = \"--version\" ]; then echo 9.9.9-shim; exit 0; fi\n",
        "if [ \"$1\" = \"compile\" ] && [ \"$2\" = \"--language-version\" ]; then echo 0.0.0-shim; exit 0; fi\n",
        "sleep 8\n",
    );
    std::fs::write(&shim_path, script).unwrap();
    std::fs::set_permissions(&shim_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut client = Client::start();
    client.initialize_with_options(json!({
        "toolchainPath": shim_dir.path().to_str().unwrap(),
        "compileOnSave": true,
    }));

    // didSave compiles the ON-DISK file, so the fixture must exist on disk; its
    // contents are irrelevant (the shim sleeps regardless). Reuse the
    // compiler-only-error fixture shape already used by the gated tests.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hang.compact");
    std::fs::write(&path, COMPILER_ONLY_ERROR).unwrap();
    let uri = lsp_types::Url::from_file_path(&path).unwrap();

    did_open(&mut client, &uri, 1, COMPILER_ONLY_ERROR);
    client.notify(
        "textDocument/didSave",
        json!({ "textDocument": { "uri": uri } }),
    );

    // Immediately shut down while the compile worker is blocked in the sleeping
    // shim. `request("shutdown")` returns once the server has replied (it then
    // blocks for `exit`, which we send next). Time the whole handshake +
    // teardown + join.
    let started = Instant::now();
    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    // `wait_with_timeout` panics (failing the test) if the server hasn't exited
    // within 5s — the actual DoD bound. Without the cancel-kill it would hang
    // ~8s here (min of the shim's 8s sleep and the 30s COMPILE_TIMEOUT).
    let status = client.wait_with_timeout(Duration::from_secs(5));
    let elapsed = started.elapsed();

    assert!(
        status.success(),
        "server exited with failure during shutdown-with-compile-in-flight: {status:?}"
    );
    eprintln!("shutdown-with-hung-compile-in-flight returned in {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(5),
        "shutdown took {elapsed:?}; a killed in-flight compile should exit in well under the \
         5s bound (and far under the shim's 8s sleep / the 30s COMPILE_TIMEOUT)"
    );
}

/// A syntax-only file that is opened but never saved must publish only
/// native (`compact-analyzer`) diagnostics — the compiler is invoked on save,
/// not on open, so nothing sourced `compactc` may appear. This holds
/// regardless of whether a toolchain is installed, so it is ungated.
#[test]
fn open_without_save_publishes_only_native_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let uri = lsp_types::Url::from_file_path(dir.path().join("syntax.compact")).unwrap();

    let mut client = Client::start();
    client.initialize_with_options(json!({ "compileOnSave": true }));

    // Missing colon → native "expected COLON" (E0001) at column 12.
    did_open(&mut client, &uri, 1, "ledger count Field;");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = diagnostics(&params);
    assert!(!diags.is_empty(), "expected at least one native diagnostic");
    for d in diags {
        assert_eq!(
            d["source"], "compact-analyzer",
            "no compiler diagnostics without a save: {d}"
        );
    }

    client.shutdown();
}

/// The server advertises `save` support in its sync capability (T4), so the
/// client actually sends `didSave`.
#[test]
fn advertises_text_document_save_capability() {
    let mut client = Client::start();
    let caps = client.initialize_with_options(json!({}));
    let sync = &caps["textDocumentSync"];
    // Options form: `{ "openClose": true, "change": 1, "save": true }`.
    assert_eq!(sync["openClose"], json!(true));
    assert_eq!(sync["change"], json!(1)); // TextDocumentSyncKind::FULL
    assert_eq!(sync["save"], json!(true));
    client.shutdown();
}

// --- textDocument/formatting (Task 7) ---------------------------------

/// A syntactically valid but unformatted buffer (extra parens/whitespace).
/// Empirically verified against real `compact 0.5.1` (2026-07-10):
/// `compact format` rewrites this to exactly `FORMATTED_FIXTURE` below,
/// exits 0, and is idempotent — the same fixture and behavior documented in
/// `analyzer-toolchain`'s own `format_source` unit test
/// (crates/analyzer-toolchain/src/format.rs).
const UNFORMATTED_FIXTURE: &str = "pragma language_version >= 0.16;\n\n\
    export circuit foo(   ): Field {\n  return    1;\n}\n";

/// The exact byte-for-byte output `compact format` produces for
/// `UNFORMATTED_FIXTURE` (captured empirically, see doc comment above).
const FORMATTED_FIXTURE: &str =
    "pragma language_version >= 0.16;\n\nexport circuit foo(): Field {\n  return 1;\n}\n";

/// Syntactically broken (unclosed paren before `: Field`). Empirically
/// verified: `compact format` exits non-zero, writes `"<path>: failed"` to
/// stderr, and leaves the file untouched.
const BROKEN_FIXTURE: &str = "pragma language_version >= 0.16;\n\n\
    export circuit foo(: Field {\n  return 1;\n}\n";

/// Applies a `textDocument/formatting` JSON-RPC result the way a real LSP
/// client would. The handler only ever returns `null`, `[]`, or a single
/// whole-document-replace edit (never a sequence of smaller edits), so
/// "applying" is just taking that edit's `newText` — but this also
/// independently re-derives the edit's expected range from `original` (rather
/// than trusting the implementation under test) and asserts it matches.
fn apply_formatting_result(original: &str, result: &Value) -> String {
    let edits = match result {
        Value::Null => return original.to_string(),
        Value::Array(edits) => edits,
        other => panic!("expected an array or null formatting result, got {other}"),
    };
    if edits.is_empty() {
        return original.to_string();
    }
    assert_eq!(
        edits.len(),
        1,
        "expected a single whole-document TextEdit, got {edits:?}"
    );
    let edit = &edits[0];
    assert_eq!(
        edit["range"]["start"],
        json!({ "line": 0, "character": 0 }),
        "a whole-document edit must start at the top of the file"
    );
    let lines: Vec<&str> = original.split('\n').collect();
    let expected_end_line = (lines.len() - 1) as u32;
    let expected_end_char = lines.last().unwrap().encode_utf16().count() as u32;
    assert_eq!(
        edit["range"]["end"],
        json!({ "line": expected_end_line, "character": expected_end_char }),
        "a whole-document edit must end at the original text's last line/column"
    );
    edit["newText"].as_str().unwrap().to_string()
}

fn request_formatting(client: &mut Client, uri: &lsp_types::Url) -> Value {
    client.request(
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": 2, "insertSpaces": true },
        }),
    )["result"]
        .clone()
}

#[test]
fn formatting_rewrites_unformatted_buffer_to_match_real_compact_format() {
    if analyzer_toolchain::Toolchain::discover(None).is_none() {
        eprintln!("compact toolchain not present; skipping gated formatting test");
        return;
    }

    let mut client = Client::start();
    client.initialize_with_options(json!({}));
    let (_dir, uri) = temp_doc();
    did_open(&mut client, &uri, 1, UNFORMATTED_FIXTURE);

    let result = request_formatting(&mut client, &uri);
    let applied = apply_formatting_result(UNFORMATTED_FIXTURE, &result);
    assert_eq!(
        applied, FORMATTED_FIXTURE,
        "formatting should produce exactly what the real `compact format` produces"
    );

    client.shutdown();
}

#[test]
fn formatting_already_formatted_buffer_returns_no_edits() {
    if analyzer_toolchain::Toolchain::discover(None).is_none() {
        eprintln!("compact toolchain not present; skipping gated formatting test");
        return;
    }

    let mut client = Client::start();
    client.initialize_with_options(json!({}));
    let (_dir, uri) = temp_doc();
    did_open(&mut client, &uri, 1, FORMATTED_FIXTURE);

    let result = request_formatting(&mut client, &uri);
    let applied = apply_formatting_result(FORMATTED_FIXTURE, &result);
    assert_eq!(
        applied, FORMATTED_FIXTURE,
        "an already-formatted buffer should be a no-op"
    );

    client.shutdown();
}

#[test]
fn formatting_broken_buffer_returns_no_edits_without_error() {
    if analyzer_toolchain::Toolchain::discover(None).is_none() {
        eprintln!("compact toolchain not present; skipping gated formatting test");
        return;
    }

    let mut client = Client::start();
    client.initialize_with_options(json!({}));
    let (_dir, uri) = temp_doc();
    did_open(&mut client, &uri, 1, BROKEN_FIXTURE);

    let response = client.request(
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": 2, "insertSpaces": true },
        }),
    );
    assert!(
        response.get("error").is_none(),
        "formatting must not error on a broken buffer: {response}"
    );
    let applied = apply_formatting_result(BROKEN_FIXTURE, &response["result"]);
    assert_eq!(
        applied, BROKEN_FIXTURE,
        "a syntactically broken buffer should yield no edits"
    );

    client.shutdown();
}

/// The server advertises `documentFormattingProvider` (T7) so the client
/// knows to send `textDocument/formatting` requests. Ungated: capability
/// advertisement doesn't depend on a real toolchain being present.
#[test]
fn advertises_document_formatting_capability() {
    let mut client = Client::start();
    let caps = client.initialize_with_options(json!({}));
    assert_eq!(caps["documentFormattingProvider"], json!(true));
    client.shutdown();
}

/// Toolchain optionality (hard invariant): with the `formatting` toggle off,
/// formatting must be a clean `[]` — never an error, regardless of whether a
/// real toolchain happens to be installed. Ungated for that reason.
#[test]
fn formatting_toggle_off_returns_empty_edits() {
    let mut client = Client::start();
    client.initialize_with_options(json!({ "formatting": false }));
    let (_dir, uri) = temp_doc();
    did_open(&mut client, &uri, 1, UNFORMATTED_FIXTURE);

    let result = request_formatting(&mut client, &uri);
    assert_eq!(
        result,
        json!([]),
        "formatting toggle off must return a clean empty edit list"
    );

    client.shutdown();
}

// --- One-time toolchain-missing notice (Task 9) ------------------------

/// Sends a `textDocument/formatting` request with a caller-chosen id (NOT
/// `Client::request`, which silently discards any notification it happens to
/// read while scanning for the matching response id — exactly the messages
/// this test needs to inspect) and returns `(response, notifications_seen)`:
/// every notification observed while waiting for the response, in order.
///
/// Because the server's main loop is single-threaded and strictly FIFO, this
/// also captures whatever an *earlier* queued notification (e.g. a preceding
/// `didSave`) produced — that notification is fully handled, and anything it
/// sends is already in the transport, before the server even looks at this
/// request.
fn request_formatting_capturing_notifications(
    client: &mut Client,
    uri: &lsp_types::Url,
) -> (Value, Vec<Value>) {
    const ID: i64 = 9_999_001; // far outside Client's own auto-incrementing id space
    client.send(json!({
        "jsonrpc": "2.0",
        "id": ID,
        "method": "textDocument/formatting",
        "params": {
            "textDocument": { "uri": uri },
            "options": { "tabSize": 2, "insertSpaces": true },
        },
    }));
    let mut notifications = Vec::new();
    loop {
        let msg = client.recv();
        if msg.get("id").and_then(Value::as_i64) == Some(ID) {
            return (msg, notifications);
        }
        if msg.get("method").is_some() {
            notifications.push(msg);
        }
    }
}

/// Never-die / optionality (hard invariant): when a toolchain-requiring
/// feature is WANTED (its toggle on) but no `compact` toolchain is present,
/// the server sends exactly ONE `window/showMessage` (INFO) for the whole
/// session — not one per feature, not one per use — and every feature use
/// still no-ops cleanly (no error response, no diagnostics storm).
///
/// The toolchain is hidden deterministically by scrubbing the child's `PATH`
/// (via `Client::start_with_env`), independent of whether the host actually
/// has `compact` installed — `Toolchain::discover(None)` only scans `PATH`.
/// Ungated: this test needs the toolchain ABSENT, so it runs unconditionally
/// (including in CI, which has no `compact` on PATH anyway).
#[test]
fn toolchain_missing_notice_fires_once_across_did_save_and_formatting() {
    let empty_path_dir = tempfile::tempdir().unwrap();
    let mut client = Client::start_with_env(&[("PATH", empty_path_dir.path().to_str().unwrap())]);
    client.initialize_with_options(json!({ "compileOnSave": true, "formatting": true }));

    let (_dir, uri) = temp_doc();
    did_open(&mut client, &uri, 1, UNFORMATTED_FIXTURE);

    // First toolchain-requiring feature use: didSave (compile-on-save).
    client.notify(
        "textDocument/didSave",
        json!({ "textDocument": { "uri": uri } }),
    );

    // Second toolchain-requiring feature use: a formatting request. The
    // capturing helper's response wait also nets whatever the preceding
    // didSave notification produced (FIFO, single-threaded dispatch).
    let (response, notifications) = request_formatting_capturing_notifications(&mut client, &uri);

    assert!(
        response.get("error").is_none(),
        "formatting must not error when the toolchain is absent: {response:?}"
    );
    assert_eq!(
        response["result"],
        json!([]),
        "formatting must still return well-formed (empty) edits when the toolchain is absent"
    );

    let show_messages: Vec<&Value> = notifications
        .iter()
        .filter(|n| n["method"] == "window/showMessage")
        .collect();
    assert_eq!(
        show_messages.len(),
        1,
        "expected exactly one window/showMessage across didSave + formatting, got: {notifications:#?}"
    );
    let params = &show_messages[0]["params"];
    assert_eq!(params["type"], json!(3), "MessageType::INFO (3)");
    let message = params["message"].as_str().unwrap();
    assert!(
        message.to_lowercase().contains("compact"),
        "notice should mention the compact toolchain: {message}"
    );

    client.shutdown();
}
