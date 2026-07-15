# v2b.2 — Type Foundation (Ty + Inference Infra + Differential Harness) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the type-checking spine — a salsa-interned `Ty` universe, a tracked per-body inference entry point, and a hybrid `compactc`-differential harness — proven end-to-end by exactly one trivial rule (primitive-literal return typing). No distinctive type rules land here.

**Architecture:** `Ty` is a lifetime-free `#[salsa::interned]` type universe (workspace-shared, id-equality). Inference is a tracked query over v2b.1's `item_tree` + `parsed` green tree. The differential harness classifies `compactc`'s verdict (accept / parse-reject / post-parse-reject) and compares it to the native checker's verdict — a per-fixture rule-tagged runner plus a blocking no-false-positive corpus gate. Both self-skip when the toolchain/corpus is absent.

**Tech Stack:** Rust 2024, salsa 0.28 (`#[salsa::interned]`, `#[salsa::tracked]`), `compactp_ast`/`compactp_syntax` (external CST), the existing `analyzer-toolchain` `compact` CLI integration.

## Global Constraints

Every task's requirements implicitly include this section.

- **Verify-first.** `compactc` at the pinned tag (`compact 0.5.1` / compiler `0.31.1`) is ground truth; never docs, never training data. The slice's accept/reject fixtures are captured from real `compactc` **before** the rule is implemented (already done — see the *Pinned toolchain facts* below).
- **No LSP types in `analyzer-core`;** byte offsets (`text_size`) only. `Ty` does **not** escape `analyzer-core` — the IDE layer gets a `String` display projection (`ty_display`).
- **Single-threaded**, cooperative `should_continue`; no salsa cross-thread `Cancelled`.
- **Diagnostics are tracked queries returning `Arc<[Diagnostic]>`** (not accumulators). `compactp_diagnostics::Diagnostic` has no `PartialEq`, so a diagnostics-returning query uses `no_eq` — the exact precedent set by `resolution_diagnostics_query` in `db.rs`.
- **Pre-release.** No back-compat shims/migrations/dual code paths — clean wholesale replacement (`prerelease-no-back-compat`).
- **Never commit `[patch.crates-io]`.**
- **Editor integration is out of scope here.** Surfacing type diagnostics in the VS Code Problems panel + the native-vs-compiler toggle is **v2b.final** (`docs/superpowers/plans/2026-07-15-v2b-final-vscode-integration.md`). This plan exposes `AnalysisHost::type_diagnostics` as the API the harness (and later v2b.final) consumes; it does **not** wire it into `server.rs` publish.

## Pinned toolchain facts (verified 2026-07-15 against `compact 0.5.1` / compiler `0.31.1`)

These resolve the spec's "blocking prerequisite" (foundation spec §5 — the `compactc` type-phase invocation). Verified empirically; Task 2 encodes them.

- There is **no** stop-after-type-check flag. Phase isolation = **classifying the rejection**.
- Invocation: `compact compile --skip-zk --vscode <source> <scratch>` (the existing `compile_file`, `analyzer-toolchain/src/compile.rs`). `--skip-zk` skips ZK codegen; parse + type-check still run.
- **Accept:** exit `0`.
- **Parse rejection:** exit `255`; the `--vscode` single-line `Exception:` message begins with the literal `parse error:` (e.g. `Exception: f.compact line 3 char 20: parse error: found ":" ...`). The native analyzer has its *own* parser (compactp), so a parse disagreement is a parser bug, not a type one → **excluded** from the type gate.
- **Post-parse rejection:** exit `255`; `Exception:` message with **no** `parse error:` prefix (type mismatch, unbound identifier, etc.) → the "type-phase reject" bucket for the differential.
- **Indeterminate:** any other exit (usage error `1`, timeout, cancellation, spawn failure) → excluded.
- Slice ground truth (captured from `compactc`):
  - `export circuit foo(): Boolean { return true; }` → **exit 0 (accept)**.
  - `export circuit foo(): Field { return true; }` → **exit 255**, `Exception: ... mismatch between actual return type Boolean and declared return type Field of circuit foo` (post-parse reject).

## Drift reconciliation (checked against as-merged v2b.1, commit `7707858`)

The v2b.2 draft outline's assumptions were verified against the merged code. Findings folded into this plan:

1. **The `Workspace` carry-forward is real and still needed.** `source_text_for` (`db.rs:718-751`) loops `ws.file_deps(db).values()` and reads every file's `deps`, taking a salsa dependency on *every* file's `FileDeps`. `Workspace` (`db.rs:171-177`) has only `stdlib` + `file_deps`. **Task 1** fixes it (adds `file_srcs`), first, before type queries inherit the broad dependency.
2. **There is no monolithic "def-map".** Resolution is a family of tracked *point* queries (`resolve_query`, `resolve_name_query`, `resolve_in_file`) over `item_tree`. The slice needs none of them — primitive-literal return typing is single-file and reads `item_tree`/`parsed` directly. Cross-declaration inference (later rules) will call the resolution queries; the foundation does not.
3. **Diagnostics-backdating is already settled by precedent.** `resolution_diagnostics_query` ships `no_eq` + `Arc<[Diagnostic]>`. Type diagnostics adopt the identical shape (Task 5).
4. **`Ty` is lifetime-free.** salsa 0.28 supports interned structs without a `'db` lifetime when all fields are `'static` (`salsa-0.28.0/tests/cycle_left_recursive_query.rs`). A `'static` `Ty` keeps the whole inference API free of `'db` threading, matching db.rs's no-lifetime tracked-fn style, and fits the spec's "shared workspace-wide" `Ty`.

## Corpus binary-gate direction (decision, from spec §5 + §8)

The full biconditional ("native reports a type error **iff** `compactc` rejects at the type phase") is the **v2b release gate** (program spec §8.1), reachable only once every rule is implemented. At **foundation**, with a single rule, the corpus gate asserts the direction that must hold from day one and forever: **no false positives** — for every corpus file `compactc` *accepts*, the native checker emits **zero** type diagnostics. Files `compactc` rejects post-parse are *counted and reported* but not asserted against native rejection (the rules that would reject them do not exist yet). This is exactly the v2b.2 done-bar: "harness runs green over the corpus binary gate with the trivial slice."

## File Structure

- `crates/analyzer-core/src/ty.rs` **(create)** — `TyKind` enum, interned `Ty`, `ty_display`. One responsibility: the type universe + its display projection.
- `crates/analyzer-core/src/infer.rs` **(create)** — CST→`Ty` lowering (`type_node_kind`, `literal_ty_kind`), circuit-node lookup, the tracked inference entry point (`infer_circuit_returns`), the tracked `type_diagnostics_query`, and the `AnalysisHost::type_diagnostics` bridge. One responsibility: inference + type diagnostics.
- `crates/analyzer-core/src/db.rs` **(modify)** — add `Workspace.file_srcs`; rewrite `source_text_for` to an O(1) map lookup (Task 1).
- `crates/analyzer-core/src/analysis.rs` **(modify)** — publish `file_srcs` (Task 1).
- `crates/analyzer-core/src/lib.rs` **(modify)** — declare `mod ty; mod infer;`, re-export `Ty`, `TyKind`, `ty_display`.
- `crates/analyzer-toolchain/src/differential.rs` **(create)** — `CompilerVerdict` + `classify` (reuses `compile_file` + `parse_compiler_stderr`).
- `crates/analyzer-toolchain/src/lib.rs` **(modify)** — declare `mod differential;`, re-export.
- `crates/compact-analyzer/tests/fixtures/type/*.compact` **(create)** — slice fixtures captured from `compactc`.
- `crates/compact-analyzer/tests/type_differential.rs` **(create)** — the rule-tagged fixture runner (Task 6) and the binary corpus gate (Task 7).

## Standard verification commands (used by every task)

- Build+test a crate: `cargo test -p <crate> -q`
- Whole workspace: `cargo test --workspace -q`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Format check: `cargo fmt --all --check`

---

### Task 1: `Workspace.file_srcs` map (v2b.1 carry-forward)

Add a `FileId → SourceText` map to the `Workspace` salsa input and make `source_text_for` an O(1), dependency-narrow lookup, replacing the O(files × deps) scan that took a salsa dependency on every file's `FileDeps`. Behavior-preserving.

**Files:**
- Modify: `crates/analyzer-core/src/db.rs` (`Workspace` struct ~171-177; `source_text_for` ~718-751; call sites of `source_text_for` at ~581, ~699; test-only `Workspace::new` at ~1042, ~1079)
- Modify: `crates/analyzer-core/src/analysis.rs` (`source_for` ~69-86; `register_stdlib` ~152-168; `workspace` ~177-185; `republish_file_deps_map` ~201-212; `forget_file` ~327-335)

**Interfaces:**
- Consumes: `Workspace` input, `SourceText` input, `AnalysisHost.sources: HashMap<FileId, (usize, SourceText)>` (`analysis.rs:38`).
- Produces: `Workspace::file_srcs(db) -> Arc<BTreeMap<FileId, SourceText>>`; `source_text_for(db, target, this_file, this_src, ws) -> Option<SourceText>` (the `fd: FileDeps` parameter is **removed**).

- [ ] **Step 1: Write the failing test** — add to `db.rs` `mod tests` a test that a transitively-included file's `SourceText` is recoverable via `file_srcs` even though the current file's `FileDeps` does not directly list it.

```rust
    #[test]
    fn source_text_for_reads_file_srcs_map() {
        let db = CompactDatabase::default();
        let this_src = SourceText::new(&db, Arc::from("circuit here(): [] {}"));
        let other_src = SourceText::new(&db, Arc::from("circuit there(): [] {}"));
        let this = crate::FileId::from_raw_for_test(0);
        let other = crate::FileId::from_raw_for_test(1);
        let mut srcs = std::collections::BTreeMap::new();
        srcs.insert(other, other_src);
        let ws = Workspace::new(&db, None, Arc::new(std::collections::BTreeMap::new()), Arc::new(srcs));
        // `other` is neither `this_file` nor stdlib nor in this file's deps:
        // it must be recovered from `file_srcs`.
        let got = source_text_for(&db, other, this, this_src, ws);
        assert_eq!(got, Some(other_src));
        // A file present nowhere resolves to None.
        assert_eq!(
            source_text_for(&db, crate::FileId::from_raw_for_test(9), this, this_src, ws),
            None
        );
    }
```

- [ ] **Step 2: Run it to verify it fails** — `cargo test -p analyzer-core source_text_for_reads_file_srcs_map -q`. Expected: FAIL to **compile** (`Workspace::new` takes 3 args, not 4; `source_text_for` takes `fd`).

- [ ] **Step 3: Add the `file_srcs` field to `Workspace`** in `db.rs`:

```rust
#[salsa::input]
pub struct Workspace {
    #[returns(clone)]
    pub stdlib: Option<(crate::FileId, SourceText)>,
    #[returns(clone)]
    pub file_deps: std::sync::Arc<std::collections::BTreeMap<crate::FileId, FileDeps>>,
    /// `FileId -> SourceText` for every file reachable as a resolution target
    /// (indexed files, resolved import/include targets, stdlib). Lets
    /// `source_text_for` recover a target's text with an O(1) map lookup that
    /// depends only on this map — not on every file's `FileDeps`. Republished
    /// by the host solely on structural change (a file enters/leaves the
    /// workspace), never on an in-body edit.
    #[returns(clone)]
    pub file_srcs: std::sync::Arc<std::collections::BTreeMap<crate::FileId, SourceText>>,
}
```

- [ ] **Step 4: Rewrite `source_text_for`** in `db.rs` (drop the `fd` parameter):

```rust
/// Maps a `FileId` reachable from the resolution context back to its
/// `SourceText`, via `Workspace.file_srcs` (O(1), and dependency-narrow: it
/// depends only on `file_srcs`, not on every file's `FileDeps`). The current
/// file and stdlib are checked first so a resolution that never leaves the
/// current file takes no `Workspace` dependency at all.
fn source_text_for(
    db: &dyn Db,
    target: crate::FileId,
    this_file: crate::FileId,
    this_src: SourceText,
    ws: Workspace,
) -> Option<SourceText> {
    if target == this_file {
        return Some(this_src);
    }
    if let Some((sfile, ssrc)) = ws.stdlib(db)
        && sfile == target
    {
        return Some(ssrc);
    }
    ws.file_srcs(db).get(&target).copied()
}
```

- [ ] **Step 5: Update `source_text_for` call sites** in `db.rs` — drop the `fd` argument at both. In `resolve_member` (~581): `let rsrc = source_text_for(db, *rfile, file, src, ws)?;`. In `field_of` (~699): `let psrc = source_text_for(db, *file, this_file, this_src, ws)?;`. (Leave `field_of`'s own `fd`/`ws` params as-is; they are still passed through to nested calls.)

- [ ] **Step 6: Add host plumbing** in `analysis.rs`. Add a snapshot builder and a republish helper, and call the republish on every `sources`-keyset change:

```rust
    /// Snapshot of the `FileId -> SourceText` map published to `Workspace`,
    /// built from the host's `sources`. Every resolution target's text lives
    /// here: `index_file` provisions each dep target via `source_for` before
    /// publishing, so `sources` is the complete set of reachable files.
    fn current_file_srcs_map(
        &self,
    ) -> Arc<std::collections::BTreeMap<FileId, crate::db::SourceText>> {
        Arc::new(self.sources.iter().map(|(&f, &(_, s))| (f, s)).collect())
    }

    /// Republishes `Workspace.file_srcs` from `sources`. Called only when the
    /// `sources` keyset changes (a file enters/leaves the workspace) — a
    /// structural event, exactly as `file_deps` is republished only on its own
    /// keyset change. In-body edits reuse a file's existing `SourceText` input
    /// (see `source_for`), so they never reach here.
    fn republish_file_srcs(&mut self) {
        use salsa::Setter as _;
        let srcs = self.current_file_srcs_map();
        match self.workspace_input {
            Some(ws) => {
                ws.set_file_srcs(&mut self.db).to(srcs);
            }
            None => {
                let deps = self.current_file_deps_map();
                self.workspace_input =
                    Some(crate::db::Workspace::new(&self.db, None, deps, srcs));
            }
        }
    }
```

- [ ] **Step 7: Fire `republish_file_srcs` on keyset change.** In `source_for` (`analysis.rs:69-86`), in the arm that **inserts a new** `file` into `self.sources` (the `None` branch of the `match self.sources.get(&file)`), call `self.republish_file_srcs();` after the insert. In `forget_file` (`analysis.rs:327-335`), after `self.sources.remove(&file);`, call `self.republish_file_srcs();`.

- [ ] **Step 8: Update the three host `Workspace::new` call sites** to pass `file_srcs`. In `register_stdlib` (~164), `workspace` (~182), and `republish_file_deps_map` (~209), build `let srcs = self.current_file_srcs_map();` alongside the existing `let map = self.current_file_deps_map();` and pass it as the fourth argument, e.g. `crate::db::Workspace::new(&self.db, stdlib, map, srcs)` / `crate::db::Workspace::new(&self.db, None, map, srcs)`.

- [ ] **Step 9: Update the two test-only `Workspace::new` sites** in `db.rs` `mod tests` (~1042, ~1079) to pass a fourth arg `Arc::new(std::collections::BTreeMap::new())`.

- [ ] **Step 10: Run the new test** — `cargo test -p analyzer-core source_text_for_reads_file_srcs_map -q`. Expected: PASS.

- [ ] **Step 11: Run the full core + workspace suite** — `cargo test -p analyzer-core -q` then `cargo test --workspace -q`. Expected: all green (every M2 cross-file resolution test still passes — this is the behavior-preserving done-bar). Then `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 12: Commit**

```bash
git add crates/analyzer-core/src/db.rs crates/analyzer-core/src/analysis.rs
git commit -m "perf(v2b.2): add Workspace.file_srcs; O(1) dependency-narrow source_text_for

Carry-forward from v2b.1 final review. source_text_for no longer scans
every file's FileDeps (O(files*deps) + a salsa dep on every FileDeps);
it reads a FileId->SourceText map on Workspace, republished only on
structural change. Behavior-preserving.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: `compactc` type-phase verdict classifier

Encode the pinned toolchain facts as a reusable classifier over a `compile_file` outcome. This is the "isolate the type-checking phase" prerequisite, in code.

**Files:**
- Create: `crates/analyzer-toolchain/src/differential.rs`
- Modify: `crates/analyzer-toolchain/src/lib.rs`

**Interfaces:**
- Consumes: `crate::compile::{CompileOutcome, CompileStatus}`, `crate::parse::parse_compiler_stderr`.
- Produces: `CompilerVerdict` (enum: `Accept`, `RejectParse`, `RejectPostParse`, `Indeterminate`); `classify(outcome: &CompileOutcome) -> CompilerVerdict`.

- [ ] **Step 1: Write the failing test** — create `crates/analyzer-toolchain/src/differential.rs` with only the test module, exercising the four verdicts from synthetic outcomes:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::{CompileOutcome, CompileStatus};

    fn outcome(status: CompileStatus, stderr: &str) -> CompileOutcome {
        CompileOutcome { status, stderr: stderr.to_string() }
    }

    #[test]
    fn accept_on_exit_zero() {
        assert_eq!(classify(&outcome(CompileStatus::Ok, "")), CompilerVerdict::Accept);
    }

    #[test]
    fn parse_error_is_reject_parse() {
        let e = "Exception: f.compact line 3 char 20: parse error: found \":\" looking for a typed pattern or \")\"";
        assert_eq!(classify(&outcome(CompileStatus::CompileError, e)), CompilerVerdict::RejectParse);
    }

    #[test]
    fn type_error_is_reject_post_parse() {
        let e = "Exception: f.compact line 2 char 3: mismatch between actual return type Boolean and declared return type Field of circuit foo";
        assert_eq!(classify(&outcome(CompileStatus::CompileError, e)), CompilerVerdict::RejectPostParse);
    }

    #[test]
    fn unbound_identifier_is_reject_post_parse() {
        let e = "Exception: f.compact line 3 char 10: unbound identifier bar";
        assert_eq!(classify(&outcome(CompileStatus::CompileError, e)), CompilerVerdict::RejectPostParse);
    }

    #[test]
    fn usage_and_timeout_are_indeterminate() {
        assert_eq!(classify(&outcome(CompileStatus::InvocationError, "Usage: ...")), CompilerVerdict::Indeterminate);
        assert_eq!(classify(&outcome(CompileStatus::TimedOut, "")), CompilerVerdict::Indeterminate);
    }
}
```

- [ ] **Step 2: Wire the module** — in `crates/analyzer-toolchain/src/lib.rs` add `mod differential;` (with the other `mod` lines) and `pub use differential::{CompilerVerdict, classify};` (with the other `pub use` lines).

- [ ] **Step 3: Run to verify it fails** — `cargo test -p analyzer-toolchain differential -q`. Expected: FAIL to compile (`classify`/`CompilerVerdict` undefined).

- [ ] **Step 4: Implement the classifier** — prepend to `differential.rs` (above the test module):

```rust
//! Classifies a `compact compile --skip-zk --vscode` outcome into a
//! type-differential verdict.
//!
//! Verified against `compact 0.5.1` / compiler `0.31.1` (2026-07-15): the
//! compiler has no stop-after-type-check flag, so phase isolation is done by
//! classifying the rejection. A parse rejection's single-line `--vscode`
//! `Exception:` message begins with the literal `parse error:`; a post-parse
//! rejection (type mismatch, unbound identifier, ...) does not. The native
//! analyzer has its own parser, so parse disagreements are excluded from the
//! type gate.

use crate::compile::{CompileOutcome, CompileStatus};
use crate::parse::parse_compiler_stderr;

/// A `compactc` verdict, reduced to what the type differential needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompilerVerdict {
    /// Exit `0`: accepted through the whole pipeline.
    Accept,
    /// Rejected at the parse phase (`Exception` message begins `parse
    /// error:`). Excluded from the type gate.
    RejectParse,
    /// Rejected after parsing (type/semantic error): the type-phase reject
    /// bucket.
    RejectPostParse,
    /// No usable type-phase verdict (usage error, timeout, cancellation,
    /// spawn failure). Excluded.
    Indeterminate,
}

/// Classify one [`compile_file`](crate::compile_file) outcome.
pub fn classify(outcome: &CompileOutcome) -> CompilerVerdict {
    match outcome.status {
        CompileStatus::Ok => CompilerVerdict::Accept,
        CompileStatus::CompileError => {
            let parsed = parse_compiler_stderr(&outcome.stderr);
            let is_parse = parsed
                .diagnostics
                .iter()
                .any(|d| d.message.starts_with("parse error:"));
            if is_parse {
                CompilerVerdict::RejectParse
            } else {
                CompilerVerdict::RejectPostParse
            }
        }
        CompileStatus::InvocationError
        | CompileStatus::TimedOut
        | CompileStatus::Cancelled => CompilerVerdict::Indeterminate,
    }
}
```

- [ ] **Step 5: Run to verify it passes** — `cargo test -p analyzer-toolchain differential -q`. Expected: PASS. Then `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-toolchain/src/differential.rs crates/analyzer-toolchain/src/lib.rs
git commit -m "feat(v2b.2): compactc type-phase verdict classifier

Pins the type-checking-phase isolation prerequisite: classify a
--skip-zk --vscode outcome into Accept / RejectParse / RejectPostParse /
Indeterminate, reusing parse_compiler_stderr. Verified against compact
0.5.1 / compiler 0.31.1.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: `Ty` universe + display projection

The interned type universe with the minimal constructors the slice needs, plus the IDE-facing `String` display.

**Files:**
- Create: `crates/analyzer-core/src/ty.rs`
- Modify: `crates/analyzer-core/src/lib.rs`

**Interfaces:**
- Consumes: `crate::db::Db`.
- Produces: `TyKind` (enum: `Boolean`, `Field`, `Unknown`); interned `Ty` with `Ty::new(db, TyKind) -> Ty` and `Ty::kind(db) -> TyKind`; `ty_display(db: &dyn Db, ty: Ty) -> String`.

- [ ] **Step 1: Write the failing test** — create `crates/analyzer-core/src/ty.rs`:

```rust
//! The native checker's type universe. `Ty` is a salsa-interned, lifetime-free
//! (`'static`) type id: equality is id comparison and types are shared
//! workspace-wide. The foundation ships only the constructors the
//! primitive-literal slice needs; the universe grows one rule at a time
//! (v2b.3…N). `Ty` never escapes `analyzer-core` — [`ty_display`] is the
//! projection the IDE layer consumes.

use crate::db::Db;

/// A Compact type, as far as the foundation models it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TyKind {
    /// `Boolean`.
    Boolean,
    /// `Field`.
    Field,
    /// A type the foundation does not yet model (any non-primitive, or an
    /// absent annotation). Never the *source* of a type error: an `Unknown`
    /// operand or expectation suppresses a rule rather than firing it.
    Unknown,
}

/// A salsa-interned type. Lifetime-free (all fields are `'static`), so a `Ty`
/// is a `Copy`, workspace-shared id — two independently constructed
/// `Boolean`s are the same `Ty`.
#[salsa::interned]
pub struct Ty {
    #[returns(copy)]
    pub kind: TyKind,
}

/// Display projection for the IDE layer (hover / completion detail).
pub fn ty_display(db: &dyn Db, ty: Ty) -> String {
    match ty.kind(db) {
        TyKind::Boolean => "Boolean".to_string(),
        TyKind::Field => "Field".to_string(),
        TyKind::Unknown => "?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CompactDatabase;

    #[test]
    fn interning_gives_id_equality() {
        let db = CompactDatabase::default();
        let a = Ty::new(&db, TyKind::Boolean);
        let b = Ty::new(&db, TyKind::Boolean);
        let c = Ty::new(&db, TyKind::Field);
        assert_eq!(a, b, "equal kinds intern to the same id");
        assert_ne!(a, c);
    }

    #[test]
    fn display_projects_to_strings() {
        let db = CompactDatabase::default();
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Boolean)), "Boolean");
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Field)), "Field");
        assert_eq!(ty_display(&db, Ty::new(&db, TyKind::Unknown)), "?");
    }
}
```

- [ ] **Step 2: Wire the module** — in `crates/analyzer-core/src/lib.rs` add `mod ty;` (with the other `mod` lines) and `pub use ty::{Ty, TyKind, ty_display};` (with the other `pub use` lines).

- [ ] **Step 3: Run to verify it fails then passes** — `cargo test -p analyzer-core ty:: -q`. If the interned macro needs adjustment, the compile error names it; fix and re-run. Expected once compiling: PASS (both tests).

- [ ] **Step 4: Lint + format** — `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add crates/analyzer-core/src/ty.rs crates/analyzer-core/src/lib.rs
git commit -m "feat(v2b.2): interned Ty universe + display projection

Lifetime-free #[salsa::interned] Ty over a minimal TyKind (Boolean,
Field, Unknown); id-equality via interning; ty_display is the IDE-facing
String projection. Ty does not escape analyzer-core.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 4: Inference entry point (primitive-literal return typing)

The tracked per-body query that types the primitive-literal operand of each `return`, plus the CST→`Ty` lowering helpers it needs.

**Files:**
- Create: `crates/analyzer-core/src/infer.rs`
- Modify: `crates/analyzer-core/src/lib.rs`

**Interfaces:**
- Consumes: `crate::db::{Db, SourceText, item_tree, parsed}`, `crate::ty::{Ty, TyKind}`, `crate::{ItemTree, SymbolKind}`, `compactp_ast`, `compactp_syntax::SyntaxKind`.
- Produces: `infer_circuit_returns(db: &dyn Db, src: SourceText, circuit_index: u32) -> Arc<[(text_size::TextRange, Ty)]>` — for the circuit at item-tree index `circuit_index`, the `(return-statement range, Ty of the returned primitive literal)` for each `return` whose operand the foundation can type. Empty when the index is not a circuit or has no such returns. Also (pub(crate)) `type_node_kind(&compactp_ast::Type) -> TyKind` and `circuit_node_by_index(db, src, u32) -> Option<compactp_ast::CircuitDef>` (reused by Task 5).

- [ ] **Step 1: Write the failing test** — create `crates/analyzer-core/src/infer.rs` with the code below **including** its test module, but expect it not to compile yet is not the flow here — write the full module, then the test drives it. Add this test module at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{CompactDatabase, SourceText};
    use crate::ty::TyKind;
    use std::sync::Arc;

    fn circuit_index(db: &dyn crate::db::Db, src: SourceText, name: &str) -> u32 {
        let tree = crate::db::item_tree(db, src);
        tree.symbols
            .iter()
            .position(|s| s.name == name && s.kind == crate::SymbolKind::Circuit)
            .expect("circuit present") as u32
    }

    #[test]
    fn boolean_literal_return_is_typed_boolean() {
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("export circuit foo(): Field { return true; }"));
        let idx = circuit_index(&db, src, "foo");
        let returns = infer_circuit_returns(&db, src, idx);
        assert_eq!(returns.len(), 1);
        assert_eq!(returns[0].1.kind(&db), TyKind::Boolean);
    }

    #[test]
    fn non_primitive_literal_return_is_unknown() {
        let db = CompactDatabase::default();
        // A numeric literal: the Uint lattice is a later rule, so it is Unknown
        // at the foundation.
        let src = SourceText::new(&db, Arc::from("export circuit foo(): Field { return 1; }"));
        let idx = circuit_index(&db, src, "foo");
        let returns = infer_circuit_returns(&db, src, idx);
        assert_eq!(returns.len(), 1);
        assert_eq!(returns[0].1.kind(&db), TyKind::Unknown);
    }

    #[test]
    fn type_node_kind_maps_primitives() {
        // Exercised indirectly, but assert the mapping directly via a parse.
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("export circuit foo(): Boolean { return true; }"));
        let idx = circuit_index(&db, src, "foo");
        let circuit = circuit_node_by_index(&db, src, idx).expect("circuit node");
        assert_eq!(type_node_kind(&circuit.return_type().unwrap()), TyKind::Boolean);
    }
}
```

- [ ] **Step 2: Implement the module** — prepend to `infer.rs` (above the test module):

```rust
//! Type inference (foundation slice) + type diagnostics.
//!
//! The inference entry point is `infer_circuit_returns`: one tracked query per
//! circuit body, typing the primitive-literal operand of each `return`. It
//! reads `item_tree`/`parsed` directly (single-file typing) — no resolution
//! queries are needed for the slice. The universe of expressions it types
//! grows one rule at a time. Incrementality is file-granular at the foundation
//! (every body reads the file's `parsed` green tree); finer input splitting is
//! a later concern, as noted in the foundation spec.

use std::sync::Arc;

use compactp_ast::AstNode;

use crate::db::{Db, SourceText, item_tree, parsed};
use crate::ty::{Ty, TyKind};

/// Lower a CST `Type` node to a `TyKind`. Only the primitives the foundation
/// models map to a concrete kind; everything else is `Unknown`.
pub(crate) fn type_node_kind(ty: &compactp_ast::Type) -> TyKind {
    use compactp_ast::Type;
    match ty {
        Type::Boolean(_) => TyKind::Boolean,
        Type::Field(_) => TyKind::Field,
        _ => TyKind::Unknown,
    }
}

/// The `TyKind` of a literal expression. A `true`/`false` token is `Boolean`;
/// numeric/string literals are `Unknown` at the foundation (the `Uint` lattice
/// and byte/string typing are later rules).
fn literal_ty_kind(lit: &compactp_ast::expr::LiteralExpr) -> TyKind {
    use compactp_syntax::SyntaxKind;
    let has = |k: SyntaxKind| {
        lit.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == k)
    };
    if has(SyntaxKind::TRUE_KW) || has(SyntaxKind::FALSE_KW) {
        TyKind::Boolean
    } else {
        TyKind::Unknown
    }
}

/// The `CircuitDef` CST node for the circuit at item-tree index `index`, found
/// by matching the symbol's `name_range` against each `CircuitDef`'s name
/// token range (covers nested circuits too). `None` if `index` is not a
/// circuit.
pub(crate) fn circuit_node_by_index(
    db: &dyn Db,
    src: SourceText,
    index: u32,
) -> Option<compactp_ast::CircuitDef> {
    let tree = item_tree(db, src);
    let sym = tree.symbols.get(index as usize)?;
    if sym.kind != crate::SymbolKind::Circuit {
        return None;
    }
    let name_range = sym.name_range;
    let root = compactp_syntax::SyntaxNode::new_root(parsed(db, src).green);
    root.descendants()
        .filter_map(compactp_ast::CircuitDef::cast)
        .find(|c| c.name().map(|n| n.text_range()) == Some(name_range))
}

/// Inference entry point for one circuit body: `(return-statement range, Ty)`
/// for each `return` whose operand is a primitive literal the foundation can
/// type. `Ty` is interned (`PartialEq`), so this query backdates normally.
#[salsa::tracked(returns(clone))]
pub fn infer_circuit_returns(
    db: &dyn Db,
    src: SourceText,
    circuit_index: u32,
) -> Arc<[(text_size::TextRange, Ty)]> {
    let Some(circuit) = circuit_node_by_index(db, src, circuit_index) else {
        return Arc::from(Vec::new());
    };
    let Some(body) = circuit.body() else {
        return Arc::from(Vec::new());
    };
    let mut out = Vec::new();
    for stmt in body.stmts() {
        let compactp_ast::Stmt::Return(ret) = stmt else {
            continue;
        };
        let Some(compactp_ast::expr::Expr::Literal(lit)) = ret.value() else {
            continue;
        };
        let kind = literal_ty_kind(&lit);
        out.push((ret.syntax().text_range(), Ty::new(db, kind)));
    }
    Arc::from(out)
}
```

- [ ] **Step 3: Wire the module** — in `crates/analyzer-core/src/lib.rs` add `mod infer;` (with the other `mod` lines). No re-export yet (Task 5 adds the host bridge and the public surface).

- [ ] **Step 4: Reconcile AST paths.** The `compactp_ast` paths above (`compactp_ast::Type`, `compactp_ast::Stmt`, `compactp_ast::expr::Expr`, `compactp_ast::expr::LiteralExpr`, `compactp_ast::CircuitDef`) match the crate's module layout (`Stmt`/`Type`/`CircuitDef` in `nodes.rs`, re-exported at crate root; `Expr`/`LiteralExpr` in `expr.rs`). If a path fails to resolve, `cargo` names the correct one — mirror the imports already used in `db.rs` (`compactp_ast::SourceFile`, `compactp_ast::expr::StructExpr`) and fix. Run `cargo test -p analyzer-core infer:: -q`.

- [ ] **Step 5: Verify tests pass** — `cargo test -p analyzer-core infer:: -q`. Expected: PASS (all three). Then `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 6: Commit**

```bash
git add crates/analyzer-core/src/infer.rs crates/analyzer-core/src/lib.rs
git commit -m "feat(v2b.2): inference entry point — primitive-literal return typing

infer_circuit_returns: one tracked query per circuit body, typing the
primitive-literal operand of each return via the interned Ty universe.
Reads item_tree/parsed directly (single-file). Plus type_node_kind and
circuit_node_by_index lowering helpers.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: Type-diagnostics query + host bridge

Apply the return-mismatch rule as a tracked `Arc<[Diagnostic]>` query (the `no_eq` diagnostics precedent), and expose it via an `AnalysisHost` bridge — the API the harness and (later) v2b.final consume.

**Files:**
- Modify: `crates/analyzer-core/src/infer.rs` (add `type_diagnostics_query`, the diagnostic builder, and the `impl AnalysisHost` bridge)
- Modify: `crates/analyzer-core/src/lib.rs` (no new re-export needed — `AnalysisHost` is already exported; the bridge is a method on it)

**Interfaces:**
- Consumes: `infer_circuit_returns`, `circuit_node_by_index`, `type_node_kind`, `crate::ty::{Ty, TyKind, ty_display}`, `crate::{Diagnostic, DiagnosticCode}`, `crate::AnalysisHost`, `crate::FileId`.
- Produces: `type_diagnostics_query(db: &dyn Db, src: SourceText) -> Arc<[Diagnostic]>` (tracked, `no_eq`); `AnalysisHost::type_diagnostics(&mut self, file: FileId) -> Vec<Diagnostic>`.

- [ ] **Step 1: Write the failing test** — add to `infer.rs` `mod tests`:

```rust
    #[test]
    fn return_mismatch_emits_one_type_diagnostic() {
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("export circuit foo(): Field { return true; }"));
        let diags = type_diagnostics_query(&db, src);
        assert_eq!(diags.len(), 1, "one mismatch");
        assert_eq!(diags[0].code, crate::DiagnosticCode::new("E", 3001));
        assert!(
            diags[0].message.contains("Boolean") && diags[0].message.contains("Field"),
            "message names both types: {:?}",
            diags[0].message
        );
    }

    #[test]
    fn matching_return_emits_no_diagnostic() {
        let db = CompactDatabase::default();
        let src = SourceText::new(&db, Arc::from("export circuit foo(): Boolean { return true; }"));
        assert!(type_diagnostics_query(&db, src).is_empty());
    }

    #[test]
    fn unknown_operand_or_expectation_suppresses() {
        let db = CompactDatabase::default();
        // Unknown declared type (a Uint): no rule fires yet.
        let a = SourceText::new(&db, Arc::from("export circuit foo(): Uint<0..7> { return true; }"));
        assert!(type_diagnostics_query(&db, a).is_empty());
        // Unknown operand (numeric literal) into a modeled type: suppressed.
        let b = SourceText::new(&db, Arc::from("export circuit foo(): Boolean { return 1; }"));
        assert!(type_diagnostics_query(&db, b).is_empty());
    }
```

Confirm the `DiagnosticCode` field name (`code`) and constructor (`DiagnosticCode::new("E", 3001)`) match usage in `db.rs` (`resolution_diagnostics_query` builds `Diagnostic::error(DiagnosticCode::new("E", 9001), msg, span)`); if the public field differs, mirror `db.rs`.

- [ ] **Step 2: Run to verify it fails** — `cargo test -p analyzer-core infer::tests::return_mismatch -q`. Expected: FAIL to compile (`type_diagnostics_query` undefined).

- [ ] **Step 3: Implement the query + diagnostic builder** — add to `infer.rs` (after `infer_circuit_returns`):

```rust
use compactp_diagnostics::{Diagnostic, DiagnosticCode};

/// Type diagnostics for `src` (foundation rule: primitive-literal return
/// mismatch). For each circuit with a declared return type the foundation
/// models (`Boolean`/`Field`), every primitive-literal `return` operand whose
/// `Ty` differs from the declared type yields one diagnostic. An `Unknown`
/// declared type or operand suppresses the rule (never a false positive).
///
/// `no_eq`: `Diagnostic` (external crate) has no `PartialEq`, so
/// `Arc<[Diagnostic]>` can't be compared for backdating — same rationale and
/// shape as `resolution_diagnostics_query` in `db.rs`.
#[salsa::tracked(returns(clone), no_eq)]
pub fn type_diagnostics_query(db: &dyn Db, src: SourceText) -> Arc<[Diagnostic]> {
    let tree = item_tree(db, src);
    let mut diags = Vec::new();
    for (idx, sym) in tree
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == crate::SymbolKind::Circuit)
    {
        let idx = idx as u32;
        let Some(circuit) = circuit_node_by_index(db, src, idx) else {
            continue;
        };
        let declared = circuit
            .return_type()
            .map(|t| type_node_kind(&t))
            .unwrap_or(TyKind::Unknown);
        if declared == TyKind::Unknown {
            continue; // only fire on return types the foundation models
        }
        for (range, ty) in infer_circuit_returns(db, src, idx).iter() {
            let actual = ty.kind(db);
            if actual != TyKind::Unknown && actual != declared {
                diags.push(return_mismatch_diag(
                    db, actual, declared, &sym.name, *range,
                ));
            }
        }
    }
    Arc::from(diags)
}

/// The return-type-mismatch diagnostic. Wording tracks `compactc`'s
/// ("mismatch between actual return type X and declared return type Y of
/// circuit Z"); wording is not gated by the harness.
fn return_mismatch_diag(
    db: &dyn Db,
    actual: TyKind,
    declared: TyKind,
    circuit_name: &str,
    span: text_size::TextRange,
) -> Diagnostic {
    let actual = ty_display(db, Ty::new(db, actual));
    let declared = ty_display(db, Ty::new(db, declared));
    Diagnostic::error(
        DiagnosticCode::new("E", 3001),
        format!(
            "mismatch between actual return type {actual} and declared return type {declared} of circuit {circuit_name}"
        ),
        span,
    )
}
```

Add `use crate::ty::ty_display;` to the module's imports (or fully-qualify).

- [ ] **Step 4: Add the host bridge** — append an `impl` block to `infer.rs`:

```rust
impl crate::AnalysisHost {
    /// Type diagnostics for `file` (foundation: primitive-literal return
    /// mismatch). Thin bridge over the tracked `type_diagnostics_query`,
    /// mirroring `resolution_diagnostics`. Single-file typing: no `FileDeps`/
    /// `Workspace` inputs are needed for the slice. Editor surfacing (Problems
    /// panel + toggle) is wired in v2b.final, which consumes this method.
    pub fn type_diagnostics(&mut self, file: crate::FileId) -> Vec<Diagnostic> {
        let Some(src) = self.src_of(file) else {
            return Vec::new();
        };
        type_diagnostics_query(self.db_ref(), src).to_vec()
    }
}
```

`src_of` and `db_ref` are the existing `pub(crate)` host accessors used by the resolution bridges (`analysis.rs:91`, `analysis.rs:97`).

- [ ] **Step 5: Add a host-level integration test** — append to `infer.rs` `mod tests` a test exercising the bridge through the VFS overlay path (mirrors `analysis.rs` test helper):

```rust
    #[test]
    fn host_bridge_reports_return_mismatch() {
        use crate::AnalysisHost;
        use std::path::Path;
        let mut host = AnalysisHost::new();
        let file = host.vfs_mut().file_id(Path::new("/t/foo.compact"));
        host.vfs_mut()
            .set_overlay(file, "export circuit foo(): Field { return true; }".to_string(), 1);
        let diags = host.type_diagnostics(file);
        assert_eq!(diags.len(), 1);
    }
```

- [ ] **Step 6: Run + verify** — `cargo test -p analyzer-core infer:: -q`. Expected: PASS (all infer tests). Then `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 7: Commit**

```bash
git add crates/analyzer-core/src/infer.rs
git commit -m "feat(v2b.2): type_diagnostics_query + AnalysisHost::type_diagnostics bridge

Return-mismatch rule as a tracked no_eq Arc<[Diagnostic]> query (the
resolution-diagnostics precedent); Unknown operand/expectation suppresses
(no false positives). Host bridge is the API the differential harness and
v2b.final consume; not yet wired into server publish.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 6: Rule-tagged fixture harness (vertical slice, both directions)

The per-fixture precision tier: native verdict vs a captured-from-`compactc` expectation, with rule attribution, plus (when the toolchain is present) a live cross-check that `compactc` still agrees.

**Files:**
- Create: `crates/compact-analyzer/tests/fixtures/type/bool_return_ok.compact`
- Create: `crates/compact-analyzer/tests/fixtures/type/bool_return_mismatch.compact`
- Create: `crates/compact-analyzer/tests/type_differential.rs`

**Interfaces:**
- Consumes: `analyzer_core::AnalysisHost`, `analyzer_toolchain::{Toolchain, compile_file, classify, CompilerVerdict}`, `tempfile`.
- Produces: (test binary only) a `native_rejects(text, path) -> bool` helper and the fixture-table runner.

- [ ] **Step 1: Create the fixtures** (captured from `compactc` — see *Pinned toolchain facts*).

`bool_return_ok.compact`:
```
export circuit foo(): Boolean {
  return true;
}
```

`bool_return_mismatch.compact`:
```
export circuit foo(): Field {
  return true;
}
```

- [ ] **Step 2: Write the harness test** — create `crates/compact-analyzer/tests/type_differential.rs`:

```rust
//! Compiler-differential harness for native type checking (v2b.2 foundation).
//!
//! Tier 1 (this file's `rule_tagged_fixtures`): per-fixture native verdict vs a
//! verdict captured from real `compactc`, with rule attribution. Tier 2
//! (`corpus_no_false_positives`): the blocking no-false-positive gate over the
//! ~486-file corpus. Both self-skip cleanly when the toolchain / corpus is
//! absent.

use std::path::{Path, PathBuf};
use std::time::Duration;

use analyzer_core::{AnalysisHost, FileId};
use analyzer_toolchain::{CompilerVerdict, Toolchain, classify, compile_file};

/// The native checker's verdict for one source: does it report any type
/// diagnostic? (Parse diagnostics are a separate surface and are not consulted
/// here — the differential is type-only.)
fn native_rejects(text: &str, path: &Path) -> bool {
    let mut host = AnalysisHost::new();
    let file: FileId = host.vfs_mut().file_id(path);
    host.vfs_mut().set_overlay(file, text.to_string(), 1);
    !host.type_diagnostics(file).is_empty()
}

/// `compactc`'s verdict for one source file on disk, or `None` if no toolchain.
fn compiler_verdict(tc: &Toolchain, source: &Path) -> CompilerVerdict {
    let scratch = tempfile::tempdir().expect("scratch");
    let outcome = compile_file(tc, source, scratch.path(), &[], Duration::from_secs(30), None);
    classify(&outcome)
}

struct Fixture {
    name: &'static str,
    /// Expected native verdict (rule-tagged expectation, captured from compactc).
    native_rejects: bool,
    /// The rule this fixture pins.
    rule: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture { name: "bool_return_ok.compact", native_rejects: false, rule: "primitive-literal-return" },
    Fixture { name: "bool_return_mismatch.compact", native_rejects: true, rule: "primitive-literal-return" },
];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/type")
}

#[test]
fn rule_tagged_fixtures() {
    let dir = fixtures_dir();
    let tc = Toolchain::discover(None); // None = no live cross-check, still assert native.
    for fx in FIXTURES {
        let path = dir.join(fx.name);
        let text = std::fs::read_to_string(&path).expect("fixture readable");

        // Native side: must match the captured expectation.
        let got = native_rejects(&text, &path);
        assert_eq!(
            got, fx.native_rejects,
            "[{}] native verdict for {} (rule {})",
            if got { "reject" } else { "accept" }, fx.name, fx.rule
        );

        // Live cross-check (only when the toolchain is present): the native
        // reject direction must correspond to a compactc post-parse rejection,
        // and accept to a compactc accept. Wording/span are NOT compared.
        if let Some(tc) = &tc {
            let verdict = compiler_verdict(tc, &path);
            let expected = if fx.native_rejects {
                CompilerVerdict::RejectPostParse
            } else {
                CompilerVerdict::Accept
            };
            assert_eq!(verdict, expected, "compactc verdict for {} (rule {})", fx.name, fx.rule);
        } else {
            eprintln!("type_differential: compactc absent; skipped live cross-check for {}", fx.name);
        }
    }
}
```

- [ ] **Step 3: Run the fixture runner** — `cargo test -p compact-analyzer --test type_differential rule_tagged_fixtures -q`. Expected: PASS. In this sandbox the toolchain IS present (`compact 0.5.1`), so the live cross-check runs and must also pass; if the toolchain is absent elsewhere, the native assertions still run and the cross-check self-skips.

- [ ] **Step 4: Lint + format** — `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add crates/compact-analyzer/tests/type_differential.rs crates/compact-analyzer/tests/fixtures/type
git commit -m "test(v2b.2): rule-tagged type-differential fixture harness

Per-fixture native verdict vs a compactc-captured expectation, with rule
attribution and a live compactc cross-check when the toolchain is present.
Slice fixtures (boolean-literal return accept/mismatch) captured from
compact 0.5.1. Wording/span not compared.

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 7: Binary corpus gate (no false positives)

The blocking release-level gate, foundation direction: over every corpus file `compactc` accepts, the native checker must emit **zero** type diagnostics. Self-skips when the corpus or toolchain is absent (this sandbox has neither the corpus nor `COMPACT_CORPUS_DIR` — it self-skips; CI runs it).

**Files:**
- Modify: `crates/compact-analyzer/tests/type_differential.rs` (add the corpus test + a `corpus_dir` helper)

**Interfaces:**
- Consumes: same as Task 6, plus the corpus-discovery convention from `corpus_smoke.rs` (`COMPACT_CORPUS_DIR` or `../../../compactp/tests/corpus`).

- [ ] **Step 1: Add the corpus-dir helper** (mirrors `corpus_smoke.rs:12-21`) to `type_differential.rs`:

```rust
fn corpus_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("COMPACT_CORPUS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../compactp/tests/corpus");
    p.is_dir().then_some(p)
}
```

- [ ] **Step 2: Write the corpus gate** — add to `type_differential.rs`:

```rust
/// Binary corpus gate (foundation direction: no false positives). For every
/// corpus file `compactc` ACCEPTS at the type phase, the native checker must
/// emit zero type diagnostics. Files compactc rejects post-parse are counted
/// and reported but not asserted against native rejection — the full
/// biconditional is the v2b release gate, reached as rules land. Skips when
/// the toolchain or corpus is absent.
#[test]
fn corpus_no_false_positives() {
    let Some(tc) = Toolchain::discover(None) else {
        eprintln!("corpus gate SKIPPED: compactc absent");
        return;
    };
    let Some(dir) = corpus_dir() else {
        eprintln!("corpus gate SKIPPED: no COMPACT_CORPUS_DIR and no ../compactp checkout");
        return;
    };
    let files = analyzer_core::discover_compact_files(&[dir]);
    assert!(files.len() > 100, "expected a large corpus, got {}", files.len());

    let mut accepted = 0usize;
    let mut compiler_rejected = 0usize;
    let mut indeterminate = 0usize;
    let mut false_positives: Vec<PathBuf> = Vec::new();

    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match compiler_verdict(&tc, &path) {
            CompilerVerdict::Accept => {
                accepted += 1;
                if native_rejects(&text, &path) {
                    false_positives.push(path);
                }
            }
            CompilerVerdict::RejectParse | CompilerVerdict::Indeterminate => indeterminate += 1,
            CompilerVerdict::RejectPostParse => compiler_rejected += 1,
        }
    }

    eprintln!(
        "corpus gate: accepted={accepted} compiler_rejected(post-parse)={compiler_rejected} indeterminate={indeterminate} false_positives={}",
        false_positives.len()
    );
    assert!(
        false_positives.is_empty(),
        "native emitted type diagnostics on {} file(s) compactc accepts (false positives): {:?}",
        false_positives.len(),
        false_positives
    );
}
```

- [ ] **Step 3: Run it** — `cargo test -p compact-analyzer --test type_differential corpus_no_false_positives -q`. Expected in this sandbox: **self-skip** (prints "corpus gate SKIPPED: no COMPACT_CORPUS_DIR ..."), test passes. To exercise it locally when a corpus is available: `COMPACT_CORPUS_DIR=/path/to/compactp/tests/corpus cargo test -p compact-analyzer --test type_differential corpus_no_false_positives -q -- --nocapture`.

- [ ] **Step 4: Lint + format + full suite** — `cargo test --workspace -q`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 5: Commit**

```bash
git add crates/compact-analyzer/tests/type_differential.rs
git commit -m "test(v2b.2): binary corpus gate — no native type false positives

Over every corpus file compactc accepts, assert the native checker emits
zero type diagnostics. Self-skips without a corpus/toolchain (the full
biconditional is the v2b release gate, reached as rules land).

Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage** (foundation spec §4/§5/§6 + program spec §5/§9):
- Interned `Ty` universe + display projection → Task 3. ✓
- Single tracked inference entry point per checkable body → Task 4 (`infer_circuit_returns`). ✓
- Type diagnostics as a tracked `Arc<[Diagnostic]>` query (no_eq precedent, not accumulators) → Task 5. ✓
- Hybrid harness: binary corpus gate (blocking) → Task 7; per-fixture rule-tagged checks → Task 6. ✓
- Pin the `compactc` type-phase invocation (blocking prerequisite) → verified empirically (recorded in *Pinned toolchain facts*), encoded in Task 2. ✓
- Trivial primitive-literal vertical slice, fixture captured from compactc before implementation → Tasks 4–6 (fixtures captured in *Pinned toolchain facts* / Task 1 pre-work, asserted in Task 6). ✓
- Verify-first, no LSP types in core, single-threaded, pre-release → Global Constraints; `Ty` stays in-crate (display projection only). ✓
- v2b.1 carry-forward (`Workspace` `FileId→SourceText` map) → Task 1. ✓
- Editor surfacing deferred to v2b.final → stated; Task 5 exposes the consuming API without wiring `server.rs`. ✓

**2. Placeholder scan:** No "TBD"/"handle edge cases"/"similar to Task N" — every code step carries complete code; every test step shows the assertions; every run step gives the command + expected outcome. The two "reconcile if the path/field name differs" notes (Task 4 Step 4, Task 5 Step 1) are verification instructions against named existing precedents in `db.rs`, not placeholders.

**3. Type consistency:** `TyKind`/`Ty`/`ty_display` (Task 3) are used with identical signatures in Tasks 4–5. `infer_circuit_returns(db, src, circuit_index) -> Arc<[(TextRange, Ty)]>` (Task 4) is consumed with that exact shape in Task 5. `circuit_node_by_index`/`type_node_kind` (Task 4, `pub(crate)`) are reused in Task 5. `CompilerVerdict`/`classify` (Task 2) are consumed in Tasks 6–7. `AnalysisHost::type_diagnostics(file) -> Vec<Diagnostic>` (Task 5) is consumed by `native_rejects` (Tasks 6–7). `Workspace::new` gains a 4th arg in Task 1 and all five call sites are updated in the same task. `source_text_for` loses its `fd` arg in Task 1 with both call sites updated together.

**Sequencing rationale:** Task 1 lands the carry-forward before any type query inherits the over-broad dependency. Task 2 pins the toolchain seam before the harness is built on it. Tasks 3→4→5 build the spine bottom-up (universe → inference → diagnostics). Tasks 6→7 build the harness on the spine (fixtures before the corpus gate). Each task ends with an independently testable, independently reviewable deliverable.
