# M4 — Toolchain integration (analyzer-toolchain)

## Goal

Add the optional `compact` CLI integration as the new `analyzer-toolchain` crate:
compile-on-save diagnostics from the real compiler and formatting via `compact format`
— with everything native continuing to work when the toolchain is absent.

## Why

Native diagnostics are syntax-only until v2; the compiler is the only source of type,
disclosure, and semantic errors in v1. "Compiler as ground truth on save" is the
design's answer to diagnostic drift. Formatting has no native implementation and none
is planned — the toolchain owns it.

## Includes

- **Toolchain discovery + version query:** configurable location override → `PATH`.
  Detect versions and surface them (informs the language-version policy notice).
- **Compile-on-save:** `didSave` → run `compactc --skip-zk --vscode` against a scratch
  output directory → parse single-line diagnostics from stderr → publish merged with
  native diagnostics.
  - Diagnostics tagged by source (`compact-analyzer` vs `compactc`).
  - Deduplicate compiler diagnostics covering the same span as a native one.
  - Configurable timeout (default ~30 s); non-zero exits parsed best-effort;
    unparseable output becomes one generic diagnostic at file top.
- **Formatting:** shell out to `compact format`. No-op with a notice when absent.
- **Graceful degradation:** missing toolchain disables compile-on-save/formatting with
  a **one-time** status message explaining how to enable them — never an error spam.
- **Configuration:** toolchain location override, compile-on-save toggle, formatting
  toggle (completing the v1 configuration surface started in M2b).
- **Testing:** integration tests gated behind toolchain detection (skip cleanly when
  absent) + one CI job that installs the real `compact` CLI and exercises
  compile-on-save and formatting end-to-end.

## Excludes

- Any parsing of compiler output beyond the documented single-line `--vscode` format.
- Proving-key generation (`--skip-zk` always).
- Watch-mode / compile-on-type (save only; native diagnostics cover typing).
- Toolchain installation/management on behalf of the user (docs only).

## Facts / constraints to honor

- The compiler is Chez Scheme with **no JSON diagnostics, no AST dump, no check-only
  mode**. The only machine surface is plain-text
  `Exception: <file> line N, char C: <msg>` on stderr; `--vscode` collapses errors to
  single lines; `--skip-zk` skips proving-key generation for speed.
- Compiler line/char positions must be mapped into our `TextRange`/UTF-16 world
  carefully — verify the compiler's line/char conventions (1-based? char = byte or
  char?) empirically against the real binary before writing the parser. **Do not trust
  training data**; fixture every parse against actual compiler output at the pinned
  version.
- Toolchain calls are sandboxed failures (spec §5): a broken/hung compiler must never
  take the server down or wedge the main loop — run in a worker, honor cancellation.
- Compile-on-save compiles files from **disk**; unsaved dependent buffers may diverge
  from what the compiler sees. Acceptable for v1 — didSave means this file is on disk.
- The compiler needs an output directory even with `--skip-zk`; use a per-workspace
  scratch dir and clean it up.

## Dependencies / notes

- Independent of M3 (could reorder if dogfooding demands compiler diagnostics sooner);
  needs M2b only for multi-file projects to compile with correct import paths
  (pass the configured import path through to the compiler invocation).
- Upstream moves fast (monthly compiler releases); the stderr parser should be
  defensive and covered by fixtures per supported compiler version.
