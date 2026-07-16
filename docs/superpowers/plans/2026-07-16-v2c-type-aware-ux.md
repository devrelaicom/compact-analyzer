# v2c — Type-Aware Editor UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three type-aware editor features — **typed member completion**, **signature help**, and **inlay hints** — as pure consumers of the v2b/v2b.9 checker, advertised by the server and verified in the VS Code extension.

**Architecture:** Each feature is an `analyzer-ide` function over byte offsets + `FileId` (zero LSP types), consumed by a request handler in the `compact-analyzer` binary that converts to/from LSP, plus thin VS Code client wiring. Typed member completion and inlay hints consume v2b.9's `AnalysisHost::type_at(file, offset) -> Option<TyKind>`; signature help consumes the item-tree `Symbol.signature` string + the CST call node. No new analysis engine — these read the checker.

**Tech Stack:** Rust (`analyzer-ide`, `compact-analyzer` server), `lsp-types = 0.95.1`, TypeScript thin client (`editors/vscode/`), `@vscode/test-cli` e2e.

## Global Constraints

- **Pure consumers.** v2c adds NO type-system rules and NO new diagnostics. It reads v2b.9's `type_at` and the resolver/item-tree. If a type isn't modeled, the feature offers nothing there (never a wrong hint/completion).
- **Never a wrong/misleading result.** `type_at` returns `None` for anything unmodeled/unresolved/cross-file-local/generic/over-deep; every feature treats `None` as "offer nothing" — no guessed member, no guessed hint.
- **Compare nominal types via `is_subtype`/name, NEVER `==`** (v2b.9 review caveat: derived `Eq` on `TyKind::Struct` compares fields too). v2c mostly reads `type_at`'s result directly (enumerating `Struct.fields`, displaying via `ty_display`/`display_kind`), so avoid `==` on nominal `TyKind`.
- **No LSP types in `analyzer-core`/`analyzer-ide`;** protocol conversion stays in the `compact-analyzer` binary (`lsp_utils.rs`/`server.rs`). Byte offsets only.
- **In-editor verification is the done-bar** (program spec §8): each feature must be advertised by the server AND verified in the actual VS Code extension host, not merely over LSP integration tests.
- **Single-threaded + cooperative cancellation** preserved (mirror existing IDE features).
- **Pre-release:** no shims/dual paths.

## Background: the as-merged integration points (verified — do not re-derive)

- **`AnalysisHost::type_at(&mut self, file: FileId, offset: TextSize) -> Option<TyKind>`** (v2b.9). Returns `Some(TyKind::Struct { name, fields: Arc<[(Arc<str>, TyKind)]> })` / `Enum { name, variants }` / primitives, or `None` when unmodeled. `analyzer_core::{TyKind, ty_display, display_kind}` are available; `display_kind(&TyKind) -> String` renders a type. Same-file/unique/non-generic only; cross-file struct *refs* resolve.
- **`push_member`** in `crates/analyzer-ide/src/completion.rs` (~680): guards the receiver to `NAME_EXPR`, resolves it, offers ledger-ADT methods (ledger field) or enum variants (enum decl), else **offers nothing** — the gap v2c fills for struct-typed values. Uses `non_trivia_start(receiver)` for the resolve offset.
- **Item-tree signatures:** `host.analyze(file)?.item_tree.symbols[index].signature` is the full `compact` signature string (e.g. `circuit double(x: Field): Field`), already used by `hover.rs`. `Definition::Item { file, index }` from `host.resolve(pos)`.
- **Server capabilities** (`server.rs` ~71-108): advertises completion (trigger `.`), hover, etc. — NO `signature_help_provider`, NO `inlay_hint_provider` yet.
- **Request-handler pattern** (`server.rs` ~465-513): `"textDocument/<method>" => { parse params → self.position_to_file_position(uri, position) → analyzer_ide::<fn>(&mut self.host, pos/file) → map to LSP → self.respond_ok(req.id, result); }`. Line index via `self.host.analyze(file)?.line_index`.
- **VS Code client** (`editors/vscode/`): thin `vscode-languageclient`; capabilities are inherited once advertised. Inlay hints are gated behind a VS Code client setting (`editor.inlayHints.enabled`) — no per-extension code needed to *render*, but the extension may add a `compact-analyzer.inlayHints.*` enablement following the v2b.final `typeDiagnostics` config pattern.
- **e2e harness:** `editors/vscode/src/test/e2e/activation.test.ts` + `.vscode-test.mjs` — drives a real extension host; the §8 verification vehicle.

---

## File Structure

- **Modify** `crates/analyzer-ide/src/completion.rs` — extend `push_member` with a struct-field branch via `type_at`. (Task 1)
- **Create** `crates/analyzer-ide/src/signature_help.rs` — compute `SignatureHelp` data (label + params + active index) from the callee's item-tree signature + the CST call node. (Task 2)
- **Create** `crates/analyzer-ide/src/inlay_hints.rs` — type hints for `const` bindings via `type_at`. (Task 3)
- **Modify** `crates/analyzer-ide/src/lib.rs` — export the two new modules' entry points + result types.
- **Modify** `crates/compact-analyzer/src/server.rs` — advertise `signature_help_provider` (triggers `(`, `,`) + `inlay_hint_provider`; add `textDocument/signatureHelp` + `textDocument/inlayHint` handlers. (Tasks 2–3)
- **Modify** `crates/compact-analyzer/src/lsp_utils.rs` — conversions for the two new result types. (Tasks 2–3)
- **Modify** `editors/vscode/` — inlay-hint enablement setting (if added) + e2e verification. (Task 4)

---

### Task 1: Typed member completion on struct-typed values

Extend `push_member` so that when the receiver is neither a ledger field nor an enum, but `type_at` resolves it to a `TyKind::Struct`, the struct's fields are offered as completions.

**Files:**
- Modify: `crates/analyzer-ide/src/completion.rs` (`push_member`)
- Test: `crates/analyzer-ide/src/completion.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `AnalysisHost::type_at`, `analyzer_core::{TyKind, display_kind}`, existing `CompletionKind::StructField`.
- Produces: struct-field completions from `push_member`.

- [ ] **Step 1: Write the failing test**

Add to `completion.rs` tests (mirror the existing `member_on_enum_offers_variants` test harness — it builds a host with stdlib + an overlay and calls `completion` at a `$0` marker):

```rust
#[test]
fn member_on_struct_typed_param_offers_fields() {
    let labels = completion_labels(
        "struct S { a: Field; b: Boolean; }\n\
         circuit c(s: S): Field { return s.$0a; }",
    );
    assert!(labels.contains(&"a".to_string()), "{labels:?}");
    assert!(labels.contains(&"b".to_string()), "{labels:?}");
}

#[test]
fn member_on_non_struct_local_offers_no_fields() {
    // A Field-typed local has no members -> offer nothing (never a wrong member).
    let labels = completion_labels(
        "circuit c(x: Field): Field { return x.$0y; }",
    );
    assert!(labels.is_empty(), "{labels:?}");
}
```

Use the existing test helper that returns the completion labels (find its exact name in the tests module — e.g. `completion_labels`/`labels_at`; mirror `member_on_enum_offers_variants`).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p analyzer-ide --lib member_on_struct_typed_param_offers_fields` — expected FAIL (no struct branch).

- [ ] **Step 3: Add the struct branch to `push_member`**

After the enum branch (before the closing `}` of `push_member`), add — using `type_at` at the same `non_trivia_start(receiver)` offset the resolve used:

```rust
    // Struct-typed receiver (v2b.9 type_at): offer the struct's fields. This is
    // the typed-member-access case the resolve-based branches above cannot cover
    // (a param/local/const of a nominal struct type). `type_at` returns None for
    // anything unmodeled/unresolved/cross-file-local/generic, so a non-struct or
    // unknown receiver correctly offers nothing.
    let offset = non_trivia_start(receiver);
    if let Some(analyzer_core::TyKind::Struct { fields, .. }) = host.type_at(pos.file, offset) {
        for (name, ty) in fields.iter() {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: CompletionKind::StructField,
                detail: Some(analyzer_core::display_kind(ty)),
                documentation: None,
            });
        }
    }
```

Note `display_kind` must be `pub` (it is `pub(crate)` today — if not re-exported for `analyzer-ide`, use the `pub fn ty_display(db, Ty)` path instead, or make `display_kind` `pub`. Check the `analyzer_core` re-exports; the plan's Task 1 may need a one-line `pub use`/visibility bump in `analyzer-core/src/lib.rs` — do it if needed and note it). Prefer exposing a small `pub fn` if `display_kind` can't be made public cleanly.

- [ ] **Step 4: Run tests**

Run: `cargo test -p analyzer-ide --lib` — new + all existing completion tests green. In particular confirm `member_on_chained_receiver_offers_nothing` and the ledger/enum member tests still pass (the struct branch runs only when the ledger + enum branches didn't return, and `type_at` is `None` for those).

- [ ] **Step 5: Lints + commit**

Run: `cargo clippy -p analyzer-ide --all-targets -- -D warnings`, `cargo fmt --all --check`.

```bash
git add crates/analyzer-ide/src/completion.rs crates/analyzer-core/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(v2c): typed member completion on struct-typed values (via type_at)

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Signature help

New `analyzer-ide` function computing signature-help data at a call site from the callee's item-tree signature string + the CST call node, plus server capability + handler. Pure consumer — uses the existing resolver + item tree (no v2b.9 needed).

**Files:**
- Create: `crates/analyzer-ide/src/signature_help.rs`
- Modify: `crates/analyzer-ide/src/lib.rs` (export `signature_help`, `SignatureHelpData`, `ParamLabel`)
- Modify: `crates/compact-analyzer/src/server.rs` (advertise + handle)
- Modify: `crates/compact-analyzer/src/lsp_utils.rs` (convert to `lsp_types::SignatureHelp`)
- Test: `signature_help.rs` unit tests + `crates/compact-analyzer/tests/lsp_integration.rs`

**Interfaces:**
- Consumes: `AnalysisHost::{resolve, analyze}`, item-tree `Symbol.signature`, `FilePosition`.
- Produces:
  ```rust
  pub struct ParamLabel { pub start: u32, pub end: u32 } // UTF-8 byte offsets INTO the label string
  pub struct SignatureHelpData {
      pub label: String,            // the full signature string
      pub params: Vec<ParamLabel>,  // one per parameter, offsets into `label`
      pub active_param: Option<u32>,
  }
  pub fn signature_help(host: &mut AnalysisHost, pos: FilePosition) -> Option<SignatureHelpData>;
  ```

- [ ] **Step 1: Write the failing unit tests**

In `signature_help.rs`:

```rust
#[test]
fn active_param_advances_with_commas() {
    // cursor after the first comma -> active param index 1
    let d = sig_help_at(
        "circuit add(x: Field, y: Field): Field { return x + y; }\n\
         circuit c(): Field { return add(1, $0 2); }",
    ).unwrap();
    assert_eq!(d.label, "circuit add(x: Field, y: Field): Field");
    assert_eq!(d.params.len(), 2);
    assert_eq!(d.active_param, Some(1));
    // param labels index into the signature string
    assert_eq!(&d.label[d.params[0].start as usize..d.params[0].end as usize], "x: Field");
}

#[test]
fn signature_help_absent_outside_a_call_is_none() {
    assert!(sig_help_at("circuit c(): Field { return 1$0; }").is_none());
}
```

`sig_help_at` = a test helper (mirror `hover.rs`'s `hover_md`: extract the `$0`, build a host with stdlib, call `signature_help`).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p analyzer-ide --lib signature_help` — FAIL (module doesn't exist).

- [ ] **Step 3: Implement `signature_help`**

Algorithm:
1. Find the enclosing `CALL_EXPR` whose argument-list parens contain `pos.offset` (walk ancestors of the token at offset; a `CALL_EXPR` with a `DOT` child is a method call — for v2c scope, method-call signature help is out; return `None` for those, matching `callee_sig`'s posture).
2. Resolve the callee name token → `Definition::Item { file, index }` → `analyze(file).item_tree.symbols[index].signature` (the label). `None` if unresolved or not a circuit/witness.
3. Parse the parameter labels from the signature: locate the top-level `(...)` in the label, split on **top-level** commas (depth-track `<>`/`[]`/`()` so `Uint<0..8>` / `[A, B]` don't split), each slice's trimmed byte-range → a `ParamLabel` (offsets into `label`).
4. `active_param` = count of top-level commas in the call's argument list before `pos.offset` (depth-track the same way). Clamp to `params.len().saturating_sub(1)` when there are params; `None` when the signature has zero params.
5. Return `SignatureHelpData`.

Follow `hover.rs`'s CST-walk idioms (`ident_at`, `non_trivia_start`, ancestor walk). Never panic on stale offsets (clamp like `hover.rs`).

- [ ] **Step 4: Export + server capability + handler**

In `analyzer-ide/src/lib.rs`: `mod signature_help; pub use signature_help::{signature_help, SignatureHelpData, ParamLabel};`.

In `server.rs` capabilities: add
```rust
        signature_help_provider: Some(lsp_types::SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
```
Add the handler arm (mirror `textDocument/completion`), converting via a new `lsp_utils::signature_help_to_lsp(SignatureHelpData) -> lsp_types::SignatureHelp` (one `SignatureInformation`, `parameters` = each `ParamLabel` as `ParameterInformation { label: ParameterLabel::LabelOffsets([start, end]) }` — note LSP label offsets are **UTF-16**; convert the byte offsets into the label to UTF-16 using the label string, or use `ParameterLabel::Simple(substring)` to sidestep encoding). Set `active_signature: Some(0)` and `active_parameter` from the data.

- [ ] **Step 5: LSP integration test**

In `lsp_integration.rs`, add a test that `initialize` advertises `signatureHelpProvider`, and a `textDocument/signatureHelp` request at a call site returns a signature with the expected `activeParameter` (mirror an existing request test).

- [ ] **Step 6: Verify + commit**

`cargo test -p analyzer-ide --lib`, `cargo test -p compact-analyzer --test lsp_integration`, clippy + fmt.

```bash
git add crates/analyzer-ide/src/signature_help.rs crates/analyzer-ide/src/lib.rs crates/compact-analyzer/src/server.rs crates/compact-analyzer/src/lsp_utils.rs crates/compact-analyzer/tests/lsp_integration.rs
git commit -m "$(cat <<'EOF'
feat(v2c): signature help (active-parameter at call sites)

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Inlay hints (const binding types)

New `analyzer-ide` function emitting a type inlay hint after each un-annotated `const` binding whose initializer `type_at` can type, plus server capability + handler.

**Files:**
- Create: `crates/analyzer-ide/src/inlay_hints.rs`
- Modify: `crates/analyzer-ide/src/lib.rs`
- Modify: `crates/compact-analyzer/src/server.rs`, `lsp_utils.rs`
- Test: `inlay_hints.rs` unit tests + `lsp_integration.rs`

**Interfaces:**
- Consumes: `AnalysisHost::type_at`, `display_kind`/`ty_display`, the CST.
- Produces:
  ```rust
  pub struct InlayHint { pub offset: u32, pub label: String } // label e.g. ": Field"; offset = byte pos to insert at (after the binding name)
  pub fn inlay_hints(host: &mut AnalysisHost, file: FileId) -> Vec<InlayHint>;
  ```

- [ ] **Step 1: Write the failing unit tests**

```rust
#[test]
fn const_binding_gets_a_type_hint() {
    let hints = hints_for("circuit c(): Field { const x = 1; return x; }");
    assert!(hints.iter().any(|h| h.label == ": Uint<0..2>"), "{hints:?}");
}

#[test]
fn annotated_const_gets_no_hint() {
    // Already annotated -> redundant, emit nothing.
    let hints = hints_for("circuit c(): Field { const x: Field = 1; return x; }");
    assert!(hints.iter().all(|h| !h.label.starts_with(": ")) || hints.is_empty(), "{hints:?}");
}

#[test]
fn unmodeled_initializer_gets_no_hint() {
    // A call whose type isn't modeled -> type_at None -> no hint (never a guess).
    let hints = hints_for("circuit c(): Field { const x = mystery(); return 1; }");
    assert!(hints.is_empty(), "{hints:?}");
}
```

`hints_for` = test helper building a host + overlay, calling `inlay_hints`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p analyzer-ide --lib inlay` — FAIL.

- [ ] **Step 3: Implement `inlay_hints`**

Walk the file's CST for `CONST_STMT` nodes (confirm the exact node kind in `compactp_syntax`); for each **without** an explicit type annotation:
- Compute the initializer expression's offset; `type_at(file, init_offset)`.
- If `Some(kind)` (never `None`; `type_at` already returns `None` for Unknown) → hint `label = format!(": {}", display_kind(&kind))`, `offset` = byte position just after the binding-name identifier.
- Skip annotated consts (they already show the type). Poll `should_continue` on the walk (mirror other IDE walks).

- [ ] **Step 4: Export + capability + handler**

`lib.rs`: `mod inlay_hints; pub use inlay_hints::{inlay_hints, InlayHint};`.
`server.rs` capabilities: `inlay_hint_provider: Some(lsp_types::OneOf::Left(true)),`.
Handler arm `textDocument/inlayHint`: parse `InlayHintParams` (has a `range`), get the file + line index, call `analyzer_ide::inlay_hints`, filter to hints within the requested range, convert each to `lsp_types::InlayHint { position: <byte→LSP via line_index>, label: InlayHintLabel::String(h.label), kind: Some(InlayHintKind::TYPE), padding_left: Some(false), padding_right: Some(false), .. }`. Add `lsp_utils::inlay_hint_to_lsp`.

- [ ] **Step 5: LSP integration test**

`lsp_integration.rs`: `initialize` advertises `inlayHintProvider`; a `textDocument/inlayHint` request over a range covering `const x = 1;` returns a `: Uint<0..2>` hint.

- [ ] **Step 6: Verify + commit**

`cargo test -p analyzer-ide --lib`, `cargo test -p compact-analyzer --test lsp_integration`, clippy + fmt.

```bash
git add crates/analyzer-ide/src/inlay_hints.rs crates/analyzer-ide/src/lib.rs crates/compact-analyzer/src/server.rs crates/compact-analyzer/src/lsp_utils.rs crates/compact-analyzer/tests/lsp_integration.rs
git commit -m "$(cat <<'EOF'
feat(v2c): inlay hints for const binding types (via type_at)

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: VS Code client wiring + in-editor verification (§8 done-bar)

Confirm the three features work in a real extension host. Signature help + completion inherit from capability advertisement (no client code). Inlay hints render once the server advertises the capability and the client's `editor.inlayHints.enabled` is on; add a `compact-analyzer.inlayHints` enablement setting only if the team wants a per-extension toggle (follow the v2b.final `typeDiagnostics` config pattern).

**Files:**
- Create: `editors/vscode/src/test/e2e/fixture/uxfeatures.compact`
- Modify: `editors/vscode/src/test/e2e/activation.test.ts`
- (Optional) Modify: `editors/vscode/package.json`, `src/config-core.ts` if adding an inlay-hint enablement setting.

- [ ] **Step 1: Fixture**

`uxfeatures.compact` — a file exercising all three: a struct + a circuit with a `const` binding and a call:
```
struct S { a: Field; b: Boolean; }
circuit add(x: Field, y: Field): Field { return x + y; }
export circuit c(s: S): Field { const n = add(1, 2); return s.a + n; }
```
Verify it compiles-accepts live (`compact compile --skip-zk --vscode`). It is valid, so no compile-check exclusion needed.

- [ ] **Step 2: e2e assertions**

In `activation.test.ts` (under the running-server scenario), add tests driving the real host:
- **Inlay hints:** `vscode.commands.executeCommand('vscode.executeInlayHintProvider', uri, fullRange)` → assert a hint whose label contains `Field` appears near the `const n` line.
- **Signature help:** `vscode.commands.executeCommand('vscode.executeSignatureHelpProvider', uri, positionInsideAddCall, '(')` → assert `signatures.length >= 1` and the active parameter is defined.
- **Typed member completion:** open the doc, position after `s.`, `vscode.commands.executeCommand('vscode.executeCompletionItemProvider', uri, posAfterDot)` → assert items include `a` and `b`.

Poll where needed (the server must have indexed the file); reuse the existing `delay`/`fixtureUri` helpers. Set generous `this.timeout`.

- [ ] **Step 3: Run e2e**

`cargo build -p compact-analyzer` (fresh server binary), then `cd editors/vscode && npm run test:e2e`. Confirm the three new assertions pass in the real host.

- [ ] **Step 4: Full client verification + commit**

`cd editors/vscode && npm test && npm run check && npm run lint && npm run test:e2e`.

```bash
git add editors/vscode/src/test/e2e/fixture/uxfeatures.compact editors/vscode/src/test/e2e/activation.test.ts
git commit -m "$(cat <<'EOF'
test(v2c): in-editor e2e for member completion, signature help, inlay hints

Co-Authored-By: Claude Code <noreply@anthropic.com>
EOF
)"
```

---

## Final verification (whole-milestone)

- [ ] `cargo test --workspace` green (analyzer-ide unit + lsp_integration + all existing).
- [ ] Corpus gate `false_positives=0` (v2c adds no diagnostics, so it must be unchanged — confirm no regression).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all --check` clean.
- [ ] `cd editors/vscode && npm test && npm run check && npm run lint && npm run test:e2e` — the three features verified in a real extension host (§8 done-bar).

## Notes / risks for the reviewer

- **Never a wrong result** is the v2c contract: every feature offers nothing when `type_at` is `None` (member completion, inlay hints) or the callee doesn't resolve (signature help). No feature guesses.
- **Nominal `==` caveat** (v2b.9): compare/branch on `TyKind` via pattern-match on `Struct{..}`/`is_subtype`, never `==`.
- **LSP label-offset encoding**: signature-help `ParameterLabel::LabelOffsets` is UTF-16 into the label; either convert or use `ParameterLabel::Simple(substring)`. Called out in Task 2 Step 4.
- **Method-call signature help / chained-receiver member completion** are out of scope (deferred, safe): `callee_sig`/`type_at` cover same-file/unique/non-generic and one-level receivers; deeper cases offer nothing.
- **v2b.9 precision gap** (same-named cross-module struct pruning) surfaces here only as an occasional missing member-completion — safe, tracked in the v2b.9 memory.
