//! Black-box LSP tests: spawn the real binary, speak the protocol over
//! stdio, assert observable behavior. No internal APIs.

mod support;

use serde_json::json;
use support::{Client, did_open, temp_doc};

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
        "textDocument/thisMethodDoesNotExist",
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

#[test]
fn publishes_native_type_diagnostic_on_open() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    // Verified type error: `true` is Boolean, not a subtype of the declared
    // Field return -> native emits E3001, source "compact-analyzer".
    did_open(
        &mut client,
        &uri,
        1,
        "export circuit foo(): Field { return true; }",
    );
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d["code"] == "E3001" && d["source"] == "compact-analyzer"),
        "expected an E3001 type diagnostic from compact-analyzer, got {diags:#?}"
    );
    client.shutdown();
}

#[test]
fn type_diagnostics_toggle_off_suppresses_type_but_keeps_parse() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    // Initialize with typeDiagnostics disabled.
    client.initialize_with_options(json!({ "typeDiagnostics": false }));

    // A file with BOTH a parse error and (were it parseable) a type error.
    // The missing colon is a parse error (E0001) that must still surface;
    // type diagnostics are suppressed by the toggle.
    did_open(&mut client, &uri, 1, "ledger count Field;");
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    assert!(
        diags.iter().any(|d| d["source"] == "compact-analyzer"),
        "parse/resolution diagnostics must still publish with the toggle off, got {diags:#?}"
    );
    assert!(
        diags.iter().all(|d| d["code"] != "E3001"),
        "no type diagnostic (E3xxx) should be published with the toggle off, got {diags:#?}"
    );

    // A clean-parsing file whose ONLY error is a type error: with the toggle
    // off it must publish ZERO compact-analyzer type diagnostics.
    let (_dir2, uri2) = temp_doc();
    did_open(
        &mut client,
        &uri2,
        1,
        "export circuit foo(): Field { return true; }",
    );
    // `wait_for_notification` returns the first matching notification
    // regardless of which file it's for; filter on `uri` (unique per
    // `temp_doc`) so a residual publish for `uri` can't be mistaken for
    // `uri2`'s publish.
    let d2 = loop {
        let params2 = client.wait_for_notification("textDocument/publishDiagnostics");
        if params2["uri"] == json!(uri2) {
            break params2["diagnostics"].as_array().unwrap().clone();
        }
    };
    assert!(
        d2.iter().all(|d| d["code"] != "E3001"),
        "type diagnostic must be suppressed with the toggle off, got {d2:#?}"
    );

    client.shutdown();
}
