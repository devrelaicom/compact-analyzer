mod support;

use serde_json::json;
use support::{Client, did_open, temp_doc};

#[test]
fn semantic_tokens_full_returns_encoded_data() {
    let mut client = Client::start();
    client.initialize();
    let (_dir, uri) = temp_doc();
    did_open(
        &mut client,
        &uri,
        1,
        "export circuit inc(x: Field): Field { return x + 1; }",
    );
    let resp = client.request(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": {"uri": uri} }),
    );
    let data = resp["result"]["data"]
        .as_array()
        .cloned()
        .expect("data array");
    assert!(!data.is_empty());
    assert_eq!(data.len() % 5, 0, "5 ints per token");
    client.shutdown();
}
