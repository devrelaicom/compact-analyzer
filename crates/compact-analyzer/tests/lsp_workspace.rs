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
