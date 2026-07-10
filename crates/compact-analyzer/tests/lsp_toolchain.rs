//! Black-box tests for M4 compile-on-save (`didSave` → real `compact`
//! compiler → merged, tagged diagnostics). The compile-driving test is
//! **gated**: it is a no-op unless a real `compact` toolchain is discoverable,
//! because it drives the actual compiler as its oracle.

mod support;

use serde_json::{Value, json};
use support::{Client, did_open};

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
