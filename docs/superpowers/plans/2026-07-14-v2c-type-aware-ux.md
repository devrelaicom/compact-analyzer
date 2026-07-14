# v2c — Type-Aware Editor UX — DRAFT / OUTLINE (not execution-ready)

> ⚠️ **DRAFT OUTLINE — DO NOT EXECUTE AS-IS.** Forward-context sketch captured alongside
> the v2 program spec. **Not** a bite-sized, execution-ready plan.
>
> **This outline is highly likely to change** based on implementation decisions made while
> building the prior sub-milestones — especially **v2b (the type system)**, whose type
> queries and `Ty` model are the sole data source for everything here. If v2b modelled
> types or exposed inference differently than assumed below, this outline is wrong in the
> ways that matter.
>
> **Before turning this into an actual implementation plan, the executing agent MUST:**
> 1. Re-read the v2 program spec `docs/superpowers/specs/2026-07-14-v2-native-type-checking-design.md`.
> 2. Review the **as-merged** implementation of v2a and v2b (start with the type queries
>    in `crates/analyzer-core/src/db.rs` / `src/ty/` and the existing
>    `crates/analyzer-ide/src/completion.rs`) and the current project state — checking for
>    **drift** between what this outline assumes and what exists.
> 3. Run `superpowers:writing-plans` (and `superpowers:brainstorming` first if drift is
>    material) to produce the real, TDD, bite-sized plan.

**Goal (from spec §6):** Three type-aware editor features — **type-aware completion**,
**signature help**, and **inlay hints** — all pure consumers of v2b's checker, delivered
and verified inside the VS Code extension.

**Architecture (provisional):** Each feature is an `analyzer-ide` function that queries
v2b types, plus an LSP capability the server advertises and a handler in `server.rs`, plus
thin VS Code client wiring. No new analysis engine — these read the checker.

**Tech Stack:** Rust (`analyzer-ide`, `compact-analyzer` server), `lsp-types = 0.95.1`,
`vscode-languageclient` (thin client in `editors/vscode/`).

## Global Constraints (inherited — reconfirm at plan time)

- **In-editor verification is the done-bar** (spec §8): each feature must be advertised by
  the server *and* verified working in the actual VS Code extension — not merely via LSP
  request/response integration tests.
- No LSP types in `analyzer-core`; IDE features live in `analyzer-ide`, protocol conversion
  in the `compact-analyzer` binary.
- Follow existing capability-registration patterns in `server.rs` (`initialize` response)
  and the semantic-tokens/completion precedents already in the tree.

## Provisional file structure

- New: `crates/analyzer-ide/src/signature_help.rs`, `crates/analyzer-ide/src/inlay_hints.rs`.
- Modify: `crates/analyzer-ide/src/completion.rs` — add type-aware member completion +
  type-informed ranking on top of M3's syntactic completion.
- Modify: `crates/compact-analyzer/src/server.rs` — advertise `signatureHelpProvider`
  (trigger chars `(` and `,`) and `inlayHintProvider` in `initialize`; add
  `textDocument/signatureHelp` and `textDocument/inlayHint` request handlers (mirror the
  existing `textDocument/*` arms around `server.rs:330-549`).
- Modify: `crates/compact-analyzer/src/lsp_utils.rs` — offset↔LSP conversions as needed.
- Modify: `editors/vscode/` — inlay-hint enablement setting (VS Code gates inlay hints
  behind a client setting); confirm signature-help/completion need no extra client config
  beyond capability advertisement.

## Phase outline (coarse — each becomes several TDD tasks in the real plan)

1. **Type-aware completion.** Member access on struct-typed and ADT-typed values (resolve
   the receiver's `Ty` via v2b, offer its members), and type-informed ranking. Extends
   M3's completion; verify the trailing-dot recovery path (M3 spec §1) still drives it.
2. **Signature help.** Compute active parameter at a call site from the callee's checked
   signature; advertise the capability + trigger characters; handle the request; verify
   in-editor.
3. **Inlay hints.** Types of `const` bindings (and similar) from v2b inference; advertise
   the capability; handle the request; wire + verify the VS Code enablement toggle.

## Key open questions to resolve at plan time

- **Ranking model:** how completion uses types to rank (exact-type-match first? assignable?)
  — depends on the shape of v2b's assignability/lattice queries.
- **Signature-help source:** does v2b expose a callee-signature query directly, or does v2c
  derive it from the resolver + `Ty`? Decide against what v2b actually shipped.
- **Inlay-hint verbosity + client toggle:** which positions get hints, and the exact
  `compact-analyzer.*` / VS Code setting names — align with the config conventions
  established in v1/M5 and the v2b diagnostic toggle.
- **In-editor verification method:** how each feature is actually exercised in the extension
  (manual checklist vs an automated extension test) to satisfy the spec §8 done-bar.
