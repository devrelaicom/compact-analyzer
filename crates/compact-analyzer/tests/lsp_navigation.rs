//! Black-box tests for the navigation features added in M2a.

mod support;

use serde_json::{Value, json};
use support::{Client, did_open, lsp_position, temp_doc};

const NAV_FIXTURE: &str = "\
import CompactStandardLibrary;
struct Point { x: Field; y: Field; }
circuit helper(base: Field): Field { return base; }
export circuit main(input: Field): Bytes<32> {
  const doubled = helper(input);
  return persistentHash<Field>(doubled);
}
";

fn open_fixture(client: &mut Client) -> (tempfile::TempDir, lsp_types::Url) {
    let (dir, uri) = temp_doc();
    did_open(client, &uri, 1, NAV_FIXTURE);
    (dir, uri)
}

fn definition_at(client: &mut Client, uri: &lsp_types::Url, line: u32, character: u32) -> Value {
    let response = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        }),
    );
    response["result"].clone()
}

#[test]
fn definition_of_local_call_and_stdlib() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = open_fixture(&mut client);

    // On `helper` in `helper(input)` (line 4) → the circuit on line 2.
    let (line, col) = lsp_position(NAV_FIXTURE, "helper(input)");
    let result = definition_at(&mut client, &uri, line, col);
    assert_eq!(result["uri"], json!(uri));
    assert_eq!(result["range"]["start"]["line"], 2);

    // On `persistentHash` → a DIFFERENT file (the materialized stdlib stub).
    let (line, col) = lsp_position(NAV_FIXTURE, "persistentHash");
    let result = definition_at(&mut client, &uri, line, col);
    let target_uri = result["uri"].as_str().expect("stdlib definition location");
    assert_ne!(target_uri, uri.as_str());
    assert!(target_uri.ends_with("CompactStandardLibrary.compact"));

    // On whitespace → null.
    let result = definition_at(&mut client, &uri, 0, 0);
    assert!(result.is_null());

    client.shutdown();
}

#[test]
fn references_for_helper() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = open_fixture(&mut client);

    let (line, col) = lsp_position(NAV_FIXTURE, "helper(base");
    let response = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": col},
            "context": {"includeDeclaration": true},
        }),
    );
    let locations = response["result"].as_array().expect("array of locations");
    assert_eq!(locations.len(), 2); // declaration + call in main

    client.shutdown();
}
