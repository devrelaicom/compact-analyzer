# VS Code extension dogfood checklist

A manual smoke test of the packaged VSIX against a **real, multi-file Compact
project**. The automated E2E suite (`npm run test:e2e`) already machine-covers
activation, live diagnostics, and the toolchain-unavailable path; this
checklist covers the interactive surface a human has to eyeball.

Run it against the packaged VSIX produced by
`cd editors/vscode && npx vsce package --no-dependencies`.

## Setup

1. Install the packaged extension into a clean VS Code:

   ```sh
   code --install-extension editors/vscode/compact-analyzer-0.1.0.vsix
   ```

2. Open a multi-file Compact project (several `.compact` files with at least one
   cross-file `import`).
3. Open the **Output** panel → select **"Compact Analyzer"** from the dropdown.
   Confirm the startup line names the server binary and reports the toolchain
   (`compact toolchain <version> ... at <path>` when present).

## Core language features (no toolchain needed)

4. **Open / edit / save** a `.compact` file — the extension activates and the
   file is recognised as Compact (bottom-right language indicator).
5. **Live diagnostics:** introduce a syntax error — a squiggle appears as you
   type, with no save required; fix it and the squiggle clears.
6. **Goto-definition (cross-file):** invoke Go to Definition on a symbol
   imported from another file — the editor jumps to the definition in the other
   file.
7. **Find references (cross-file):** invoke Find All References on a
   widely-used symbol — references from multiple files are listed.
8. **Rename (cross-file):** rename a symbol used across files — all usages
   update, in every file.
9. **Hover:** hover a symbol — a hover card with its information appears.
10. **Completion:** trigger completion mid-identifier — relevant candidates
    appear.
11. **`.`-triggered completion:** type `.` after an expression — member
    completions appear (ledger ADT methods, struct fields, enum variants, and
    import-prefix/alias completions where applicable).
12. **Document symbols:** open the Outline view / "Go to Symbol in File" — the
    file's symbols are listed. Try "Go to Symbol in Workspace" for workspace
    symbols.
13. **Semantic tokens:** confirm identifiers are coloured by semantic role
    (not just the TextMate grammar) — e.g. types vs. functions vs. variables.
14. **Folding:** confirm fold arrows appear on blocks and fold/unfold works.
15. **Selection ranges:** use Expand/Shrink Selection — the selection grows and
    shrinks along syntactic boundaries.

## Toolchain features (Compact toolchain present)

16. Ensure the `compact` CLI is installed and discoverable (on `PATH`, or set
    `compact-analyzer.toolchainPath`), then restart the server
    (**"Compact Analyzer: Restart Server"**). Confirm the Output channel now
    reports the toolchain path.
17. **Compile-on-save diagnostics:** introduce a *semantic* error the syntax
    layer would not catch (one only `compactc` flags), then save — a compiler
    diagnostic appears (sourced from `compactc`). Fix and save — it clears.
18. **Formatting:** run **Format Document** on a deliberately messy but valid
    file — it reformats via `compact format`. Run it again on an
    already-formatted file — nothing changes (a no-op format means the file is
    already formatted, or the toolchain is disabled/absent — see step 20).

## Toolchain-absent behaviour

19. **One-time notice:** in a fresh session with the toolchain **off** `PATH`
    (scrub `PATH`, or set `compact-analyzer.toolchainPath` to a bogus value) and
    restart the server. Confirm exactly **one** informational notice is shown
    that toolchain features are disabled, and that core features (steps 4–15)
    still work. The Output channel should read
    `no compact toolchain found; compile-on-save disabled`.
20. Confirm **Format Document** in this state is a no-op (no error, no change) —
    the same observable outcome as "already formatted".

## Server lifecycle

21. **Restart command:** run **"Compact Analyzer: Restart Server"** from the
    command palette — the server restarts cleanly (watch the Output channel for
    a fresh startup line) and features keep working.
22. **Settings-change prompt:** change a `compact-analyzer.*` setting — confirm
    the extension offers to restart the server, and that accepting applies the
    change.

## Acquisition / download path

23. **Deferred until a real release exists.** Once the first GitHub Release is
    published (with a real `server-manifest.json` and checksums), verify the
    clean-machine acquisition path: with no `serverPath` set and no binary on
    `PATH`, the extension downloads the matching server, caches it, verifies its
    checksum, and starts it. Until then this step is not runnable (the dev
    manifest carries placeholder checksums and the E2E never downloads).
