//! Black-box LSP tests: spawn the real binary, speak the protocol over
//! stdio, assert observable behavior. No internal APIs.

mod support;

use serde_json::json;
use support::{Client, did_open, lsp_position, temp_doc};

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
    // A parse error also has disclosure results unavailable (v3c C6), so this
    // publishes alongside a paused U3100 advisory — find the parse error
    // itself rather than assume it's the only diagnostic.
    let parse_err = diags
        .iter()
        .find(|d| d["code"] == "E0001")
        .unwrap_or_else(|| panic!("expected an E0001 parse error, got {diags:#?}"));
    assert_eq!(parse_err["message"], "expected COLON");
    assert_eq!(parse_err["source"], "compact-analyzer");
    assert_eq!(parse_err["severity"], 1); // Error
    assert_eq!(
        parse_err["range"]["start"],
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
    // A parse error also has disclosure results unavailable (v3c C6), so this
    // publishes alongside a paused U3100 advisory — find the parse error
    // itself rather than assume it's the only diagnostic.
    let parse_err = diags
        .iter()
        .find(|d| d["code"] == "E0001")
        .unwrap_or_else(|| panic!("expected an E0001 parse error, got {diags:#?}"));
    assert_eq!(
        parse_err["range"]["start"],
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
fn publishes_disclosure_leak_diagnostic_on_open() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    // Verified leak fixture (A4 K1 ledger sink, mirrored from
    // analyzer-core's `ledger_cell_write_records_leak`): `c = getW();`
    // writes a witness value straight to a ledger Cell -> native emits a
    // confirmed E3100, source "compact-analyzer", with a secondary span at
    // the witness origin.
    did_open(
        &mut client,
        &uri,
        1,
        "import CompactStandardLibrary;\n\
         export ledger c: Uint<8>;\n\
         witness getW(): Uint<8>;\n\
         export circuit f(): [] { c = getW(); }\n",
    );
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    let leak = diags
        .iter()
        .find(|d| d["code"] == "E3100")
        .unwrap_or_else(|| panic!("expected an E3100 disclosure leak, got {diags:#?}"));
    assert_eq!(leak["source"], "compact-analyzer");
    assert_eq!(leak["severity"], 1); // Error
    assert!(
        leak["relatedInformation"]
            .as_array()
            .is_some_and(|r| !r.is_empty()),
        "leak must carry witness-origin related-info spans, got {leak:#?}"
    );

    client.shutdown();
}

const LEAK_FIXTURE: &str = "import CompactStandardLibrary;\n\
     export ledger c: Uint<8>;\n\
     witness getW(): Uint<8>;\n\
     export circuit f(): [] { c = getW(); }\n";

/// v3c C3 (security-sensitive): a `textDocument/codeAction` at the
/// confirmed leak's range returns exactly one `quickfix` that wraps the
/// MINIMAL tainted expression (`getW()`) in `disclose(...)` — never the
/// whole `c = getW();` statement. Differential-verified separately (see the
/// task report) that applying this exact edit produces source `compactc`
/// 0.31.1 accepts.
#[test]
fn code_action_offers_minimal_scope_disclose_quickfix() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    did_open(&mut client, &uri, 1, LEAK_FIXTURE);
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    let leak = diags
        .iter()
        .find(|d| d["code"] == "E3100")
        .unwrap_or_else(|| panic!("expected an E3100 disclosure leak, got {diags:#?}"));
    let leak_range = leak["range"].clone();

    let response = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": {"uri": uri},
            "range": leak_range,
            "context": {"diagnostics": [leak]},
        }),
    );
    let actions = response["result"].as_array().expect("actions array");
    assert_eq!(
        actions.len(),
        1,
        "expected exactly one quickfix, got {actions:#?}"
    );
    let action = &actions[0];
    assert_eq!(action["title"], "Reveal this value with disclose()");
    assert_eq!(action["kind"], "quickfix");
    assert_eq!(action["isPreferred"], false);

    let edits = action["edit"]["changes"][uri.as_str()]
        .as_array()
        .expect("edits for the document");
    assert_eq!(edits.len(), 2, "an insert-open + insert-close pair");
    assert_eq!(edits[0]["newText"], "disclose(");
    assert_eq!(edits[1]["newText"], ")");
    assert_eq!(
        edits[0]["range"]["start"], edits[0]["range"]["end"],
        "the open-paren insertion must be zero-width"
    );
    assert_eq!(
        edits[1]["range"]["start"], edits[1]["range"]["end"],
        "the close-paren insertion must be zero-width"
    );

    // The COMBINED range the edit touches (earliest start, latest end)
    // must equal the diagnostic's own range EXACTLY — not the whole
    // statement. This is the security property the quick-fix exists to
    // uphold: a wider wrap could silently sanitize a second, unrelated leak
    // sharing the same statement.
    assert_eq!(edits[0]["range"]["start"], leak_range["start"]);
    assert_eq!(edits[1]["range"]["end"], leak_range["end"]);

    client.shutdown();
}

/// The C1 `disclosureDiagnostics` toggle governs the quick-fix too: with
/// disclosure diagnostics off, no disclosure quick-fix is ever offered
/// (there's no published leak to attach it to, and offering one anyway
/// would contradict the toggle).
#[test]
fn code_action_respects_disclosure_diagnostics_toggle_off() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize_with_options(json!({ "disclosureDiagnostics": false }));

    did_open(&mut client, &uri, 1, LEAK_FIXTURE);
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    assert!(
        diags.iter().all(|d| d["code"] != "E3100"),
        "toggle off: no E3100 should publish, got {diags:#?}"
    );

    // Request a code action over the (unpublished) leak's source range.
    let (line, col) = lsp_position(LEAK_FIXTURE, "getW(); }");
    let response = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": line, "character": col},
                "end": {"line": line, "character": col + 6},
            },
            "context": {"diagnostics": []},
        }),
    );
    let actions = response["result"].as_array().expect("actions array");
    assert!(
        actions.is_empty(),
        "no disclosure quickfix with the toggle off, got {actions:#?}"
    );

    client.shutdown();
}

#[test]
fn publishes_disclosure_advisory_diagnostic_on_open() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    client.initialize();

    // Verified fail-closed fixture (A8, mirrored from analyzer-core's
    // `module_nested_circuit_renders_amber_advisory_not_a_leak`): a
    // module-nested `export circuit` is excluded from the root set, so the
    // analyzer can't decide it and records an amber U3100 advisory rather
    // than silently reporting no leak.
    did_open(
        &mut client,
        &uri,
        1,
        "import CompactStandardLibrary;\n\
         module M {\n\
         export ledger c: Field;\n\
         export circuit leak(x: Field): Field { c = x; return x; }\n\
         }\n",
    );
    let params = client.wait_for_notification("textDocument/publishDiagnostics");
    let diags = params["diagnostics"].as_array().unwrap();
    let advisory = diags
        .iter()
        .find(|d| d["code"] == "U3100")
        .unwrap_or_else(|| panic!("expected a U3100 advisory, got {diags:#?}"));
    assert_eq!(advisory["source"], "compact-analyzer (unverified)");
    assert_eq!(advisory["severity"], 2); // Warning
    assert!(
        diags.iter().all(|d| d["code"] != "E3100"),
        "an advisory must never manufacture a confirmed E3100 leak, got {diags:#?}"
    );

    client.shutdown();
}

#[test]
fn signature_help_is_advertised_and_reports_active_parameter() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    let caps = client.initialize_with_options(json!({}));
    assert!(
        caps["signatureHelpProvider"].is_object(),
        "initialize must advertise signatureHelpProvider: {caps}"
    );

    let src = "circuit add(x: Field, y: Field): Field { return x + y; }\n\
               circuit c(): Field { return add(1, 2); }";
    did_open(&mut client, &uri, 1, src);

    // Position ON the second argument's `2` — after the top-level comma.
    let (line, col) = lsp_position(src, "2);");
    let resp = client.request(
        "textDocument/signatureHelp",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": col},
        }),
    );
    let result = &resp["result"];
    assert_eq!(result["activeParameter"], 1, "{resp}");
    assert_eq!(result["activeSignature"], 0, "{resp}");
    let sigs = result["signatures"].as_array().expect("signatures array");
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0]["label"], "circuit add(x: Field, y: Field): Field");
    let params = sigs[0]["parameters"].as_array().expect("parameters array");
    assert_eq!(params.len(), 2);
    assert_eq!(params[0]["label"], "x: Field");
    assert_eq!(params[1]["label"], "y: Field");

    client.shutdown();
}

#[test]
fn inlay_hint_is_advertised_and_reports_const_binding_type() {
    let (_dir, uri) = temp_doc();
    let mut client = Client::start();
    let caps = client.initialize_with_options(json!({}));
    assert!(
        caps["inlayHintProvider"].is_object() || caps["inlayHintProvider"] == json!(true),
        "initialize must advertise inlayHintProvider: {caps}"
    );

    let src = "circuit c(): Field { const x = 1; return x; }";
    did_open(&mut client, &uri, 1, src);

    // A range covering the whole (single-line) document.
    let resp = client.request(
        "textDocument/inlayHint",
        json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": src.encode_utf16().count() as u32},
            },
        }),
    );
    let hints = resp["result"].as_array().expect("hints array");
    // `1` types to `TyKind::Uint(Some(2))`, and `display_kind` spells an
    // exact-power-of-two upper bound (2 == 1 << 1) as `Uint<1>` rather than
    // the range form `Uint<0..2>` — ground truth confirmed against
    // analyzer-core's own `ty.rs` display test.
    assert!(
        hints
            .iter()
            .any(|h| h["label"] == ": Uint<1>" && h["kind"] == 1),
        "expected a `: Uint<1>` TYPE inlay hint, got {hints:#?}"
    );
    let hint = hints.iter().find(|h| h["label"] == ": Uint<1>").unwrap();
    let (line, col) = lsp_position(src, "const x");
    assert_eq!(
        hint["position"],
        json!({"line": line, "character": col + "const x".encode_utf16().count() as u32}),
        "hint must land right after the binding name: {hint}"
    );

    // A range that does NOT cover `const x = 1` (only the trailing
    // `return x; }`) must exclude the hint — proves the handler actually
    // filters by range rather than returning every document hint.
    let (rline, rcol) = lsp_position(src, "return x");
    let resp2 = client.request(
        "textDocument/inlayHint",
        json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": rline, "character": rcol},
                "end": {"line": rline, "character": src.encode_utf16().count() as u32},
            },
        }),
    );
    let hints2 = resp2["result"].as_array().expect("hints array");
    assert!(
        hints2.iter().all(|h| h["label"] != ": Uint<1>"),
        "range excluding the const binding must not return its hint: {hints2:#?}"
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
