# M2b — Cross-file resolution + workspace index

## Goal

Extend M2a's single-file resolver across file boundaries so that imports and includes
resolve to real files on disk, and navigation (goto-definition, references, rename,
symbols) works across an entire multi-file Compact project — matching the compiler's
actual file-resolution semantics exactly.

## Why

M2a deliberately resolves only in-file modules and the bundled stdlib; any import that
would require filesystem search silently resolves to `None`. Real Compact projects are
multi-file, and the v1 bar is daily-driver use on a real multi-file project — M2b is
what closes that gap.

## Includes

- **Filesystem import resolution** implementing the compiler's verified semantics
  (see "Verified facts" below — do not re-derive these, they came from the Scheme source).
- **`include` handling:** treat as the textual splice it is for resolution purposes,
  without physically splicing buffers (per spec §3.4).
- **Workspace discovery + item index:** find the `.compact` files that constitute the
  workspace, index every declaration (name, kind, span, signature) with per-file dirty
  flags, as the spec's incrementality design describes. Non-open files read from disk;
  open-buffer overlays always win (the M1 `Vfs` already supports this).
- **Cross-file features:** goto-definition through imports/includes into other files;
  find-references and rename made workspace-wide (rename refuses conflicts as in M2a);
  **workspace symbols** (the one v1 symbol feature M2a doesn't cover).
- **Configuration:** import search path setting, defaulting to `COMPACT_PATH` when set
  (mirrors `--compact-path`).
- **Diagnostics for unresolvable imports** (M2a is silent by design; M2b owns making
  failures visible), including the known per-use-site import-conflict reporting gap
  noted in the M2a plan (~line 2544).
- Corpus smoke over the workspace index if not already landed.

## Excludes

- Completion, semantic tokens, folding (M3).
- Any toolchain invocation (M4).
- Type-aware anything (v2).
- Multi-root VS Code workspace refinements beyond a sane default (revisit in M5 if needed).

## Verified facts to honor (from compiler Scheme source @ compactc-v0.31.0)

Recorded in the M2a plan ("M2b planning notes") — the implementation contract:

- `find-source-pathname` **always appends `.compact`** — the extension must be omitted
  in source.
- Search order: absolute path used exactly; otherwise the **importing file's directory
  first**, then `COMPACT_PATH` entries left-to-right.
- `import Foo;` consults **in-scope modules first**, filesystem second; a resolved
  `Foo.compact` must contain **exactly one `module Foo {...}`** and nothing else.
- String-path imports (`import "some/path";`) **never** consult in-scope modules and
  derive the expected module name from the last path component.
- Includes splice textually; cycles are detected along the active include path only;
  duplicate includes are **not** deduplicated.
- Imports are memoized per `(name, pathname)`.
- `import CompactStandardLibrary;` is compiler-internal and never touches the
  filesystem (already handled in M2a via the bundled stub).

## Dependencies / notes

- Builds directly on M2a's `Resolver` and `ItemTree`; plan M2b only after M2a merges so
  the plan can reference the real APIs.
- Expect design work on invalidation: an edit to file A must dirty dependents of A
  (reverse-dependency tracking), and the generation-counter cancellation from the spec
  becomes load-bearing here.
- Open question: workspace discovery heuristic — Compact has no project manifest, so
  likely "all `.compact` under the workspace root(s)" with the import path config on top.
