mod support;

use serde_json::json;
use support::{Client, did_open, lsp_position, temp_doc};

#[test]
fn completion_offers_ledger_methods_after_dot() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    let src = "export ledger cnt: Counter;\ncircuit f(): [] { cnt. }";
    did_open(&mut client, &uri, 1, src);
    let (line, col) = lsp_position(src, ". }"); // position OF the '.'
    let resp = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": col + 1}, // right after the '.'
        }),
    );
    let items = resp["result"].as_array().cloned().unwrap_or_default();
    let labels: Vec<String> = items
        .iter()
        .filter_map(|i| i["label"].as_str().map(str::to_string))
        .collect();
    assert!(labels.contains(&"increment".to_string()), "{labels:?}");
    assert!(labels.contains(&"resetToDefault".to_string()), "{labels:?}");
    client.shutdown();
}

#[test]
fn completion_offers_keywords_at_statement_start() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    let src = "circuit f(): Field {\n  \n}";
    did_open(&mut client, &uri, 1, src);
    // Blank line inside the body.
    let resp = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": 1, "character": 2},
        }),
    );
    let items = resp["result"].as_array().cloned().unwrap_or_default();
    let labels: Vec<String> = items
        .iter()
        .filter_map(|i| i["label"].as_str().map(str::to_string))
        .collect();
    assert!(labels.contains(&"return".to_string()), "{labels:?}");
    client.shutdown();
}
