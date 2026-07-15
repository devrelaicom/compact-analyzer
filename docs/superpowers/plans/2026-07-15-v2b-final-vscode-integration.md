# v2b.final — VS Code Type-Diagnostic Integration — DRAFT / OUTLINE (not execution-ready)

> ⚠️ **DRAFT OUTLINE — DO NOT EXECUTE AS-IS.** Forward-context sketch. **Not** a
> bite-sized, execution-ready plan.
>
> **This outline is highly likely to change** based on decisions made while building the
> v2b rule plans (v2b.3…v2b.8) — the diagnostics they emit, the codes they use, and how
> native type diagnostics interleave with M4's compile-on-save diagnostics.
>
> **Before turning this into an actual implementation plan, the executing agent MUST:**
> 1. Re-read the v2 program spec §8 and the v2b foundation design §6.
> 2. Review the **as-merged** rule diagnostics and the existing extension wiring (start
>    with `crates/compact-analyzer/src/server.rs`, `lsp_utils.rs`, `editors/vscode/`, and
>    M4's dual-source diagnostic tagging) — checking for **drift**.
> 3. Run `superpowers:writing-plans` (and `superpowers:brainstorming` first if drift is
>    material) to produce the real plan.

**Goal (program spec §8, foundation spec §6):** Close v2b's release loop — native type
diagnostics appear in the VS Code **Problems panel**, verified **in-editor** (not merely
over LSP integration tests), with a `compact-analyzer.*` **native-vs-compiler diagnostic
preference** toggle following the existing config + restart-to-apply pattern.

**Architecture (provisional):** The server already publishes diagnostics; native type
diagnostics (tracked `Arc<[Diagnostic]>` queries from the rules) merge with M4's on-save
`compactc` diagnostics through the existing dual-source tagging so users see one
vocabulary. The thin VS Code client largely inherits this; the real work is the merge
policy, the preference setting, and in-editor verification.

## Global Constraints (inherited — reconfirm at plan time)

- **In-editor verification is the done-bar** (program spec §8): the VS Code extension is
  how most users interact with the project; "works over raw LSP" is not "works for users."
- No LSP types in `analyzer-core`; protocol conversion stays in the `compact-analyzer`
  binary.
- Follow the established config + restart-to-apply pattern (v1/M5) for the toggle.
- Diagnostic wording tracks compiler wording where practical (one vocabulary across native
  + on-save).

## Provisional phase outline (each becomes several TDD tasks)

1. **Merge policy.** How native type diagnostics combine with M4's on-save diagnostics
   (dedup, source tagging, precedence) so the same error is not double-reported.
2. **`compact-analyzer.*` preference toggle.** Native-vs-compiler diagnostic preference
   setting; wire through server config; restart-to-apply.
3. **VS Code client wiring.** Confirm the Problems panel surfaces native diagnostics with
   correct source labels; add any client config the toggle needs.
4. **In-editor verification.** Exercise the features in the actual extension (manual
   checklist and/or an automated extension test) to satisfy the §8 done-bar — not only LSP
   request/response tests.

## Key open questions to resolve at plan time

- **Exact setting name(s)** — align with existing `compact-analyzer.*` config conventions.
- **Merge precedence** — when native and `compactc` disagree on a span, which wins in the
  panel, and how is the source tagged (v1's dual-source tagging shows compiler ground truth
  on save — preserve that).
- **Verification method** — manual checklist vs automated VS Code extension test harness.
