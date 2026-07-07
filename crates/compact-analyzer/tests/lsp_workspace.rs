//! Black-box workspace-feature integration tests: real binary, real stdio.
mod support;

use serde_json::json;
use support::{Client, did_open, lsp_position};

#[test]
fn workspace_symbol_finds_indexed_declarations() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.compact"),
        "circuit transferValue(): Field { return 0; }",
    )
    .unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();

    let mut client = Client::start();
    client.initialize_root(&root, json!({}));
    let resp = client.request("workspace/symbol", json!({ "query": "transfer" }));
    let syms = resp["result"].as_array().expect("array result");
    assert!(
        syms.iter().any(|s| s["name"] == "transferValue"),
        "expected transferValue in {syms:?}"
    );
    client.shutdown();
}

#[test]
fn watched_file_creation_updates_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();
    let mut client = Client::start();
    client.initialize_root(
        &root,
        json!({ "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }),
    );

    // Barrier: a served `workspace/symbol` response proves the server has
    // reached its main loop, which only happens after the synchronous
    // startup crawl (and file-watcher registration) complete in `run()`.
    // An empty result here proves the crawl already ran and genuinely found
    // nothing — `added.compact` doesn't exist on disk yet.
    let before = client.request("workspace/symbol", json!({ "query": "addedThing" }));
    let before_syms = before["result"].as_array().expect("array result");
    assert!(
        before_syms.is_empty(),
        "expected no addedThing before file creation, got {before_syms:?}"
    );

    // Create a file after the barrier, then notify the server of the change.
    let added = dir.path().join("added.compact");
    std::fs::write(&added, "circuit addedThing(): Field { return 0; }").unwrap();
    let added_uri = lsp_types::Url::from_file_path(&added).unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": added_uri, "type": 1 }] }), // 1 = CREATED
    );

    // Because the barrier proved the crawl already ran and found nothing,
    // any positive result here is attributable only to the
    // `didChangeWatchedFiles` handler.
    let after = client.request("workspace/symbol", json!({ "query": "addedThing" }));
    let after_syms = after["result"].as_array().unwrap();
    assert!(after_syms.iter().any(|s| s["name"] == "addedThing"));
    client.shutdown();
}

#[test]
fn unresolved_import_publishes_an_error_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();
    let mut client = Client::start();
    client.initialize_root(&root, json!({}));

    let main = dir.path().join("main.compact");
    let uri = lsp_types::Url::from_file_path(&main).unwrap();
    did_open(
        &mut client,
        &uri,
        1,
        "import Missing;\ncircuit m(): Field { return 0; }",
    );

    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    assert!(
        diags.iter().any(|d| d["message"]
            .as_str()
            .unwrap_or("")
            .contains("cannot resolve import")),
        "expected an unresolved-import diagnostic in {diags:?}"
    );
    client.shutdown();
}

#[test]
fn two_open_files_each_receive_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();
    let mut client = Client::start();
    client.initialize_root(&root, json!({}));

    let ua = lsp_types::Url::from_file_path(dir.path().join("a.compact")).unwrap();
    let ub = lsp_types::Url::from_file_path(dir.path().join("b.compact")).unwrap();
    did_open(&mut client, &ua, 1, "ledger x Field;"); // missing colon → parse error
    did_open(&mut client, &ub, 1, "ledger y Field;");

    let mut seen = std::collections::HashSet::new();
    while seen.len() < 2 {
        let params = client.wait_for_notification("textDocument/publishDiagnostics");
        let uri = params["uri"].as_str().unwrap().to_string();
        let has_diags = params["diagnostics"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        if has_diags {
            seen.insert(uri);
        }
    }
    client.shutdown();
}

#[test]
fn references_are_workspace_wide_over_lsp() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Foo.compact"),
        "module Foo { export circuit ff(): Field { return 0; } }",
    )
    .unwrap();
    let main = dir.path().join("main.compact");
    let main_text = "import Foo;\ncircuit m(): Field { return ff() + ff(); }";
    std::fs::write(&main, main_text).unwrap();
    let root = lsp_types::Url::from_file_path(dir.path()).unwrap();

    let mut client = Client::start();
    client.initialize_root(&root, json!({}));
    let uri = lsp_types::Url::from_file_path(&main).unwrap();
    did_open(&mut client, &uri, 1, main_text);

    let (line, col) = lsp_position(main_text, "ff");
    let resp = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col },
            "context": { "includeDeclaration": true }
        }),
    );
    let locs = resp["result"].as_array().expect("array result");
    assert_eq!(locs.len(), 3); // declaration in Foo + two uses in main
    client.shutdown();
}
