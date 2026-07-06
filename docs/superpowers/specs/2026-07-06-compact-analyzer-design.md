# compact-analyzer — Design

**Date:** 2026-07-06
**Status:** Approved
**Author:** Aaron Bassett (with Claude)

A rust-analyzer equivalent for the Compact smart contract language (Midnight Network): a native language server plus a first-party VS Code extension, built on the [compactp](https://github.com/devrelaicom/compactp) parser frontend.

## 1. Context and motivation

- The official Compact VS Code extension is being sunsetted. It provided only a TextMate grammar, snippets, and regex problem-matchers over compiler stderr — no language intelligence. Its disappearance leaves Compact developers with nothing turnkey.
- No official language server exists. The official Zed extension reserves a binary name (`compact-language-server`) that has never shipped. The closest community effort (`1NickPappas/compact-lsp`) is experimental and delegates diagnostics to the compiler on save.
- The compiler (`compactc`, Chez Scheme) offers no JSON diagnostics, no AST dump, and no check-only mode. Plain-text `Exception: <file> line N, char C: <msg>` errors are the only machine-consumable surface (`--vscode` collapses them to single lines; `--skip-zk` skips proving-key generation for faster runs). All real-time intelligence must therefore be native.
- compactp already provides the hard bottom half of a rust-analyzer architecture: a hand-written, event-based, error-tolerant recursive-descent parser over a lossless rowan CST, a typed AST, and span-carrying structured diagnostics — effectively complete for language 0.23 (validated against a ~486-file corpus from the compiler repo), published on crates.io, fuzzed, MIT.
- Compact's grammar is small (~70 nonterminals) but its semantics are unusually rich for its size: a `Uint<0..n>` subtype lattice, mandatory explicit generic specialization, ledger ADTs with special method surfaces, witness/TypeScript cross-language binding, and disclosure rules that amount to interprocedural taint analysis. That semantic depth is where an analyzer adds value no existing tool has.

## 2. Decisions (settled during brainstorming)

| Question | Decision |
|---|---|
| Positioning | Fully independent branding: the binary is `compact-analyzer`; the reserved `compact-language-server` name is ignored |
| Foundation | Depend on compactp crates (`compactp_syntax`, `compactp_parser`, `compactp_ast`, `compactp_diagnostics`) — git dependencies during heavy iteration, crates.io releases at milestones. Parser improvements land upstream in compactp |
| Semantic depth | Staged to full native: v1 name resolution, v2 native type checking, v3 native disclosure analysis |
| Clients | Server + first-party VS Code extension (Marketplace + Open VSX). Other editors get setup docs only |
| Toolchain dependency | Optional with graceful degradation: toolchain present adds compile-on-save diagnostics and formatting; absent, everything native still works |
| v1 release bar | Daily-driver dogfood: a real multi-file Compact project is comfortably developed in VS Code with it every day |
| Core architecture | Memoized recompute (Approach A): content-hash parse cache, dirty-flagged workspace index, generation-counter cancellation. Query-shaped APIs so salsa could replace the engine later without re-architecture |

## 3. Architecture

### 3.1 Workspace layout

```
crates/
  analyzer-core/       # document store (VFS), parse cache, name resolution,
                       # workspace symbol index, cancellation primitives
  analyzer-ide/        # editor-agnostic features: goto-def, completion, hover,
                       # rename, references, symbols, folding, semantic tokens
  analyzer-toolchain/  # optional `compact` CLI integration: discovery, version
                       # queries, compile-on-save diagnostics, format
  compact-analyzer/    # the binary: LSP server (stdio), UTF-16 line index,
                       # lsp-types mapping, request scheduling
editors/
  vscode/              # first-party VS Code extension (TypeScript)
```

### 3.2 Key choices

- **LSP framework:** `lsp-server` + `lsp-types` (the crates rust-analyzer itself uses). Synchronous main loop dispatching to a small thread pool. No async runtime.
- **VFS overlay:** open editor buffers shadow disk files. Compact's `include`/`import` can reference files that aren't open; the analyzer reads those from disk, but unsaved edits in open buffers always win.
- **Incrementality engine:** per-file parse results cached by content hash; a workspace item index (every declaration: name, kind, span, signature) maintained with per-file dirty flags; edits bump a generation counter that long-running computations poll for cancellation.
- **Layering rule:** `analyzer-ide` speaks byte offsets and plain Rust types — zero LSP types. Only the `compact-analyzer` binary knows about LSP and UTF-16. The feature layer is fully testable without a protocol harness.

### 3.3 Data flow

**Typing:** `didChange` → VFS update → parse-cache invalidation for that file → dirty-flag propagation in the item index → debounced (~150 ms) native syntax diagnostics published. Feature requests query `analyzer-ide`, which pulls from core, recomputing only dirty entries.

**Save:** `didSave` → if toolchain detected → run `compactc --skip-zk --vscode` against a scratch output directory → parse single-line diagnostics from stderr → publish merged with native diagnostics, tagged by source (`compact-analyzer` vs `compactc`), deduplicating compiler diagnostics that cover the same span as a native one.

### 3.4 Name resolution (v1 scope)

- Per-file lexical scopes: top-level, module-level (`export` visibility), generic parameters, circuit/constructor parameters, block-level `const`, with shadowing.
- Cross-file resolution implementing the compiler's search order: relative to the importing/including file first, then each entry of the configured import path (mirrors `COMPACT_PATH` / `--compact-path`).
- `include` treated as the textual splice it is, for resolution purposes, without physically splicing buffers.
- Import forms: bare `import M;`, selective `import { a, b as c } from M;`, and prefix `import M prefix M$;`.

### 3.5 Standard library modeling

`import CompactStandardLibrary` resolves to a bundled, signature-only stub file (~58 items: ~13 structs, ~45 circuits) with doc comments, shipped inside the binary and parsed by compactp like ordinary source. Stubs are versioned per language version; v1 bundles 0.23. Stub generation/curation notes live alongside the stubs so they can be refreshed each language release.

## 4. v1 feature set

| Feature | v1 behavior |
|---|---|
| Diagnostics (real-time) | compactp syntax errors as you type, debounced |
| Diagnostics (on save) | full compiler errors via `compactc --skip-zk --vscode` when toolchain present |
| Go-to-definition | item index + scope resolution; cross-file through imports/includes |
| Find references / rename | reverse index lookup; rename is workspace-wide and refuses conflicts (existing name in scope, keywords) |
| Completion | context-aware: keywords by position, in-scope symbols, stdlib items, struct fields in literals, and ledger ADT methods (`counter.` → `increment`, …) — ledger fields are explicitly typed at declaration, so no type checker is needed |
| Hover | declaration signature + doc comment; bundled docs for stdlib items |
| Document symbols | outline from the item tree |
| Workspace symbols | from the workspace index |
| Folding / selection ranges | derived from the CST |
| Semantic tokens | syntax-level classification from the CST (keywords, types, circuits, witnesses, ledger fields) |
| Formatting | shell out to `compact format`; no-op with notice when toolchain absent |

**Explicit v1 non-goals:** signature help, code actions, inlay hints (v2, once types exist); native type checking; disclosure analysis; witness-to-TypeScript navigation; first-party Zed/Neovim plugins (docs only); debugging support.

**Configuration surface (v1):** import search path (defaults to `COMPACT_PATH` when set), toolchain location override, compile-on-save toggle, formatting toggle.

## 5. Resilience and error handling

- **The server never dies on bad input.** Every request handler runs under `catch_unwind`; a panic fails that one request with an LSP error and a log line — the session continues.
- **Toolchain calls are sandboxed failures:** configurable timeout (default ~30 s) on compile-on-save; non-zero exits parsed best-effort; unparseable output becomes a single generic diagnostic at file top; a missing toolchain disables compile-on-save/formatting with a one-time status message explaining how to enable them.
- **Stale-result protection:** results are tagged with the document version they were computed against; responses for outdated versions are dropped.

## 6. Language-version policy

The analyzer targets the latest language version (0.23 at time of writing; compiler 0.31.x). It parses `pragma language_version` constraints; a file demanding an unsupported version still gets parsing and navigation, plus an info-level diagnostic stating the supported version and that semantic accuracy is not guaranteed. One grammar per release — no multi-version grammar multiplexing in v1. Stdlib stubs carry the language version they model. The upstream cadence is roughly monthly compiler releases with slower language drift; tracking latest with a graceful notice is the deliberate trade-off.

## 7. VS Code extension and distribution

- **Extension:** `vscode-languageclient` over stdio. Server acquisition order: user-configured path → PATH → download the platform binary for the extension's pinned server version from GitHub Releases (checksum-verified) into extension storage. Extension and server release in lockstep; the extension refuses server versions outside its pinned compatible range (for 0.x releases, the minor version must match).
- TextMate grammar, language-configuration, and snippets ship in the extension so highlighting works before/without the server (adapted from the official extension's Apache-2.0 assets, with attribution), progressively superseded by semantic tokens.
- Published to VS Code Marketplace and Open VSX.
- **Server distribution:** cargo-dist on GitHub Releases (macOS arm64/x64, Linux x64, Windows x64), shell/PowerShell installers, Homebrew tap — the same pipeline compactp uses. Workspace crates stay unpublished on crates.io until there is a concrete reason; the binary is the product.

## 8. Testing strategy

- **analyzer-core / analyzer-ide:** fixture-based unit tests using rust-analyzer's marker convention (`$0` cursors in inline fixtures), snapshot-tested with `expect-test`.
- **Corpus smoke:** compactp's ~486-file corpus run through indexing, resolution, and every IDE feature, asserting no panics and no out-of-bounds spans.
- **LSP integration:** black-box tests driving the real binary over stdio — initialize, open, edit; assert published diagnostics and request/response correctness, including UTF-16 position mapping with explicit multibyte/emoji fixtures.
- **Toolchain integration:** tests gated behind toolchain detection (skip cleanly when absent) plus one CI job that installs the real `compact` CLI and exercises compile-on-save and formatting end-to-end.
- **Extension:** minimal activation smoke test; primary validation is daily dogfooding.

## 9. Roadmap beyond v1 (direction, not commitment)

- **v2 — native type checking:** the `Uint<0..n>` subtype lattice, generic specialization, ledger ADT typing, tuple/vector covariance → real-time type errors, type-aware completion, signature help, inlay hints.
- **v3 — native disclosure analysis (the flagship):** real-time interprocedural taint tracking of witness-derived values, with the witness-to-sink path rendered as related-information locations matching compiler wording, plus a quick-fix code action to insert `disclose()`.
- **Opportunistic:** incremental reparse and expression-position recovery hardening land upstream in compactp as they become bottlenecks; witness/TypeScript cross-language navigation.

## 10. Risks

- **Language churn:** monthly compiler releases; renames have happened (`CurvePoint` → `NativePoint`). Mitigated by the tracking-latest policy, versioned stdlib stubs, and compactp's corpus-driven validation against upstream tags.
- **compactp gaps under mid-keystroke editing:** expression-position error recovery has a known rough edge (silent progress-failure in `expr_bp`). Mitigated by hardening upstream when dogfooding surfaces it.
- **Diagnostic drift vs the compiler:** native diagnostics may disagree with `compactc`. Mitigated by the dual-source tagging (users can see which engine said what) and treating the compiler as ground truth on save.
- **Prior-art overlap:** `1NickPappas/compact-lsp` covers similar ground. Studied as prior art; no code dependency planned (different foundation — it delegates semantics to the compiler).
