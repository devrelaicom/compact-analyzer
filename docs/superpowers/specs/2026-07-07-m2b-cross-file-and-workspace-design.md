# compact-analyzer M2b — Cross-file Resolution + Workspace Index — Design

**Date:** 2026-07-07
**Status:** Approved
**Author:** Aaron Bassett (with Claude)

Milestone-level design for **M2b**, the second half of the spec's M2. It extends
the project design (`docs/superpowers/specs/2026-07-06-compact-analyzer-design.md`,
§3.4 name resolution) and the milestone context
(`.superpowers/milestones/m2b-cross-file-and-workspace.md`). It builds directly on
M2a (single-file navigation + stdlib), merged to `main` at commit `87244c1`.

## 1. Context

M2a resolves only in-file modules and the bundled stdlib. Any import that would
require a filesystem search resolves silently to `None`
(`resolve_through_import`'s two `None` arms: string-path imports, and identifier
imports with no matching in-file module). Find-references and rename are
single-file. There is no workspace index and no workspace-symbols feature.

Real Compact projects are multi-file, and v1's release bar is daily-driver use on
a real multi-file project. M2b closes that gap: it makes `import`/`include`
resolve to real files on disk exactly as the compiler does, stands up an eager
workspace index so navigation works across the whole project, makes import
failures visible, and lands the corpus smoke test the project spec commits to.

## 2. Decisions (settled during brainstorming)

| Question | Decision |
|---|---|
| Indexing model | **Eager workspace index** — the only model that makes workspace-wide references/rename correct (you must know every file to find every reference). Matches the project spec's "dirty-flagged workspace index." |
| Workspace discovery | LSP `workspaceFolders`/`rootUri`, then a **gitignore-aware crawl** of `*.compact` under each root. Import-search-path files outside the root are indexed on-demand when first resolved. |
| Invalidation | **Full incremental engine**: reverse-dependency tracking, dirty propagation, cached cross-file derived facts, generation counter. |
| Concurrency | **Single-threaded, cooperative cancellation** — the generation counter is a revision polled at unit boundaries by draining pending edits; no thread pool, no `!Send` snapshot refactor. |
| File freshness | Register `workspace/didChangeWatchedFiles` (capability-gated, graceful fallback) to keep the index correct against on-disk changes outside open buffers. |
| Import diagnostics | **Error severity, precise, conservative** — fired only after the full search fails; matches the compiler (unresolved import is a hard error). |
| Deferred-chore cleanups | All four ride along: analysis-cache polish, resolver-edge test gaps, per-file debounce, LineIndex/VFS edge tests. |

## 3. Architecture

### 3.1 New structure

A new `workspace` module in `analyzer-core` (with a companion source-path
resolver, e.g. `workspace.rs` + `source_path.rs`) holds the workspace index, the
dependency graph, and the generation counter. The existing resolver's two
silent-`None` arms in `resolve_through_import` are filled in to consult the
filesystem. `analyzer-ide` keeps its type vocabulary unchanged — still
`FileId`/`TextSize`/`TextRange`, zero `lsp-types`. The binary begins consuming
`initialize` params (roots, `initializationOptions`) and gains handlers for
`workspace/symbol`, `workspace/didChangeWatchedFiles`, and
`workspace/didChangeConfiguration`.

### 3.2 Data model

- **Generation counter** — `AnalysisHost` holds a `current_gen` bumped on every
  VFS mutation (`set_overlay`, `remove_overlay`, watched-file change). Cached
  derived facts record the generation they were computed at, so a stale fact is
  detectable.
- **Workspace symbol index** — `name → [(FileId, symbol_idx)]` over all top-level
  declarations (exported or not), rebuilt for a single file when it goes dirty.
  Powers `workspace/symbol` and prunes find-references candidate scanning.
  Excludes the bundled stdlib stub.
- **Dependency graph** — forward edges (`file → files it imports/includes`) and
  reverse edges (`file → dependents`), derived by resolving each file's
  imports/includes to target `FileId`s. This is the primary cached cross-file
  derived fact; the reverse edges drive precise dirty propagation.
- **Resolution diagnostics** — import/include resolution failures. Computed in
  the workspace layer, **not** in the content-hash-keyed per-file parse cache,
  because they depend on the state of *other* files. Keyed and invalidated by
  generation + dependency edges.

The per-file parse cache and `ItemTree` from M1/M2a are unchanged in role: still
keyed by content, still the source of a file's declarations and spans.

### 3.3 File-resolution semantics (the compiler contract)

A `resolve_source_path(importing_file, spec)` mirrors the compiler's
`find-source-pathname` (verified from the compiler Scheme source at
`compactc-v0.31.0`; **re-confirm at plan time**):

- An absolute path is used exactly; otherwise the search order is the importing
  file's directory first, then each import-path entry left-to-right.
- Imports **always append `.compact`** (the extension is omitted in source).
- Identifier `import Foo;` consults **in-scope modules first**, the filesystem
  second; a resolved `Foo.compact` must contain **exactly one** `module Foo {…}`
  and nothing else.
- String-path imports (`import "some/path";`) **never** consult in-scope modules
  and derive the expected module name from the last path component.
- Imports are memoized per `(name, pathname)` — realized here as the
  dependency-edge cache.

This fills the two silent-`None` arms of `resolve_through_import`: the identifier
arm with no in-file module, and the string-path arm.

### 3.4 `include` handling

`include` is treated as the textual splice it is, for resolution purposes, without
physically splicing buffers: an included file's top-level declarations become
visible in the includer's file scope; goto-def crosses into the included file;
cycles are detected along the active include path only; duplicate includes are
not deduplicated. `SourceFile::includes()` / `Include::path()` (a `STRING_LIT`)
are the AST touchpoints.

Note an asymmetry to resolve at plan time: verified include paths carry the
`.compact` extension (`include "std/lib.compact";`) whereas imports omit it. The
exact include-path mechanics — extension handling, the relative base, and the
order-dependent visibility of a textual splice — are **plan-time verification
items** (§8), not assumptions.

## 4. Features

### 4.1 Goto-definition

Works once §3.3/§3.4 resolve the target file; no feature-layer change beyond the
resolver following into other files. `NavTarget` already carries a `FileId`.

### 4.2 Find-references (workspace-wide)

Scan candidate files for `IDENT` tokens matching the name, re-resolve each against
its own file, and keep only those resolving to the same `Definition` — the same
shadowing-correct approach M2a uses single-file, extended across files. Candidate
files are pruned via the reverse-dependency graph plus a cheap name-presence check
(a file that neither is the definition's file nor reaches it through
import/include, and does not contain the identifier text, cannot reference it).
The pruning is an optimization over "scan every workspace file"; correctness does
not depend on it.

### 4.3 Rename (workspace-wide + per-use-site conflicts)

Rename produces edits across all files, and upgrades M2a's conservative
single-site conflict check (Errata E3: anchored at the last reference) to **full
per-use-site conflict detection**: at every reference site, `new_name` must
resolve to nothing or to the same definition; any conflicting binding at any site
fails the rename. Aliased and prefixed imports are handled correctly — renaming an
original name edits the import specifier's original-name token and the
declaration, never the alias binding or its uses. Keyword refusal, invalid-name
refusal, and builtin/stdlib-target refusal from M2a still apply.

### 4.4 Workspace symbols

`workspace/symbol` over the symbol index: case-insensitive subsequence match on
the query, returning name, kind, and location (`FileId` + range). User code only;
the bundled stdlib is excluded.

## 5. Diagnostics for unresolvable imports

Error severity, `source: compact-analyzer`, anchored on the import/include
path-or-name token, emitted only after the full `find-source-pathname` search
fails — plus the module-name-mismatch and not-exactly-one-module cases. Resolution
diagnostics are merged with parser diagnostics at publish time. Because they are
cross-file, open files are re-published on any change so that a fix (or a new
break) in one file reflects immediately in its dependents.

## 6. Incrementality & cancellation

Single-threaded, cooperative. Heavy operations — the initial crawl, and
workspace-wide find-references/rename — take a `should_continue: &dyn Fn() -> bool`
callback and poll it between files. The **binary** backs that callback by draining
pending `didChange` notifications from the LSP message queue (applying them,
bumping `current_gen`, marking files dirty) and returning whether the operation's
start-generation is still current. If superseded, the operation stops before
touching the now-mutated host and answers null; the client re-requests. This keeps
queue-draining in the binary (which owns the `Connection`) and cancellation-polling
as a clean primitive in core/ide, with no layering violation. Because the single
thread never runs the operation and the drain concurrently, the mutated host is
only observed after the operation has already decided to stop.

## 7. Discovery, watching, config, VFS

- **Discovery** — read `workspaceFolders`/`rootUri` from `initialize`; crawl each
  root with the `ignore` crate (gitignore-aware; skips `.git` and vendored dirs),
  collecting `*.compact`. Fall back to the directories of opened files if no root
  is provided. Import-path directories outside the root are interned and indexed
  on-demand the first time an import resolves into them.
- **Watching** — dynamically register a `**/*.compact` watcher via
  `workspace/didChangeWatchedFiles` (client-capability-gated; graceful fallback to
  on-demand resolution if unsupported). Create → intern + index; change →
  `Vfs::invalidate_disk` + reindex + dirty dependents; delete → evict + dirty
  dependents (their imports now fail, surfacing diagnostics).
- **Config** — import search path from `initializationOptions` /
  `workspace/didChangeConfiguration`, defaulting to the `COMPACT_PATH` environment
  variable (split on the platform path separator) when the setting is absent.
- **VFS** — add `Vfs::invalidate_disk(file)` to drop cached `Disk` content (never
  an active `Overlay`, which always wins) so a watched change re-reads from disk.

## 8. Plan-time verification items

Precedent: the M1 plan carried 5 real bugs and the M2a plan 3, all caught only by
empirical checks. Every fixture and API fact in the plan must be pre-verified.
Specific items:

- Exact `include` resolution mechanics: extension handling (include paths carry
  `.compact`, imports do not), the relative base, and textual-splice visibility
  ordering — verified against the compiler, not assumed.
- Re-confirm the `find-source-pathname` search order and the
  "exactly one `module Foo`" rule against the `../compactp` / compiler source
  before relying on them.
- The precise compactp AST accessors for every new touchpoint (`Include::path`,
  import specifier original-vs-alias, any node the dependency builder reads).
- That the `ignore` crate is acceptable as a new workspace dependency (widely
  used, MIT/Apache dual-licensed, builds under `--locked` on all three CI OSes).
- Whether `didChangeWatchedFiles` dynamic registration requires a specific client
  capability check, and the exact registration payload.

## 9. Testing

- **Fixture unit tests** (rust-analyzer `$0`-marker convention, `expect-test`) for
  every resolution path and feature, now spanning multiple in-memory files: bare
  and string-path imports, `include`, prefix and aliased imports, cross-file
  goto/references/rename, per-use-site conflicts, unresolvable-import diagnostics.
- **Corpus smoke** — run compactp's ~486-file corpus through workspace indexing,
  resolution, and every IDE feature, asserting no panics and no out-of-bounds
  spans. Lands with M2b's workspace index (per the project spec's commitment).
- **LSP integration** — black-box tests driving the real binary over stdio for the
  new capabilities (`workspace/symbol`, watched-file events, cross-file rename
  producing a multi-file `WorkspaceEdit`).
- **Deferred-chore coverage** — the three resolver-edge gaps (typed-lambda-param,
  `STRUCT_PAT_FIELD` label-as-local, `resolve_local_name` token-pick vs
  `ident_at_offset`), `LineIndex::offset()` edge fixtures, and the `Vfs::path`
  foreign-`FileId` precondition.

## 10. Deferred-chore cleanups (riding along)

- **Analysis-cache polish** — re-key the parse cache to avoid recomputing a hash
  on every cache hit and to remove the `DefaultHasher` collision risk (key
  overlays by version; key disk content by a hash computed once at read). Add
  eviction for files deleted from disk or no longer referenced by the workspace.
- **Resolver-edge test gaps** — close the three untested M2a edges above.
- **Per-file debounce** — replace the global diagnostics debounce with per-file
  trailing-edge deferral.
- **LineIndex / VFS edge tests** — `LineIndex::offset()` fixtures and the
  `Vfs::path` foreign-`FileId` documented precondition.

## 11. Invariants preserved

`lsp-types` appears only in the `compact-analyzer` binary; `analyzer-core` and
`analyzer-ide` speak byte offsets (`TextSize`/`TextRange`) + `FileId` + plain Rust
types; UTF-16 ↔ byte conversion happens only at the binary boundary; `lsp-types`
stays pinned at `0.95.1`. The server never dies on malformed or adversarial input
(crawl/watch failures degrade gracefully; per-request `catch_unwind`); unresolvable
positions answer null; stdout is protocol-only. `compactp_*` stays pinned at
`0.1.0-beta.1`; no `[patch.crates-io]` is ever committed. Rust edition 2024,
rust-version 1.90; CI = fmt + clippy `-D warnings` + tests on ubuntu/macOS/windows
with `--locked`. Conventional commits.

## 12. Out of scope (unchanged from the milestone context)

Completion, semantic tokens, folding/selection ranges (M3); any toolchain
invocation (M4); type-aware anything (v2); multi-root VS Code workspace
refinements beyond a sane default (revisit in M5 if needed).
