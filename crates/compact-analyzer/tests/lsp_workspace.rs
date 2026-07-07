//! Black-box workspace-feature integration tests: real binary, real stdio.
mod support;

use serde_json::json;
use support::Client;

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

    // Create a file after initialize, then notify the server of the change.
    let added = dir.path().join("added.compact");
    std::fs::write(&added, "circuit addedThing(): Field { return 0; }").unwrap();
    let added_uri = lsp_types::Url::from_file_path(&added).unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": added_uri, "type": 1 }] }), // 1 = CREATED
    );

    let resp = client.request("workspace/symbol", json!({ "query": "addedThing" }));
    let syms = resp["result"].as_array().unwrap();
    assert!(syms.iter().any(|s| s["name"] == "addedThing"));
    client.shutdown();
}
