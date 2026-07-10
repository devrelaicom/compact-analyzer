mod support;

use serde_json::json;
use support::{Client, did_open, temp_doc};

#[test]
fn folding_ranges_cover_body_and_imports() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    did_open(
        &mut client,
        &uri,
        1,
        "import CompactStandardLibrary;\nimport Foo;\ncircuit f(): Field {\n  return 0;\n}",
    );
    let resp = client.request(
        "textDocument/foldingRange",
        json!({ "textDocument": {"uri": uri} }),
    );
    let ranges = resp["result"].as_array().cloned().expect("ranges");
    assert!(ranges.iter().any(|r| r["kind"] == "imports"));
    // The circuit body spans multiple lines → a fold with no/region kind.
    assert!(ranges.iter().any(|r| r["startLine"] != r["endLine"]));
    client.shutdown();
}

#[test]
fn selection_range_is_nested() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    let src = "circuit f(): Field { return xyz; }";
    did_open(&mut client, &uri, 1, src);
    // Position on `xyz`.
    let col = src.find("xyz").unwrap() as u32 + 1;
    let resp = client.request(
        "textDocument/selectionRange",
        json!({
            "textDocument": {"uri": uri},
            "positions": [{"line": 0, "character": col}],
        }),
    );
    let sel = &resp["result"][0];
    // Innermost range is the identifier; it has a parent chain.
    assert!(sel["parent"].is_object());
    client.shutdown();
}
