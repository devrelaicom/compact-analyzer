# M4 — Toolchain Integration Implementation Plan

> **DRAFT — drift-checked against merged M3 (`main` @ `ed206a4`, Errata E1–E7) on 2026-07-10; see "M3 Drift Reconciliation" below.**
>
> Of the original pre-execution checklist, items (1) confirm M3's final public surface and (2) re-read the M3 Errata are **DONE** — results are folded into this draft in place. Item (3) still stands: re-run the "Plan-time verification items" below against the *pinned* `compact` toolchain at implementation time (the CLI ships monthly — the facts below were captured against `compact 0.5.1` / `compactc 0.31.1` / language `0.23.0` on 2026-07-10 and MUST be re-fixtured).

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

## M3 Drift Reconciliation (checked 2026-07-10)

Checked by a fresh reviewer against merged `main` @ `ed206a4` (M3 Done; 196 tests): `crates/analyzer-core`, `crates/analyzer-ide`, `crates/compact-analyzer` (server.rs, lsp_utils.rs, tests/support), the workspace manifests, and the M3 plan's Errata (E1–E7) + Self-review notes. Corrections are applied in place below; this section is the summary.

**Confirmed unchanged (former plan-time VERIFY item 10 — now RESOLVED):**
- `LineIndex::line_col(TextSize) -> LineCol { line: u32 /*0-based*/, col: u32 /*UTF-16*/ }` and `LineIndex::offset(LineCol) -> Option<TextSize>`. Columns are UTF-16 (M3-confirmed). Clamp semantics (relevant to Tasks 3/7): `line_col` clamps past-EOF offsets to the end and mid-char offsets down to a char boundary; `offset` clamps a past-line-end column to the line's content end and returns `None` only for a nonexistent line.
- `Diagnostic` (re-export of `compactp_diagnostics::Diagnostic`: `severity` Error/Warning/Note, `primary_span`, `message`, `notes`, `secondary_spans`, `code`), `FilePosition { file, offset }`, `AnalysisHost::analyze(FileId) -> Option<FileAnalysis>` (`green`, `diagnostics`, `line_index`, `item_tree`), `AnalysisHost::import_search_path() -> Vec<PathBuf>`, the `Vfs` API (`file_id`/`path`/`set_overlay`/`remove_overlay`/`overlay_version`/`invalidate_disk`/`read`), and `lsp_utils::{range_to_lsp, offset_from_position, diagnostic_to_lsp}` — all exactly as this plan consumes them. M3 *added* surface this plan can lean on (`scope_bindings_at`, `Binding`, `LedgerMethod`, `host.ledger_adt_methods`, `host.ledger_field_adt`; analyzer-ide `completion`/`semantic_tokens`/`folding_ranges`/`selection_ranges`; binary `semantic_tokens_legend`), but nothing it consumes was renamed.
- Pins: `lsp-types = "0.95.1"`, `compactp_* = "0.1.0-beta.1"`, NO `[patch.crates-io]`. `compactp_parser`, `compactp_syntax`, `text-size`, `tempfile` are already `[workspace.dependencies]` — Task 1 introduces zero new external crates.
- The capability literal still advertises `TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)` (server.rs) — Task 6's replace-with-Options instruction stands (the literal also now carries completion/semanticTokens/folding/selection entries; add save + formatting alongside them). `corpus_dir()` and `m3_features_never_panic_on_corpus` exist in `tests/corpus_smoke.rs`.

**Corrections applied (stale vs. merged M3):**
1. **Worker↔state plumbing gap (Task 6 / Open Question 1 — the biggest find).** `GlobalState` is owned by the single-threaded main loop. A compile worker gets a cloned `sender`, but that channel is write-only *toward the client* — there is NO path back into `handle_message`/`GlobalState`. The draft's `compiler_diagnostics: HashMap<FileId, …>` field could not be written by the worker as drafted. Corrected to `Arc<Mutex<…>>` shared state (alternative: a second channel + `crossbeam_channel::select!` in the main loop — decide in OQ 1). Also: a worker must NEVER `recv` from a `cancel_receiver` clone — it is a clone of `connection.receiver`, and receiving would steal client messages; the M3 pattern only tests `is_empty()` on the main thread.
2. **Shutdown-hang hazard (new invariant from merged code).** `run()` drops `state`+`connection` before `io_threads.join()`, and the writer thread exits only when every `sender` clone is dropped. A detached compile worker holding a cloned `sender` past shutdown blocks `io_threads.join()` forever. Added to Global Constraints + OQ 1.
3. **`didSave` is a *grouped* no-op arm** (`"textDocument/didSave" | "initialized" | "$/setTrace" | "$/cancelRequest" => {}`) — Task 6 splits `didSave` out of the group rather than replacing a dedicated arm.
4. **Native diagnostics come from TWO sources:** `publish_file_diagnostics` publishes `analysis.diagnostics` (parse) + `host.resolution_diagnostics(file)`, and `pending_diagnostics` is a per-file `HashMap<FileId, Instant>` debounce. The compiler-set re-merge must live inside `publish_file_diagnostics` (so debounced native republishes retain compiler diagnostics), and span-dedup must consider both native sources.
5. **The never-die offset clamp is an M3-established pattern this plan must reuse** (E7/B1): rowan's `token_at_offset` `assert!`-panics past the root's end; M3's fix wave clamps via `offset.min(root.text_range().end())` in `anchor_token`/`ident_at_offset` (+ guards in completion/hover/selection_ranges). Task 3's token-snap now carries the same explicit clamp requirement, and `anchor_token`'s `Between(l, r)` prefer-right-IDENT behavior is the precedent for snapping at a token's start offset.
6. **No language-version policy notice exists in the merged binary** — Task 6's "feed `language_version` into the existing … notice if wired" is resolved: not wired; log versions to stderr (a user-facing mismatch notice would be new, optional M4 scope).
7. **`LineIndex` has no public line-start/byte accessor** (`line_starts` is private) and no scalar-col API — resolves Task 3 Step 3(a)'s VERIFY: scan for `\n` locally in `analyzer-toolchain` (or extend `LineIndex`, OQ 5).
8. **Test-harness gaps (Task 9's VERIFY resolved):** `support::Client::start()` spawns with inherited env (no override hook), and `initialize`/`initialize_root` never pass `initializationOptions`. Tasks 6/7/9 need harness extensions: an env-controlling start variant (to scrub `PATH`) and an initialize variant accepting `initializationOptions` (for `compileOnSave`/`toolchainPath`). `tests/support/mod.rs` added to the Modify list.
9. **Task 1 manifest fixes:** `tempfile` moved to `[dependencies]` (Task 5's `format_source` uses it at *runtime*, not just in tests); the direct `compactp_syntax` dep dropped — M3's convention (analyzer-ide) consumes `SyntaxKind`/`SyntaxNode` via analyzer-core's re-exports, and `locate` only needs `compactp_parser::parse` directly.
10. Header Errata range corrected to E1–E7; DoD test baseline set to M3's actual 196.

**Still needs empirical verification at M4 implementation time (NOT resolved here):** all of "Plan-time verification items" 1–9 — the `compact` CLI invocation shape, `--vscode` error grammar, exit codes, the scalar-column convention, `compact format` in-place/`--check` semantics, `--compact-path` passthrough, scratch-dir semantics, and the exact lsp-types 0.95.1 field/variant names. None of these were re-probed during this reconciliation; treat every CLI string/exit-code in this plan as a fixture to recapture.

**Goal:** Add the optional `compact` CLI integration as a new `analyzer-toolchain` crate: compile-on-save diagnostics from the real compiler and formatting via `compact format` — with everything native continuing to work when the toolchain is absent, and never spamming errors when it is.

**Architecture:** A new feature-layer crate, `analyzer-toolchain`, sits beside `analyzer-ide` in the dependency graph: it depends on `analyzer-core` (for the `SyntaxNode`/`SyntaxKind` re-exports `locate`'s token-snap uses — its public surface speaks plain `text-size` byte offsets) but NOT on `analyzer-ide` and NOT on `lsp-types`. It owns four pure-ish concerns — toolchain **discovery/version query**, compiler-stderr **parsing**, `(line, char) → TextRange` **position mapping**, subprocess **invocation** (compile + format) — all expressed in plain byte-offset types. The `compact-analyzer` binary orchestrates: on `didSave` it spawns a compile worker, maps the compiler's output into diagnostics tagged by source, dedups against native diagnostics, and publishes; on `textDocument/formatting` it round-trips the buffer through `compact format` and returns a `TextEdit`. Missing toolchain degrades gracefully with a one-time `window/showMessage`.

**Tech Stack:** Rust (edition 2024, rust-version 1.90); `analyzer-core` (internal); `text-size`; `lsp-types` 0.95.1 + `lsp-server` (binary only); `std::process` for subprocess control; `tempfile` (a **runtime** dep of `analyzer-toolchain` — `format_source` round-trips a temp file — as well as dev; already a workspace dep). No new third-party runtime crate is anticipated (see Open Question 6 on whether a subprocess-timeout helper crate is warranted vs. a hand-rolled poll loop).

---

## Global Constraints

Every task's requirements implicitly include this section. Copied from the project design + M3 plan — **do not re-derive**.

- **Layering:** `lsp-types` appears ONLY in the `compact-analyzer` binary. `analyzer-core`, `analyzer-ide`, and the new `analyzer-toolchain` speak byte offsets (`TextSize`/`TextRange`) + `FileId` + plain Rust types. The compiler's `(line, char)` positions are mapped to `TextRange` inside `analyzer-toolchain`; UTF-16 conversion for publication happens only at the binary boundary (reusing the existing `lsp_utils::range_to_lsp`). `analyzer-toolchain` MUST NOT depend on `lsp-types` or `analyzer-ide`.
- **Never-die:** a broken, hung, or adversarial compiler MUST NOT take the server down or wedge the main loop. Compile-on-save runs off the main loop (worker thread) with a timeout; every subprocess interaction is wrapped so a panic/`Err` degrades to null/empty. Formatting failures return `None` (no edit), never a panic. Worker threads run OUTSIDE `dispatch`'s per-message `catch_unwind`, so every worker body needs its own `catch_unwind`. Reuse M3's offset-clamp pattern (`offset.min(root.text_range().end())` before rowan `token_at_offset` — M3 E7/B1) anywhere a CST is probed at an arbitrary offset.
- **Never-hang shutdown (M3-verified):** `run()` drops `state`+`connection` before `io_threads.join()`, and the writer thread exits only once every `sender` clone is dropped. An in-flight compile worker holding a cloned `sender` past shutdown blocks the join forever. Workers must be joined at shutdown (bounded by the compile timeout) or signalled to abandon + their child processes killed — decide in Open Question 1.
- **Pinned versions:** `lsp-types` = `0.95.1` (0.96+ drops `url::Url`). `compactp_*` = `0.1.0-beta.1` (language 0.23). **Never commit a `[patch.crates-io]`.** The `compact` CLI is a *runtime* dependency discovered at startup, NOT a build dependency — nothing about M4 changes the crate version pins.
- **Tooling gate, per task:** `cargo fmt`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and tests, all green. Conventional commits (`fix:` for correctness fixes, `feat:` for new capability, not `test:`).
- **Bin-only binary:** test it with `cargo test -p compact-analyzer` (NOT `--lib`). When adding a new crate/dependency, the **first** build must NOT use `--locked` (it needs to update `Cargo.lock`); subsequent gates restore `--locked`. Forward-declared fields use `#[allow(dead_code)]`, never `#[expect(dead_code)]`.
- **Stdout is protocol-only** (logs → stderr). A subprocess's stdout/stderr is captured into buffers — it MUST NOT be inherited onto the server's stdio (that would corrupt the LSP stream).
- **Toolchain optionality is a hard invariant:** every feature added here is additive. With no `compact` on PATH and no override configured, the server behaves exactly as it does at end of M3. This is asserted by tests that run with the toolchain deliberately hidden.

---

## Empirically-Verified Facts (captured 2026-07-10; RE-FIXTURE at the pinned version before use)

Verified by direct read-only probes of the installed toolchain: `compact 0.5.1` (wrapper), `compactc 0.31.1` (compiler, via `compact compile --version`), language `0.23.0` (`compact compile --language-version`), ledger `8.0.2`. **Upstream ships roughly monthly; treat every string/exit-code below as a fixture to reconfirm, not a constant to trust.** Each task that depends on one of these carries a `VERIFY` note.

### T1 — Toolchain shape and version query

- The user-facing tool is **`compact`** (a version-managing wrapper), not a bare `compactc`. Subcommands: `check update format fixup list clean self compile help`.
- `compact --version` → `compact 0.5.1` — this is the **wrapper** version, NOT the compiler. Do not use it for the language-version policy notice.
- The compiler/toolchain and language versions come from the `compile`/`format` subcommands:
  - `compact compile --version` → `0.31.1`
  - `compact compile --language-version` → `0.23.0`
  - `compact compile --ledger-version` → (ledger version); `--runtime-version` also exists (passthrough to `compactc`).
  - `compact format --version` → `0.31.1`; `compact format --language-version` → `0.23.0`.
- **Do NOT shell out to `compactc` directly.** The `~/.compact/bin/compactc` shim resolves `compactc.bin` next to itself, but the actual binary lives under `~/.compact/versions/<v>/<arch>/`; calling the shim out of context fails (`No such file or directory`). Going through **`compact compile`** lets the wrapper select the version. A specific compiler version can be pinned with the passthrough syntax `compact compile +<VERSION> …` (per `compact help compile`: "use +VERSION to specify version"). **VERIFY**: whether any target environment exposes a bare `compactc` on PATH that should be preferred (see Open Question 2).

### T2 — Compile invocation and outputs (`compact compile --skip-zk --vscode <src> <target-dir>`)

- Both a **source path AND a target directory are required** positional args (passed through to `compactc.bin <flags> <source> <target-dir>`). Omitting the target dir prints the compiler's `Usage:` to stderr and exits **1** (a *usage/invocation* error — distinct from a compile error's 255).
- **Success:** exit **0**, empty stdout, empty stderr. The target dir is populated with `compiler/contract-info.json`, `contract/index.{d.ts,js,js.map}`, and (even with `--skip-zk`) `zkir/<circuit>.zkir`. `--skip-zk` skips only proving-key generation.
- **Compile error:** exit **255**. The error is on **stderr**. Format:
  - **`--vscode` (what we use):** exactly one line — `Exception: <basename> line <N> char <C>: <message>`.
  - Without `--vscode`: the same content split across two lines (`Exception: <basename> line N char C:\n  <message>`). We always pass `--vscode`.
- **The file token is the source's BASENAME, not the path passed.** Probed with an absolute source path from an unrelated cwd → still `pos.compact`. **Consequence:** when compiling a file that imports/includes others, an error *inside a dependency* is attributed to that dependency's basename (e.g. `util.compact line 4 char 15: …`, `inc_part.compact line 2 char 13: …`), and two files sharing a basename are ambiguous. This drives the attribution decision in Task 6 / Open Question 3.
- **The compiler is fail-fast: it reports only the FIRST error, then stops.** A file with two undefined identifiers yields one `Exception` line. So a compile-on-save run typically yields at most one compiler diagnostic. (Do not build multi-error accumulation logic on the assumption of many.)
- **Line/char convention (load-bearing — verified with multibyte + astral probes):** `line` and `char` are both **1-based**. `char` is a **Unicode scalar (codepoint) column within the line — NOT a byte offset and NOT a UTF-16 code unit**:
  - `  const x = undefined_name;` → the `u` (0-based byte 12) is reported at **char 13** (1-based).
  - `  /* é */ const x = undefined_name;` (é = 1 scalar, 2 bytes) → char **21** (scalar), not 22 (byte).
  - `  /* 😀 */ const x = undefined_name;` (😀 = 1 scalar, 2 UTF-16 units, 4 bytes) → char **21** (scalar), not 22 (UTF-16) and not 24 (byte).
  - The reported char points at the **start of the offending token**; there is **no end column**. Task 3 must synthesize the end (snap to the CST token at that offset, or fall back to a zero-width/one-char span).
- Latency: a trivial `--skip-zk` compile ran in ~0.19 s. Real files will be slower; a timeout is still mandatory. **VERIFY** typical latency on a representative multi-file project to inform the default timeout.

### T3 — Formatting (`compact format`)

- `compact format <file>` **rewrites the file IN PLACE**, is **silent** (empty stdout + stderr) on success, exit **0**. It does NOT print the formatted text to stdout. This shapes the whole LSP-formatting design (Task 7 / Open Question 4): to produce a `TextEdit` without touching the user's real file, we must round-trip a *copy* of the current buffer through a temp file.
- `compact format --check <file>` — does NOT modify; exit **1** if the file is unformatted (prints a unified-diff-ish report to stderr ending `Error: formatting failed`), exit **0** and silent if already formatted. Useful as a "needs formatting?" probe but its diff is not directly a `TextEdit`.
- `compact format <broken-syntax-file>` — exit **1**, stderr `<basename>: failed\nError: formatting failed`, file **unchanged**. So a syntax error surfaces as a non-zero exit; formatting must degrade to "no edit" in that case.
- The default `FILES` argument is `.` (the whole cwd). **Always pass an explicit path** — never invoke `compact format` with no file argument from inside a worker.
- Flags: `-c/--check`, `-v/--verbose`, `--version`, `--language-version`.

### T4 — `lsp-types` 0.95.1 surface for M4 (verified at `~/.cargo/registry/src/*/lsp-types-0.95.1/`)

- **Save:** `notification::DidSaveTextDocument`, METHOD `"textDocument/didSave"`, `Params = DidSaveTextDocumentParams { text_document, text: Option<String> }`. To receive it, advertise save support: `TextDocumentSyncCapability::Options(TextDocumentSyncOptions { save: Some(TextDocumentSyncSaveOptions::…), open_close: Some(true), change: Some(TextDocumentSyncKind::FULL), .. })` — replacing the current bare `TextDocumentSyncCapability::Kind(FULL)`. `SaveOptions { include_text: Option<bool> }`; `TextDocumentSyncSaveOptions::{Supported(bool), SaveOptions(SaveOptions)}`. **VERIFY** exact field/variant names against the pinned lsp-types before writing. (Drift-checked: the merged server still ignores `didSave`, but as part of a *grouped* no-op arm — `"textDocument/didSave" | "initialized" | "$/setTrace" | "$/cancelRequest" => {}` — Task 6 splits `didSave` out of the group.)
- **Formatting:** `request::Formatting`, METHOD `"textDocument/formatting"`, `Params = DocumentFormattingParams { text_document, options: FormattingOptions, .. }`, `Result = Option<Vec<TextEdit>>`. Capability: `ServerCapabilities.document_formatting_provider: Option<OneOf<bool, DocumentFormattingOptions>>` → `Some(OneOf::Left(true))`.
- **Status message:** `notification::ShowMessage` (METHOD `"window/showMessage"`), `Params = ShowMessageParams { typ: MessageType, message: String }`; `MessageType::{ERROR, WARNING, INFO, LOG}` (newtype consts, in `window.rs`). Used for the one-time "toolchain not found" notice.
- `TextEdit { range: Range, new_text: String }` (already used by rename). A whole-document replace uses a range from `(0,0)` to the document's last line/col.

### T5 — Existing binary/core surface this plan builds on (re-verified against merged M3, `main` @ `ed206a4`)

- `server.rs::GlobalState` holds `host: AnalysisHost`, `open_files: HashMap<Url, FileId>`, `sender: crossbeam_channel::Sender<Message>` (a clone of the client-bound sender — usable from a worker thread to `publish`, but **write-only toward the client**: a worker cannot reach back into `GlobalState` through it — see Open Question 1), `pending_diagnostics: HashMap<FileId, Instant>` (per-file debounce deadlines), `workspace_roots`, `watch_supported`, `cancel_receiver` (a clone of `connection.receiver`, used ONLY to test emptiness for cooperative cancellation on the main thread — never `recv` from it off the main loop or client messages are stolen), `next_request_id` (server→client requests via `send_request`), plus `import_search_path` via `host`. Diagnostics are published by `publish_file_diagnostics` → `lsp_utils::diagnostic_to_lsp`, which hardcodes `source: Some("compact-analyzer")` — and it publishes **two native sets**: `analysis.diagnostics` (parse) + `host.resolution_diagnostics(file)`. Task 6's compiler-set re-merge must be added inside `publish_file_diagnostics` so debounced native republishes retain compiler diagnostics.
- `handle_notification` ignores `didSave` via a **grouped no-op arm** (`"textDocument/didSave" | "initialized" | "$/setTrace" | "$/cancelRequest" => {}`) — Task 6 splits `didSave` out of the group into its own arm.
- `import_search_path_from(options)` shows the established `initializationOptions` parsing pattern (reads `importSearchPath`, falls back to `COMPACT_PATH`). Config toggles (Task 8) follow the same shape. Note the integration-test harness (`tests/support/mod.rs`) currently has **no way to pass `initializationOptions` or env overrides** — see Task 9.
- `lsp_utils::range_to_lsp(li, range)` maps a byte `TextRange` → UTF-16 `Range`. `LineIndex` offers `line_col(byte) -> LineCol{line, col(utf16)}` and `offset(LineCol) -> Option<TextSize>` (drift-checked: unchanged; `line_col` clamps past-EOF offsets to the end, `offset` clamps past-line-end columns to the line's content end). **It does NOT expose a `(line, scalar-col) -> byte` conversion, and `line_starts` is private (no public line-start-byte accessor either)** — Task 3 needs its own (scan the source in `analyzer-toolchain`, or extend `LineIndex`; see Open Question 5).
- `analyzer-core::stdlib::materialize` shows the "write a temp file under a versioned temp dir" idiom the scratch/format temp dirs can mirror. The scratch dir naming can follow `std::env::temp_dir().join(format!("compact-analyzer-…-{}", env!("CARGO_PKG_VERSION")))` (exactly the stdlib-dir naming `run()` already uses).
- Never-die precedents to mirror: `dispatch` wraps every message in `catch_unwind` (a worker thread is outside it → wrap the worker body yourself); `flush_pending_diagnostics` double-wraps per-file publication; analyzer-core's `anchor_token`/`ident_at_offset` clamp offsets before rowan's `token_at_offset` (M3 E7/B1).

---

## Plan-time verification items (check against the real tooling BEFORE writing the parser/invoker)

Do NOT trust the strings/behaviours above as constants. At implementation time, against the **pinned** `compact` version, reconfirm and capture fresh fixtures for:

1. **VERIFY (invocation):** the exact success invocation. Is it `compact compile --skip-zk --vscode <src> <target>` (as verified), and does the target dir remain required? Confirm `--vscode` still collapses to a single line and `--skip-zk` still skips only ZK keys.
2. **VERIFY (binary choice):** whether to call `compact compile` (wrapper) vs. a bare `compactc` if present on PATH; whether `+<VERSION>` pinning is desired to lock the compiler to a language-0.23-compatible release. (Open Question 2.)
3. **VERIFY (error grammar):** the exact stderr grammar of a compile error under `--vscode`. Capture ≥4 real fixtures: (a) a semantic error (`operation … undefined for ledger field type …`), (b) an `unbound identifier`, (c) a `parse error: …`, (d) an error inside an imported/included dependency (basename attribution). Confirm the leading `Exception: ` prefix, the `line N char C:` middle, and that only the FIRST error is emitted.
4. **VERIFY (position units):** re-run the multibyte + astral (emoji) probes to confirm `char` is still a **1-based Unicode-scalar column**. If upstream ever switches to bytes or UTF-16, Task 3's mapping changes.
5. **VERIFY (exit codes):** `0` success, `255` compile error, `1` usage/invocation error, and `compact format` exit `0`/`1` for formatted/unformatted/broken. Do not branch on exit code alone — always also inspect stderr.
6. **VERIFY (format behaviour):** that `compact format <file>` still writes in place + is stdout-silent; that `--check` still exits 1 with a diff on unformatted input; that a syntax error yields `<basename>: failed`. Confirm there is no stdout-emitting "format to stdout" flag we should prefer over the temp-file round-trip (grep `compact help format` for anything like `--stdout`/`-`). (Open Question 4.)
7. **VERIFY (compact-path passthrough):** that `--compact-path <list>` is accepted by `compact compile` (colon-separated on Unix, semicolon on Windows) so multi-file projects resolve imports the same way the analyzer does. The analyzer already computes an import search path (`AnalysisHost::import_search_path`); pass it through.
8. **VERIFY (scratch dir semantics):** that pointing `--skip-zk` compile at a throwaway target dir is safe to repeat and cheap to delete; confirm the compiler does not require the target dir to pre-exist (it created `out-good/` on demand in probes) and does not write outside it.
9. **VERIFY (lsp-types names):** the exact `TextDocumentSyncOptions` / `TextDocumentSyncSaveOptions` / `SaveOptions` field names and the `Formatting` request associated types at the pinned lsp-types 0.95.1.
10. ~~**VERIFY (M3 drift)**~~ **RESOLVED 2026-07-10 (see "M3 Drift Reconciliation"):** `LineIndex`, `Diagnostic`, `FilePosition`, `AnalysisHost::analyze`, and `lsp_utils::{range_to_lsp, offset_from_position}` all retain the signatures this plan consumes on merged `main` @ `ed206a4`; M3 Errata E1–E7 renamed nothing this plan touches. New M3 invariants folded in: the worker↔state plumbing correction (OQ 1), the shutdown-join constraint, the grouped `didSave` arm, the two-source native diagnostics merge point, and the rowan `token_at_offset` clamp pattern.

---

## Open Questions (genuine design decisions — resolve with the human before/at implementation)

1. **Compile-on-save concurrency & staleness model.** Recommended baseline: spawn a **worker thread per save** that runs `compact compile`, maps the result, and publishes `textDocument/publishDiagnostics` **directly via a cloned `sender`**, tagging results with the document version compiled; a newer save supersedes older results (drop stale by tracking a per-file "latest save generation"). Alternatives: a single serialized worker queue (one compile at a time, coalescing rapid saves); or blocking the main loop up to the timeout (rejected — violates never-wedge). Decide: thread-per-save vs. single worker; how supersession/cancellation is signalled (generation counter à la M2b, or a per-file `AtomicU64`); whether an in-flight compile is killed on supersede or left to finish and discarded. **Two constraints added by the M3 drift check:** (a) *result plumbing* — the cloned `sender` is write-only toward the client; the worker has no channel back into `GlobalState`, so the persisted per-file compiler set (for later re-merges by `publish_file_diagnostics`) must live in shared state (`Arc<Mutex<HashMap<FileId, …>>>`) written by the worker, OR arrive via a second crossbeam channel with the main loop switching to `select!` over `connection.receiver` + the worker channel — decide which; and the worker must never `recv` from a `cancel_receiver` clone (emptiness tests only, main thread only). (b) *shutdown* — a worker holding a cloned `sender` past shutdown hangs `io_threads.join()` (the writer thread only exits when every sender clone is dropped): keep worker `JoinHandle`s and join them (bounded by the compile timeout) before `run()` returns, or signal abandonment + kill the child on shutdown.
2. **Which binary to invoke** (`compact compile` wrapper vs. bare `compactc`), and whether to pin the compiler with `+<VERSION>`. (Ties to Plan-time item 2.)
3. **Cross-file / un-attributable diagnostics.** When the compiler reports an error whose basename ≠ the saved file's basename (error inside a dependency), what do we do? Options: (a) attribute to the saved file at file-top as a single generic diagnostic ("compiler error in <basename>: <msg>"); (b) best-effort map the basename to a known/open `FileId` and publish there; (c) drop it. The milestone's spec §5 ("unparseable output becomes a single generic diagnostic at file top") suggests (a) for v1. Confirm.
4. **Formatting mechanism.** `compact format` writes in place and prints nothing. Recommended: write the **current buffer** to a temp file, run `compact format <temp>`, read it back, and return a single whole-document-replace `TextEdit` (or, if desired, a minimal diff). Decide: whole-document replace vs. computed minimal edits (whole-document is simpler and safe for FULL-sync; minimal edits reduce cursor/scroll churn but need a diff algorithm). Also decide: format the buffer (may be dirty/unsaved) — recommended — vs. the on-disk file. And whether to short-circuit with `--check` first to return an empty edit list when already formatted.
5. **Where the `(line, scalar-col) → byte` conversion lives.** Add a scalar-aware accessor to `analyzer-core::LineIndex` (reusable, keeps line-scanning in one place) vs. compute it locally in `analyzer-toolchain` by taking the source `&str`. Recommended: local helper in `analyzer-toolchain` first (smaller blast radius); promote to `LineIndex` only if a second consumer appears. (Drift-checked: merged `LineIndex` exposes no scalar-col API and no public line-start-byte accessor — `line_starts` is private — so "reuse a LineIndex byte API" is not an option without extending core.)
6. **Subprocess timeout implementation.** `std::process` has no built-in wait-with-timeout. Options: hand-rolled spawn + poll `try_wait` loop (as the test harness's `wait_with_timeout` already does) — no new dep; or a small crate (e.g. `wait-timeout`) — a new dependency (first build omits `--locked`). Recommended: hand-rolled, no new dep. Confirm, and decide whether to kill the child on timeout.
7. **Default timeout value.** Spec says "~30 s". Confirm the default and whether it is configurable in v1 (the milestone lists a "configurable timeout" but the v1 configuration surface in the design lists only path/compile-on-save/formatting toggles). Decide whether to add a `compileTimeoutMs` config key now or hardcode ~30 s.
8. **Formatting when the toolchain is absent, mid-session installed/removed.** Discovery happens at startup. Decide whether to re-discover on config change / on first use, or require a restart. Recommended: discover at startup and on `didChangeConfiguration`/`initialize` re-read; treat absence as "feature off + one-time notice".

---

## Shared Interfaces (single source of truth for cross-task signatures)

Implementers see only their own task; every cross-task name/type lives here. **All `analyzer-toolchain` types are plain byte-offset/plain-Rust — zero `lsp-types`.**

### analyzer-toolchain (new crate; public surface)

```rust
// discovery.rs
/// A discovered `compact` toolchain and its versions.
#[derive(Clone, Debug)]
pub struct Toolchain {
    /// Absolute path to the `compact` executable to invoke.
    pub compact_bin: std::path::PathBuf,
    /// Compiler/toolchain version, e.g. "0.31.1" (from `compact compile --version`).
    pub tool_version: String,
    /// Language version, e.g. "0.23.0" (from `compact compile --language-version`).
    pub language_version: String,
}
impl Toolchain {
    /// Discover the toolchain: explicit `override_path` first (may be a dir or a
    /// direct path to `compact`), then `PATH`. Returns `None` when no usable
    /// `compact` is found — the caller then leaves compile-on-save/formatting off.
    /// Runs the version queries once; a binary that fails the version query is
    /// treated as absent (never panics).
    pub fn discover(override_path: Option<&std::path::Path>) -> Option<Toolchain>;
}

// parse.rs
/// One compiler diagnostic as emitted, before position mapping. `line`/`col`
/// are 1-based as the compiler prints them; `col` is a Unicode-scalar column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawCompilerDiagnostic {
    pub file_basename: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedStderr {
    pub diagnostics: Vec<RawCompilerDiagnostic>,
    /// stderr content that did not match the `Exception:` grammar (non-empty ⇒
    /// the caller should surface a generic diagnostic at file top).
    pub unparsed: String,
}
/// Parse `compact compile --vscode` stderr, best-effort. Never panics.
pub fn parse_compiler_stderr(stderr: &str) -> ParsedStderr;

// locate.rs
/// Map a 1-based `line` + 1-based Unicode-scalar `col` within `source` to a byte
/// `TextRange`, snapping the end to the CST token starting at that offset when
/// one exists; otherwise a zero-width range at the mapped offset. `None` if the
/// position is out of bounds. Never panics on adversarial input.
pub fn locate(source: &str, line: u32, col: u32) -> Option<text_size::TextRange>;

// compile.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileStatus { Ok, CompileError, InvocationError, TimedOut }
#[derive(Clone, Debug)]
pub struct CompileOutcome { pub status: CompileStatus, pub stderr: String }
/// Run `compact compile --skip-zk --vscode [--compact-path <list>] <source> <scratch>`
/// with a wall-clock `timeout`, capturing stderr. stdout/stderr are captured
/// (never inherited). The child is killed on timeout. Never panics.
pub fn compile_file(
    tc: &Toolchain,
    source: &std::path::Path,
    scratch: &std::path::Path,
    search_path: &[std::path::PathBuf],
    timeout: std::time::Duration,
) -> CompileOutcome;

// format.rs
/// Format `text` by writing it to a temp file, running `compact format` on that
/// copy, and reading it back. Returns the formatted text, or `None` if the
/// toolchain is absent, the input has a syntax error, or the run fails/times
/// out. Never mutates any real project file; never panics.
pub fn format_source(tc: &Toolchain, text: &str, timeout: std::time::Duration) -> Option<String>;
```

### compact-analyzer binary (new/changed)

```rust
// config parsed from initializationOptions (server.rs, alongside import_search_path_from)
#[derive(Clone, Debug)]
pub(crate) struct ToolchainConfig {
    pub toolchain_path: Option<std::path::PathBuf>, // "toolchainPath"
    pub compile_on_save: bool,                       // "compileOnSave" (default true)
    pub formatting: bool,                            // "formatting" (default true)
}
fn toolchain_config_from(options: Option<&serde_json::Value>) -> ToolchainConfig;

// GlobalState gains:
//   toolchain: Option<analyzer_toolchain::Toolchain>   (discovered at startup)
//   toolchain_config: ToolchainConfig
//   toolchain_notice_shown: bool                        (one-time showMessage guard)
//   scratch_root: PathBuf                               (per-server compile scratch)
//   compiler_diagnostics: Arc<Mutex<HashMap<FileId, Vec<lsp_types::Diagnostic>>>>
//     (last compiler run per open file, for merge. Arc<Mutex<…>> — NOT a plain
//      field: the compile worker must write it, and GlobalState is owned by the
//      single-threaded main loop with no worker→loop channel; the cloned
//      `sender` only reaches the CLIENT. publish_file_diagnostics locks + merges.
//      Alternative: second channel + main-loop select! — Open Question 1.)
//   compile_workers: Vec<std::thread::JoinHandle<()>>   (or equivalent — joined/
//      signalled at shutdown so no sender clone outlives run(); see Global
//      Constraints "Never-hang shutdown")

// server.rs handlers:
//   "textDocument/didSave"      -> compile-on-save (Task 6)
//   "textDocument/formatting"   -> format via compact format (Task 7)
// capabilities: text_document_sync with save, document_formatting_provider (Tasks 6/7)
```

---

## File Structure

**Create:**
- `crates/analyzer-toolchain/Cargo.toml` — new feature-layer crate (deps: `analyzer-core`, `text-size`, `compactp_parser` for the parse in `locate` (use analyzer-core's `SyntaxNode`/`SyntaxKind` re-exports — M3's analyzer-ide convention — instead of a direct `compactp_syntax` dep), `tempfile` (runtime — `format_source`'s temp-file round-trip)).
- `crates/analyzer-toolchain/src/lib.rs` — module decls + re-exports.
- `crates/analyzer-toolchain/src/discovery.rs` — `Toolchain::discover`, version queries.
- `crates/analyzer-toolchain/src/parse.rs` — `parse_compiler_stderr`, `RawCompilerDiagnostic`, `ParsedStderr`.
- `crates/analyzer-toolchain/src/locate.rs` — `locate` (scalar-col → byte, token-snap).
- `crates/analyzer-toolchain/src/compile.rs` — `compile_file`, `CompileOutcome`, timeout/kill.
- `crates/analyzer-toolchain/src/format.rs` — `format_source` (temp-file round-trip).
- `crates/compact-analyzer/tests/lsp_toolchain.rs` — gated integration tests (compile-on-save + formatting), skip cleanly when `compact` absent.

**Modify:**
- `Cargo.toml` (workspace) — add `crates/analyzer-toolchain` to `members`; add `analyzer-toolchain = { path = … }` to `[workspace.dependencies]`.
- `crates/compact-analyzer/Cargo.toml` — depend on `analyzer-toolchain`.
- `crates/compact-analyzer/src/server.rs` — capabilities (save + formatting), `didSave` (split out of the grouped no-op arm) + `formatting` handlers, `ToolchainConfig`, discovery at startup, one-time notice, compiler-diagnostic merge/dedup (inside `publish_file_diagnostics`, alongside the parse + resolution sets), worker join/termination at shutdown, scratch dir lifecycle.
- `crates/compact-analyzer/tests/support/mod.rs` — harness extensions: a start variant with env overrides (scrub `PATH` for Task 9) and an initialize variant that passes `initializationOptions` (Tasks 6/7/9); neither exists today.
- `crates/compact-analyzer/src/lsp_utils.rs` — a `compiler_diagnostic_to_lsp` helper (source `"compactc"`) OR a `source` parameter on `diagnostic_to_lsp`; whole-document `TextEdit` helper.
- `crates/compact-analyzer/src/main.rs` — (no new module unless config helper is split out).
- `.github/workflows/ci.yml` — a new `toolchain` job that installs the real `compact` CLI and runs the gated tests.
- `.superpowers/milestones/README.md` + `milestone-status` memory — flip M4 → Done at the end.

---

## Task 1: Scaffold `analyzer-toolchain` crate + toolchain discovery/version query

**Files:**
- Create: `crates/analyzer-toolchain/Cargo.toml`, `crates/analyzer-toolchain/src/lib.rs`, `crates/analyzer-toolchain/src/discovery.rs`
- Modify: `Cargo.toml` (workspace members + workspace dep)

**Interfaces:** Produces `Toolchain` + `Toolchain::discover` (see Shared Interfaces). Consumes nothing but `std`.

**Context:** The crate is the M4 feature layer, mirroring `analyzer-ide`'s place in the graph. Discovery is: explicit override (a path to `compact`, or a directory containing it) → search `PATH`. Version numbers come from `compact compile --version` and `--language-version` (T1). A discovered-but-unrunnable binary is treated as absent (never fatal).

- [ ] **Step 1: Create the crate manifest.** `crates/analyzer-toolchain/Cargo.toml`:
  ```toml
  [package]
  name = "analyzer-toolchain"
  version.workspace = true
  edition.workspace = true
  rust-version.workspace = true
  license.workspace = true
  repository.workspace = true
  publish = false

  [dependencies]
  analyzer-core.workspace = true
  text-size.workspace = true
  compactp_parser.workspace = true
  # SyntaxNode/SyntaxKind come via analyzer-core's re-exports (M3 convention,
  # as analyzer-ide does) — no direct compactp_syntax dep.
  # tempfile is a RUNTIME dep: format_source round-trips through a temp file.
  tempfile.workspace = true
  ```
  Add `"crates/analyzer-toolchain"` to `[workspace] members` and `analyzer-toolchain = { path = "crates/analyzer-toolchain" }` to `[workspace.dependencies]`.

- [ ] **Step 2: Write a failing discovery test.** In `discovery.rs` `#[cfg(test)]`: a test that `Toolchain::discover(Some(<a temp dir with a fake executable `compact` that prints known versions>))` returns a `Toolchain` whose `tool_version`/`language_version` match the fake's output, and that `discover(Some(<empty dir>))` returns `None`. (Use a tiny shell/script shim written into a `tempfile::tempdir`, marked executable on Unix; `#[cfg(unix)]`-gate the exec-bit test.) **VERIFY** the exact version-query subcommands before finalizing (Plan-time item 1/2).

- [ ] **Step 3: Run it — expect failure** (`cargo test -p analyzer-toolchain discovery`). New crate: **first build omits `--locked`.**

- [ ] **Step 4: Implement `discover`.** Resolve the candidate `compact` path (override file → override dir/`compact` → `which`-style PATH scan via `std::env::var_os("PATH")` + `std::env::split_paths`). Run `<candidate> compile --version` and `<candidate> compile --language-version`, trimming stdout. Any spawn/exit failure ⇒ `None`. Capture stdout; never inherit. Keep it panic-free.

- [ ] **Step 5: Add a `lib.rs`** with `mod discovery; pub use discovery::Toolchain;` and module stubs (`mod parse; mod locate; mod compile; mod format;` added in later tasks — add now as empty files with `#![allow(dead_code)]` placeholders or add per-task; prefer per-task to avoid dead code).

- [ ] **Step 6: Gate + commit.**
  ```
  cargo fmt && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test -p analyzer-toolchain
  ```
  ```
  git add crates/analyzer-toolchain Cargo.toml
  git commit -m "feat(toolchain): analyzer-toolchain crate with compact discovery + version query"
  ```

---

## Task 2: Compiler stderr parser (`parse_compiler_stderr`)

**Files:** Create/modify `crates/analyzer-toolchain/src/parse.rs`; `lib.rs` re-exports.

**Interfaces:** Produces `RawCompilerDiagnostic`, `ParsedStderr`, `parse_compiler_stderr`. Consumes only `&str`.

**Context:** Parse the `--vscode` single-line grammar `Exception: <basename> line <N> char <C>: <message>` (T2). Best-effort: lines that don't match go into `unparsed`. The compiler is fail-fast (usually one line), but parse all matching lines defensively. **This is the task most exposed to upstream drift — fixture every case against real output at the pinned version (Plan-time item 3).**

- [ ] **Step 1: Write failing tests from REAL captured fixtures.** Paste actual stderr strings (re-captured at implementation time) as fixtures:
  - `"Exception: bad.compact line 7 char 8: operation incremen undefined for ledger field type Counter"` → one diag, basename `bad.compact`, line 7, col 8, message from `operation …`.
  - `"Exception: parse.compact line 1 char 1: parse error: found \"zzz\" looking for a program element or end of file"` → line 1, col 1, message begins `parse error:`.
  - A dependency-attributed line (`util.compact line 4 char 15: unbound identifier undefined_in_util`) → basename `util.compact`.
  - Non-matching junk (e.g. a `Usage:` block) → `diagnostics` empty, `unparsed` non-empty.
  - Empty stderr → default `ParsedStderr` (empty).

- [ ] **Step 2: Run — expect failure.**

- [ ] **Step 3: Implement `parse_compiler_stderr`.** For each line: strip an optional `Exception: ` prefix; match `<file> line <N> char <C>: <rest>` (the message is everything after the first `: ` following the `char <C>`). Use a hand-rolled scan or a small regex-free split (avoid adding `regex` — Open Question 6 spirit). Non-matching, non-blank lines accumulate into `unparsed`. Never panic; malformed numbers ⇒ skip to `unparsed`.
  - **VERIFY**: whether basenames can contain spaces (they can't in the corpus, but the parser should split on ` line ` / ` char ` markers rather than the first whitespace to be safe).

- [ ] **Step 4: Run — expect pass.**

- [ ] **Step 5: Gate + commit.**
  ```
  git commit -m "feat(toolchain): parse compact --vscode compiler diagnostics"
  ```

---

## Task 3: Position mapping (`locate` — scalar column → byte TextRange, token-snapped)

**Files:** Create/modify `crates/analyzer-toolchain/src/locate.rs`; `lib.rs` re-exports.

**Interfaces:** Produces `locate(source, line, col) -> Option<TextRange>`. Consumes `compactp_parser::parse` + analyzer-core's `SyntaxNode`/`SyntaxKind` re-exports for token-snapping; the source `&str`.

**Context:** Convert a 1-based `line` + 1-based **Unicode-scalar** `col` (T2) to a byte offset, then snap the end to the CST token at that offset (nicer squiggles than a zero-width point). **Do NOT reuse `LineIndex::offset` blindly — it expects a UTF-16 column, which differs from the compiler's scalar column for astral chars.** Implement scalar→byte by locating the line's start byte and advancing `col-1` `char`s (Rust `char` = Unicode scalar).

- [ ] **Step 1: Write failing tests (multibyte + astral).** Mirror the verified probes:
  - `locate("...\n  const x = undefined_name;\n", 2, 13)` lands on the `u` of `undefined_name`; the returned range covers the whole `undefined_name` token (token-snap).
  - A line containing `é` before the token: scalar col maps to the right byte (byte ≠ scalar).
  - A line containing `😀` (1 scalar, 4 bytes): scalar col maps correctly (the emoji-probe case).
  - Out-of-bounds line/col ⇒ `None`. col past end of line ⇒ clamp or `None` (decide + assert).

- [ ] **Step 2: Run — expect failure.**

- [ ] **Step 3: Implement `locate`.** (a) Find the byte offset of line `line`'s start by scanning for the `(line-1)`-th `\n`. (RESOLVED drift-check: merged `LineIndex` exposes NO public line-start/byte API — `line_starts` is private — so scan locally; see Open Question 5.) (b) From there advance `col-1` `char`s to get the start byte. (c) Parse `source` (`compactp_parser::parse`) once, build the `SyntaxNode`, and find the token whose range starts at (or contains) that offset; use its `text_range()` as the snapped range. **Clamp first — M3 invariant (E7/B1):** rowan's `token_at_offset` `assert!`-panics when `offset > root.text_range().end()`; guard/clamp exactly as merged `anchor_token`/`ident_at_offset`/`selection_ranges` do (`offset.min(root.text_range().end())` or an early `None`). When `token_at_offset` returns `Between(l, r)`, prefer the right token when it starts at the offset (the compiler points at token *starts*) — same preference `anchor_token` applies for IDENTs. (d) Fall back to a zero-width `TextRange::empty(start)` when no token matches. Panic-free on any input.
  - **Note:** this re-parses the source. The binary already has a cached parse (`host.analyze`); consider (Open Question) passing the CST in instead of re-parsing, to avoid a second parse per diagnostic. For the crate's standalone API, re-parsing keeps it self-contained; the binary can memoize.

- [ ] **Step 4: Run — expect pass.**

- [ ] **Step 5: Gate + commit.**
  ```
  git commit -m "feat(toolchain): map compiler (line, scalar-col) to byte TextRange with token snap"
  ```

---

## Task 4: Compile invocation with timeout (`compile_file`)

**Files:** Create/modify `crates/analyzer-toolchain/src/compile.rs`; `lib.rs` re-exports.

**Interfaces:** Produces `compile_file`, `CompileOutcome`, `CompileStatus`. Consumes `Toolchain`.

**Context:** Run `compact compile --skip-zk --vscode [--compact-path <list>] <source> <scratch>` (T2), capturing stderr, with a wall-clock timeout and child-kill on timeout (Open Question 6). Classify: exit 0 ⇒ `Ok`; 255 ⇒ `CompileError`; 1/usage ⇒ `InvocationError`; timeout ⇒ `TimedOut`. **Do not inherit stdio** (protocol safety). This task's tests are **gated** behind toolchain presence.

- [ ] **Step 1: Write a gated integration-style unit test.** Guard with `let Some(tc) = Toolchain::discover(None) else { eprintln!("compact not present; skipping"); return; };`. Then:
  - Compile a temp `.compact` file with a known error (e.g. an undefined identifier) into a temp scratch dir → assert `status == CompileError` and stderr contains `Exception:`.
  - Compile a valid file → `status == Ok`, stderr empty, scratch dir populated.
  - (Optional) a pathological infinite/slow case is hard to construct reliably — instead unit-test the timeout path with a fake `compact` shim (a script that `sleep`s) via a hand-built `Toolchain` pointing at the shim; assert `TimedOut` and that the call returns within ~timeout+ε. `#[cfg(unix)]`-gate the shim test.

- [ ] **Step 2: Run — expect failure** (function absent).

- [ ] **Step 3: Implement `compile_file`.** `std::process::Command::new(&tc.compact_bin).args(["compile","--skip-zk","--vscode"])` + optional `--compact-path <joined>` (VERIFY join separator per-OS via `std::env::join_paths`) + `<source>` + `<scratch>`; `.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null())`. Spawn, then poll `try_wait` against a deadline (mirroring the test harness's `wait_with_timeout`); on timeout `child.kill()` + `wait()` + return `TimedOut`. Read captured stderr. Map exit code → `CompileStatus` (VERIFY codes, Plan-time item 5).

- [ ] **Step 4: Run — expect pass (locally, toolchain present).**

- [ ] **Step 5: Gate + commit.**
  ```
  git commit -m "feat(toolchain): run compact compile --skip-zk --vscode with timeout"
  ```

---

## Task 5: Formatting round-trip (`format_source`)

**Files:** Create/modify `crates/analyzer-toolchain/src/format.rs`; `lib.rs` re-exports.

**Interfaces:** Produces `format_source(tc, text, timeout) -> Option<String>`. Consumes `Toolchain`.

**Context:** `compact format` writes in place + is stdout-silent (T3). To format the current buffer without touching real files, write `text` to a temp file (unique dir), run `compact format <temp>`, read the temp back. Failure (absent toolchain, syntax error → non-zero exit, timeout) ⇒ `None`. **Never touch any project file.** (Open Question 4 governs whether the binary turns this into a whole-document vs. minimal `TextEdit`.)

- [ ] **Step 1: Write a gated test.** Guard on `Toolchain::discover(None)`. Given deliberately-unformatted-but-valid source (extra spaces, per the verified probe), assert `format_source` returns `Some(formatted)` where `formatted != input` and is idempotent (`format_source(formatted) == formatted`). Given syntactically-broken source, assert `None`.

- [ ] **Step 2: Run — expect failure.**

- [ ] **Step 3: Implement.** `tempfile::Builder` → a temp dir + `X.compact`; write `text`; run `Command::new(&tc.compact_bin).args(["format", <temp>])` with piped stdio + timeout (reuse the poll loop from Task 4, consider extracting a shared `run_with_timeout` helper). On exit 0, read the temp file back → `Some`. On non-zero/timeout ⇒ `None`. Temp dir auto-cleans on drop. **VERIFY** stdout-silence and in-place semantics still hold (Plan-time item 6); if a `--stdout` flag exists, prefer it (no temp file).

- [ ] **Step 4: Run — expect pass.**

- [ ] **Step 5: Gate + commit.**
  ```
  git commit -m "feat(toolchain): format via compact format temp-file round-trip"
  ```

---

## Task 6: Binary — compile-on-save handler (didSave), tagged + merged diagnostics

**Files:** Modify `crates/compact-analyzer/src/server.rs`, `crates/compact-analyzer/src/lsp_utils.rs`, `crates/compact-analyzer/Cargo.toml` (add `analyzer-toolchain` dep).

**Interfaces:** Consumes `analyzer_toolchain::{Toolchain, compile_file, parse_compiler_stderr, locate}`; `lsp_utils::range_to_lsp`; F/T4 lsp-types save surface. Produces the `didSave` handler + a `compiler_diagnostic_to_lsp` helper (source `"compactc"`). Config comes from Task 8 (land Task 8 first if executing strictly in order, or forward-declare the config with defaults).

**Context:** On `didSave`, if the toolchain is present AND `compile_on_save` is on, compile the **saved file from disk** (spec: didSave means on-disk), parse stderr, map each diagnostic to a `TextRange` via `locate` (on the file's current source), tag as `compactc`, **dedup any compiler diagnostic whose span coincides with a native diagnostic** (keep native), and publish the merged set. Un-attributable (dependency-basename) diagnostics follow Open Question 3 (recommended: one generic diagnostic at file top of the saved file). **Runs off the main loop per Open Question 1 (recommended: worker thread publishing via a cloned `sender`, results keyed by saved version, stale-dropped).** Advertise save capability (T4).

- [ ] **Step 1: Write a gated integration test** in `crates/compact-analyzer/tests/lsp_toolchain.rs`. Guard: skip if `analyzer_toolchain::Toolchain::discover(None).is_none()`. Drive the real binary: initialize (with `compileOnSave: true` — the harness's `initialize`/`initialize_root` do NOT currently pass `initializationOptions`; add an initialize-with-options variant to `tests/support/mod.rs` first), `didOpen` a file with a **compiler-only** error (e.g. `count.incremen(1)` on a `Counter` — a semantic error native diagnostics can't catch), write it to disk at the doc's path, send `didSave`, then `wait_for_notification("textDocument/publishDiagnostics")` until a diagnostic with `source == "compactc"` appears at the expected line. Assert native syntax-only files still publish only `compact-analyzer`-sourced diagnostics.

- [ ] **Step 2: Run — expect failure** (didSave is a no-op today).

- [ ] **Step 3: Discovery + capability + config plumbing.** In `run()`: after stdlib setup, `state.toolchain = analyzer_toolchain::Toolchain::discover(state.toolchain_config.toolchain_path.as_deref());` and log the versions to stderr. (RESOLVED drift-check: there is NO existing language-version policy notice in the merged binary to feed `language_version` into — a user-facing mismatch notice would be new, optional scope; do not assume one.) Replace `text_document_sync` with the Options form advertising `save` (T4, VERIFY names) — the capability literal also carries M3's completion/semanticTokens/folding/selection entries; add save + `document_formatting_provider` alongside, don't rebuild it. Add the `GlobalState` fields from Shared Interfaces.

- [ ] **Step 4: Implement the `didSave` arm** (split `"textDocument/didSave"` out of the grouped no-op arm `"textDocument/didSave" | "initialized" | …` — don't remove the others). Parse `DidSaveTextDocumentParams`; look up `FileId` + source + `LineIndex` from `host`; if toolchain present + toggle on, launch the compile (Open Question 1 model). In the worker: `compile_file(&tc, disk_path, &scratch, &search_path, timeout)`; `parse_compiler_stderr`; for each `RawCompilerDiagnostic` whose basename matches the saved file, `locate(source, line, col)` → `TextRange` → `compiler_diagnostic_to_lsp` (source `"compactc"`, severity ERROR); un-attributable ones → one generic file-top diagnostic; on `TimedOut`/`InvocationError`/`unparsed` non-empty → one generic diagnostic at file top ("compiler unavailable / could not parse compiler output"). Merge with native diagnostics (dedup by coinciding span, keep native — natives are TWO sets: `analysis.diagnostics` + `host.resolution_diagnostics(file)`). Publish via the cloned `sender`. Store the compiler set in the shared `compiler_diagnostics` (`Arc<Mutex<…>>` — the worker cannot write a plain `GlobalState` field; see Shared Interfaces + OQ 1) and extend `publish_file_diagnostics` to lock + append it, so a later debounced native republish re-merges instead of wiping compiler diagnostics. **Guard the whole worker body with its own `catch_unwind` (it runs outside `dispatch`'s), and register the worker handle for shutdown join/termination (Global Constraints "Never-hang shutdown").**

- [ ] **Step 5: Add `compiler_diagnostic_to_lsp`** (or a `source` param on `diagnostic_to_lsp`) in `lsp_utils.rs`, sourced `"compactc"`. Add a unit test asserting the source string + severity.

- [ ] **Step 6: Run the integration test — expect pass (locally).**

- [ ] **Step 7: Gate + commit.**
  ```
  git commit -m "feat(server): compile-on-save diagnostics via compact, tagged and merged with native"
  ```

---

## Task 7: Binary — `textDocument/formatting` handler + capability

**Files:** Modify `crates/compact-analyzer/src/server.rs`, `crates/compact-analyzer/src/lsp_utils.rs`.

**Interfaces:** Consumes `analyzer_toolchain::format_source`; `lsp_types::{DocumentFormattingParams, TextEdit}`; a whole-document-range helper. Produces the `formatting` handler + capability.

**Context:** On `textDocument/formatting`, if toolchain present + `formatting` toggle on, format the **current buffer** (Open Question 4) via `format_source`, and return a single whole-document-replace `TextEdit` (or `[]` if unchanged / `null` if unavailable). Absent toolchain ⇒ `null` + one-time notice (Task 9).

- [ ] **Step 1: Write a gated integration test.** Skip if toolchain absent. `didOpen` an unformatted-but-valid buffer, request `textDocument/formatting`, assert the response is a `TextEdit[]` whose applied result equals the compiler-formatted text (from the verified probe's expected output — recapture at implementation time). Also assert an already-formatted buffer returns `[]` (or a no-op edit) and a broken buffer returns `null`/`[]` without error.

- [ ] **Step 2: Run — expect failure** (no handler/capability).

- [ ] **Step 3: Advertise capability.** `document_formatting_provider: Some(lsp_types::OneOf::Left(true))` in the capabilities literal.

- [ ] **Step 4: Implement the handler.** Parse `DocumentFormattingParams`; resolve open `FileId` + current buffer text + `LineIndex`; if toolchain + toggle: `format_source(&tc, &text, timeout)`; on `Some(formatted)` and `formatted != text`, return one `TextEdit { range: whole_document_range(&line_index, &text), new_text: formatted }`; on `Some(formatted) == text` return `Some(vec![])`; on `None` return `Some(vec![])` or `None` (decide) + trigger the one-time notice if the cause is "toolchain absent". Add `whole_document_range` to `lsp_utils` with a unit test — drift-check simplification: `range_to_lsp(li, TextRange::new(TextSize::new(0), TextSize::of(text)))` already yields `(0,0)`→last-line/col (merged `line_col` clamps the end offset correctly); no bespoke last-line arithmetic needed.

- [ ] **Step 5: Run — expect pass (locally).**

- [ ] **Step 6: Gate + commit.**
  ```
  git commit -m "feat(server): textDocument/formatting via compact format"
  ```

---

## Task 8: Binary — configuration surface (toolchain path, compile-on-save, formatting)

**Files:** Modify `crates/compact-analyzer/src/server.rs` (+ tests).

**Interfaces:** Produces `ToolchainConfig` + `toolchain_config_from(options)` (mirrors `import_search_path_from`). Consumed by Tasks 6/7/9.

**Context:** Completes the v1 configuration surface (design §4): reads `toolchainPath` (string), `compileOnSave` (bool, default true), `formatting` (bool, default true) from `initializationOptions`. (Whether to also add `compileTimeoutMs` is Open Question 7.) *If executing tasks strictly in order, land this before Tasks 6/7 or forward-declare with defaults.*

- [ ] **Step 1: Write failing unit tests** (mirror `import_search_path_reads_options`): given `{"toolchainPath":"/opt/compact","compileOnSave":false}` → the config reflects the path + toggle; given `{}` → defaults (no path, both toggles true).

- [ ] **Step 2: Run — expect failure.**

- [ ] **Step 3: Implement `toolchain_config_from`.** Parse the three keys with the existing `serde_json::Value` pointer/`get` idiom; defaults when absent.

- [ ] **Step 4: Wire it in `run()`** — parse from `initializationOptions` next to `import_search_path_from`, store on `GlobalState`, and use in discovery/handlers.

- [ ] **Step 5: Run + gate + commit.**
  ```
  git commit -m "feat(server): toolchain configuration (path, compile-on-save, formatting toggles)"
  ```

---

## Task 9: Binary — graceful degradation (one-time toolchain-absent notice)

**Files:** Modify `crates/compact-analyzer/src/server.rs`.

**Interfaces:** Consumes `lsp_types::{ShowMessageParams, MessageType, notification::ShowMessage}` (T4). Uses the existing `send_notification` helper. Produces a `notify_toolchain_missing_once` guard gated by `toolchain_notice_shown`.

**Context:** When a toolchain-requiring feature (compile-on-save trigger on save, or a formatting request) is used but no toolchain is present, send ONE `window/showMessage` (INFO) explaining how to enable it (install `compact` / set `toolchainPath`), then never again this session. Absence must NEVER produce error spam or per-request errors — the feature simply no-ops.

- [ ] **Step 1: Write an integration test** that runs the server with the toolchain deliberately hidden (Open Question: how to hide it deterministically — e.g. set `toolchainPath` to a dir with no `compact`, and/or run with a scrubbed `PATH` in the spawned child's env). `didSave` a file and request formatting; assert exactly one `window/showMessage` is emitted across both, and that diagnostics/format responses are still well-formed (no error responses). RESOLVED drift-check: the harness CANNOT currently control the child's env — `support::Client::start()` spawns with inherited env and no override hook — so extending `tests/support/mod.rs` with a `start_with_env`-style variant (and the initialize-with-`initializationOptions` variant from Task 6 Step 1) is a required sub-step, not a maybe.

- [ ] **Step 2: Run — expect failure.**

- [ ] **Step 3: Implement `notify_toolchain_missing_once`** — if `!toolchain_notice_shown`, send the `showMessage`, set the flag. Call it from the `didSave` and `formatting` arms when `toolchain.is_none()` and the relevant toggle is on.

- [ ] **Step 4: Run + gate + commit.**
  ```
  git commit -m "feat(server): one-time notice when compact toolchain is absent"
  ```

---

## Task 10: Never-die corpus/robustness pass for the toolchain path

**Files:** Modify `crates/compact-analyzer/tests/corpus_smoke.rs` (or add a toolchain-scoped test), and/or `analyzer-toolchain` unit tests.

**Context:** Ensure the parser + `locate` never panic on adversarial/garbled compiler output and on error-recovered CSTs, mirroring M3's corpus sweep. This is a pure/native pass (no subprocess) so it runs in CI without the toolchain.

- [ ] **Step 1: Add a fuzz-ish test** feeding `parse_compiler_stderr` a battery of malformed strings (truncated `Exception:`, negative/huge numbers, non-UTF-8-ish escapes, empty, only-`Usage:`), asserting no panic and sane `ParsedStderr`.
- [ ] **Step 2: Add a `locate` sweep** over the corpus files (M3's `corpus_dir()` helper exists but is private to `tests/corpus_smoke.rs` — add the sweep to that file, or duplicate the small helper in a toolchain-scoped test) calling `locate` at random-ish `(line, col)` including out-of-bounds, asserting `None` or in-bounds ranges, never a panic. Guard the sweep so one file can't abort the run (never-die), matching M3's `m3_features_never_panic_on_corpus` (confirmed present on merged main).
- [ ] **Step 3: Run + gate + commit.**
  ```
  git commit -m "test(toolchain): parser + locate never panic on adversarial input and corpus"
  ```

---

## Task 11: CI — toolchain integration job

**Files:** Modify `.github/workflows/ci.yml`.

**Context:** Add one job that installs the real `compact` CLI and runs the gated integration tests end-to-end (compile-on-save + formatting), per design §8. The existing `test` matrix runs WITHOUT the toolchain (gated tests self-skip); this new job is the one place the real CLI is exercised.

- [ ] **Step 1: Add a `toolchain` job** (ubuntu-latest): checkout → install Rust → install `compact` (VERIFY the official install method — the `midnight-tooling:install-cli` skill / the documented installer script; the plan MUST NOT hardcode an unverified install URL). Set up `PATH`/`COMPACT_DIRECTORY` as the installer requires. Run `cargo test -p compact-analyzer --test lsp_toolchain --locked` (+ `cargo test -p analyzer-toolchain --locked`). **VERIFY** the installer is non-interactive and pins a language-0.23-compatible compiler.
- [ ] **Step 2: Ensure gated tests self-skip cleanly** when the job's toolchain install is unavailable (they already guard on `discover`), so a flaky install fails loudly only in this job, not the whole matrix.
- [ ] **Step 3: Commit.**
  ```
  git commit -m "ci: install compact CLI and run toolchain integration tests"
  ```

---

## Self-review notes (deliberate departures / decisions to confirm)

1. **`analyzer-toolchain` is a feature-layer crate parallel to `analyzer-ide`, not part of `analyzer-core`.** The design (§3.1) lists it as a top-level crate; this plan keeps the compiler-facing subprocess concerns out of core (which stays pure/in-process) and out of `analyzer-ide` (which stays lsp-free feature logic). `analyzer-toolchain` depends on `analyzer-core` only for shared value types.
2. **We invoke `compact compile`, not `compactc`, contradicting the milestone/spec wording.** Verified: the installed `compactc` shim isn't callable out of context; the wrapper handles version selection. Flagged as VERIFY + Open Question 2 in case a target environment ships a bare `compactc`.
3. **Formatting round-trips a temp copy of the buffer** because `compact format` writes in place and prints nothing (verified). This is more machinery than "shell out to `compact format`" implies, but it is the only way to honour LSP's "return edits, don't mutate my file" contract for a possibly-unsaved buffer. Open Question 4.
4. **Compiler `char` is a Unicode-scalar column** (verified with an emoji probe), so `locate` must NOT reuse `LineIndex`'s UTF-16 `offset`. This is the single most drift-prone, most load-bearing fact in the plan.
5. **Compile-on-save runs off the main loop** (worker thread), unlike every M3 feature (which were fast, synchronous, no-cancellation). The concurrency/staleness model is the biggest genuine design decision (Open Question 1) and is intentionally left for the human rather than guessed.
6. **The compiler is fail-fast (one diagnostic per run)** — the plan deliberately avoids building multi-error accumulation and keeps dedup simple.

## Errata

_(During execution, when a step's own code/fixture/API turns out wrong, fix it empirically, record the correction here with the task number and root cause, and commit the Errata edit promptly — uncommitted plan edits get reverted by implementer subagents' git operations.)_

- _(none yet — draft)_

## Definition of Done

- All 11 tasks complete; every task's gate (`cargo fmt` + `cargo clippy --workspace --all-targets --locked -- -D warnings` + tests) green; per-task commit landed.
- **All Open Questions resolved with the human** and reflected in the code (especially 1 concurrency, 3 attribution, 4 formatting mechanism, 7 timeout/config).
- **All Plan-time verification items re-checked** against the pinned `compact` toolchain, with fresh fixtures committed for the parser and (where feasible) the gated integration tests.
- **Optionality invariant proven:** with no toolchain and no `toolchainPath`, the server behaves exactly as end-of-M3 (a test asserts this), and toolchain-absent use emits exactly one `window/showMessage`, never error spam.
- **Layering preserved:** `analyzer-toolchain` has zero `lsp-types`; no `[patch.crates-io]` committed; version pins unchanged.
- **Shutdown never hangs:** `shutdown`/`exit` completes promptly with a compile in flight (a test saves then immediately shuts down; the harness's `wait_with_timeout` bounds it) — no `sender` clone outlives `run()`.
- Gated integration tests pass locally (toolchain present); self-skip in the default CI matrix; the new `toolchain` CI job installs the real CLI and exercises compile-on-save + formatting E2E.
- Test count risen from the M3 baseline of **196** (per `.superpowers/sdd/progress.md` at the M3 merge) (new: ~3 discovery, ~5 parser, ~4 locate, ~2 compile[gated], ~2 format[gated] unit; ~1 config, ~1 lsp_utils unit; ~2 compile-on-save, ~2 formatting, ~1 degradation integration[gated]; ~2 never-die) — record the exact final count in `.superpowers/sdd/progress.md`.
- Final whole-branch review on the most capable model (base = `git merge-base main HEAD`); one fix subagent for any Critical/Important findings; re-verify.
- `superpowers:finishing-a-development-branch` → fast-forward merge to `main` locally, then push.
- Update the `milestone-status` memory and flip **M4 → Done** in `.superpowers/milestones/README.md` (commit the flip on the branch so it merges with M4).

---
## Resolved Open Questions (human-approved 2026-07-10)
1. **Worker result plumbing → `Arc<Mutex<HashMap<FileId, Vec<Diagnostic>>>>`** shared state (the applied default). Simpler than a second crossbeam channel + main-loop `select!`; keeps the M3 single-threaded main loop mostly intact; mutex contention is negligible at compile-on-save cadence.
2. **Shutdown policy → FAST EXIT.** On shutdown, kill the in-flight child compiler process(es); do NOT `join()`-block `io_threads` on compile workers, and workers must drop their `sender` clones promptly so `io_threads.join()` returns. Honors the "never-hang shutdown" invariant; a worst-case ~30s exit wait is unacceptable editor UX. (Rejected: join-and-wait.)
3. **Compiler diagnostics vs per-file debounce → BYPASS for the compiler-on-save set** (`didSave` is already a natural throttle); only the merged native+compiler republish is debounced. (The draft's default.)
