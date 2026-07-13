# M5 — VS Code Extension + Distribution Implementation Plan

**Status: EXECUTABLE.** Supersedes `m5-vscode-extension-and-distribution-plan-draft.md`. Drift-checked against merged `main` @ **`ac071bd`** on 2026-07-13 (the draft's reconciliation was against `028f24f`; the post-M4 cleanup wave — CR-3/CR-4/CR-5, pgid hardening, shim 60→8 — changes **nothing** the client depends on; see Ground Truth §G8). All 13 Open Questions are **resolved** (§Resolved Decisions — four by the human on 2026-07-13, the rest by direct evidence). Human-owned steps H1–H6 gate only the publish/release legs; **every task below is codeable immediately**.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship v1's delivery vehicle: a first-party thin-client VS Code extension (`editors/vscode`, TypeScript, `vscode-languageclient` over stdio) published to VS Code Marketplace + Open VSX, and a cargo-dist GitHub Releases pipeline for the `compact-analyzer` binary (4 targets, shell/PowerShell installers, Homebrew tap) — installable by a stranger in one step, as the successor to the sunsetted official Compact extension.

**Architecture:** The extension is a **thin client**: static language assets (TextMate grammar, language-configuration, snippets — adapted with attribution from the official extension's Apache-2.0 assets at a pinned commit) that work before/without the server, plus an acquisition-then-launch pipeline: resolve the server binary (settings path → `PATH` → extension-storage cache → checksum-verified **download-on-activate** of the extension's pinned server version from GitHub Releases), validate every candidate via `--version` + the 0.x minor-match policy, then hand it to `vscode-languageclient` over stdio and get out of the way. All analysis comes from the server (M1–M4); the extension only configures and consumes it. Failed acquisition degrades to ONE clear message with manual-install guidance — never a broken activation. Distribution is cargo-dist (axodotdev, pinned 0.32.0) driving GitHub Releases off version tags, archives forced to `.tar.gz` on **all** targets so the extension extracts with built-in `zlib` + a ~40-line tar reader (zero runtime deps). Extension and server release in lockstep (same version); artifact checksums are baked into the VSIX at release time.

**Tech Stack:** TypeScript (strict) + `@types/vscode` 1.91.x; `vscode-languageclient` 10.1.0; esbuild 0.28.x single-file bundle; eslint (flat) + `tsc --noEmit`; **vitest** 4.x unit tests; E2E smoke via `@vscode/test-cli` 0.0.15 / `@vscode/test-electron` 3.0.0 (VERIFY harness shape at impl); `@vscode/vsce` 3.9.2 + `ovsx` 1.0.2 (devDependencies, Node ≥ 20; local node v26.3.1); **cargo-dist 0.32.0** (axodotdev — alive, latest release 2026-05-22; the astral-sh fork is archived — do NOT use it). Rust side untouched except dist metadata/`[profile.dist]`.

---

## Dispatch guidance (controller: read before Task 1)

- **Branch:** `feat/m5-vscode-extension-and-distribution` off `main` @ `ac071bd`. Merge via the pre-authorised ff-merge workflow (clean+reviewed → origin-not-diverged check → `git merge --ff-only` → full gate on merged main → push → delete branch).
- **Agent types** (per `use-specialist-agent-types`): implementers `devs:typescript-dev` for Tasks 1–2, 4–12; `devs:rust-dev` for Task 3 (cargo-dist/cargo surface). Reviewers `devs:code-reviewer`. **Model tiers:** sonnet default; **opus** for Task 6's review (checksum-verified download = the security trust anchor) and for the final whole-branch review.
- **Review loop:** per task, fresh implementer → run the SDD `review-package BASE HEAD` script → hand the reviewer the printed diff-file PATH (never paste diffs) → don't advance past an unclean review → controller re-runs the gate after every implementer returns → append a per-task line to `.superpowers/sdd/progress.md` (new `# M5 progress ledger` section).
- **Every implementer brief includes:** British spelling in comments/doc-comments (identifiers/API names/string literals stay as-is); conventional commits with NO trailer lines; never a `[patch.crates-io]`; the Global Constraints section below; and the training-data warning — **all vscode-languageclient / vsce / ovsx / cargo-dist / VS Code API facts must be read from the installed package's `.d.ts` / `--help` / generated output, never recalled.**
- **Gates:** extension tasks: from `editors/vscode`, `npm run lint && npm run check && npm test && npm run build` (+ `npx vsce ls` when the manifest changed). Rust-gate (`cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`, 272 tests baseline) whenever any root file is touched (Tasks 3, 11).
- **Blocked-vs-codeable:** nothing blocks Tasks 1–12. H1–H6 gate only: the real `vsce publish`/`ovsx publish` executions, the tap push, the icon in the shipped VSIX, and the first real `v0.1.0` tag. Task 11 writes all of that **secret-guarded and dormant**.

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Thin client:** the extension launches `compact-analyzer` and speaks LSP; it MUST NOT reimplement or approximate any analysis (no client-side parsing, no regex diagnostics, no completion providers). Every feature gap is fixed server-side. The only non-LSP language features are the static assets (grammar/config/snippets).
- **Rust side unchanged:** `lsp-types = 0.95.1`, `compactp_* = 0.1.0-beta.1`, never a `[patch.crates-io]`. The only permitted Cargo changes in M5 are dist metadata + `[profile.dist]`. No server code changes in this milestone (a server bug found during M5 is a separately-scoped fix).
- **Never break activation:** `activate()` must not throw and must not leave a broken state. Acquisition failure (offline, checksum mismatch, unsupported platform, incompatible version) produces ONE clear message with manual-install instructions and a functioning grammar/snippets-only session. No retry storms.
- **Checksum-verified downloads only:** every downloaded binary is verified against a sha256 pinned **inside the shipped VSIX** before first execution; https only, from the pinned release URL. No auto-update outside the pinned-version flow — a new server version ships as a new extension release (lockstep).
- **Lockstep versioning:** extension `package.json` version == server crate version (`0.1.0` now). 0.x ⇒ server **minor must match** the pin; ≥1.0 ⇒ major must match; patch skew within a matching minor is accepted (Resolved OQ7).
- **Branding fully independent:** binary `compact-analyzer`; the reserved `compact-language-server` name is deliberately ignored. Identity (Resolved OQ2/H1): publisher **`aaronbassett`**, `name: "compact-analyzer"`, `displayName: "Compact Analyzer"` — succession clear, never claiming to be official. Do NOT reuse the official extension's icons/logos (Midnight trademarks); only its Apache-2.0 grammar/config/snippets, with attribution.
- **Tooling gate per extension task:** `npm run lint && npm run check && npm test && npm run build` green from `editors/vscode`; `package-lock.json` committed; CI uses `npm ci`. Conventional commits, no trailers.
- **Secrets are human-owned, never committed:** `VSCE_PAT`, `OVSX_TOKEN`, `HOMEBREW_TAP_TOKEN` live only in GitHub Actions secrets (H2–H4).
- **Node floor:** dev tooling needs Node ≥ 20. Extension **runtime** code targets the Node bundled with `engines.vscode` — set the engine floor first (VERIFY item V4), don't use newer Node APIs.
- **Protocol hygiene:** stdio is reserved for LSP; the server logs to stderr — pipe it to the output channel, never the protocol stream.
- **British spelling** in comments/docs; identifiers and user-visible marketplace copy follow their own conventions.

---

## Ground truth — server surface @ `ac071bd` (re-verified 2026-07-13)

The client-facing contract, read from current `main` source. Line numbers cite `crates/compact-analyzer/src/server.rs` unless said otherwise.

- **G1 — `initializationOptions` keys (the ONLY config the server reads):** `toolchainPath` (string → PathBuf), `compileOnSave` (bool, default true), `formatting` (bool, default true) via `toolchain_config_from` (:1410–1417, defaults :1392–1400); `importSearchPath` (array of strings) with `COMPACT_PATH` env fallback via `import_search_path_from` (:1424–1432). **No timeout key** — `COMPILE_TIMEOUT = Duration::from_secs(30)` hardcoded (:33). Mistyped values are silently ignored (`as_str`/`as_bool` guards), defaults kept.
- **G2 — `importSearchPath: []` SUPPRESSES the `COMPACT_PATH` fallback** (an explicitly-sent empty array parses as `Some(empty)` → `[]`; the fallback fires only when the key is absent/non-array). The extension's "omit the key when the setting is empty" rule (Task 8) is **load-bearing**, keep it verbatim.
- **G3 — config is read ONCE at initialize; NO `didChangeConfiguration` handler exists** (no-op notification group is exactly `"initialized" | "$/setTrace" | "$/cancelRequest"`, :684) **and toolchain discovery is startup-only** (`Toolchain::discover` right after config, :145). ⇒ the restart prompt on `compact-analyzer.*` changes (Task 8/9) is **mandatory, not a courtesy**, and the restart command is the ONLY recovery path for a mid-session `compact` install.
- **G4 — capabilities:** `text_document_sync` Options form — `open_close: Some(true)`, `change: FULL`, `save: Supported(true)` with `includeText` deliberately omitted (the compiler reads the saved file from disk) (:76–83); `document_formatting_provider: Some(OneOf::Left(true))` (:92); all M3 providers (definition/references/rename/hover/documentSymbol/workspaceSymbol/folding/selection/completion-with-`.`/semanticTokens-full-with-server-legend) (:84–107). The extension configures **nothing** feature-specific — capability negotiation does it all (client-side didSave evidence: fact E6).
- **G5 — `toolchainPath` override is strict and soft-failing** (`crates/analyzer-toolchain/src/discovery.rs`): file-or-dir accepted; when set, PATH is NEVER searched; an unresolvable override ⇒ toolchain silently absent + ONE `window/showMessage` INFO on first wanted use — never an error. Deliberate asymmetry with the extension's own `serverPath` (hard failure, departure D5): do not "fix" server-side, do document it.
- **G6 — toolchain-absent notice:** exactly one per session, `MessageType::INFO`, `notify_toolchain_missing_once` (:936–950); message text: `compact-analyzer: no ``compact`` toolchain found. Install the Compact CLI (``compact``) or set "toolchainPath" in initializationOptions to enable compile-on-save diagnostics and document formatting.` — fired only when a *wanted* (toggle-on) feature finds the toolchain absent; toggle-off is silent. Resolved OQ11: README + setting-description carry the `initializationOptions.toolchainPath` ⇄ `compact-analyzer.toolchainPath` mapping; no interception, no server change.
- **G7 — handshake surface:** `compact-analyzer --version` → stdout `compact-analyzer {CARGO_PKG_VERSION}`, exit 0, handled before the server starts (`main.rs:6–9`). Initialize result carries `serverInfo: { name: "compact-analyzer", version: env!("CARGO_PKG_VERSION") }` (:112–114). Startup stderr logs the toolchain line (`compact-analyzer: compact toolchain <v> (language <v>) at <path>` / `no compact toolchain found; compile-on-save disabled`, :146–153) — first-line troubleshooting surface once piped to the output channel.
- **G8 — post-M4 cleanup delta (draft didn't know):** the ONLY client-visible change is **CR-4**: per-file diagnostics are capped at `MAX_DIAGNOSTICS_PER_FILE = 100` + one INFORMATION truncation note sourced `"compact-analyzer"` (:44, `cap_diagnostics` :1248), applied in the shared `merge_diagnostics` on BOTH publish paths. The client just renders ≤101 diagnostics — **no extension work**; Task 10's "≥1 diagnostic" E2E assertions unaffected. CR-3 (core-only memo), CR-5 (doc), pgid hardening and the shim change are invisible to the client. Formatting response shape unchanged: toggle-off/absent/already-formatted/syntax-error/timeout ALL `Some(vec![])`, success = one whole-document `TextEdit` (:610–635). Compiler diags still `source: "compactc"`, fail-fast ≤1 per save; `did_save` only compiles open docs with a real `file://` path. FAST-EXIT shutdown holds (`client.stop()` safe mid-compile).
- **G9 — layering invariant (extended for M5):** `lsp-types` appears ONLY in the `compact-analyzer` binary; analyzer crates speak bytes + `FileId`. M5's TS mirror: **only `extension.ts` and `config.ts` import `vscode`**; version/platform/download/acquire take injected deps (`fetch`/`exec`/`env`) and unit-test without an extension host. The TS↔Rust boundary is **exactly** the LSP protocol over stdio + the `initializationOptions` of G1 — nothing else crosses it.
- **G10 — repo facts:** workspace `repository = "https://github.com/devrelaicom/compact-analyzer"`; 4 members, only `crates/compact-analyzer` has a `[[bin]]`; all crates `publish = false` (stays). CI jobs: `lint`, `test` (3-OS), `msrv`, `toolchain` (installs compact via the official installer from `midnightntwrk/compact` releases, pins 0.31.1, `timeout-minutes: 15` + `GITHUB_TOKEN`). Baseline **272 local tests** (core 96, ide 72, binary 56, toolchain 48; gated suites lower in CI). Workspace version `0.1.0`.

---

## Empirically-verified facts (probed 2026-07-13 — re-verify anything marked VERIFY at impl time)

- **E1 — npm snapshot (via `npm view`):** `vscode-languageclient` **10.1.0** (engines `vscode: ^1.91.0`); `@vscode/vsce` **3.9.2** (node ≥ 20); `ovsx` **1.0.2** (node ≥ 20); `@types/vscode` latest 1.125.0 — but **pin `@types/vscode` to 1.91.x** to match the engine floor (vsce rejects `@types/vscode` newer than `engines.vscode`); `esbuild` **0.28.1**; `vitest` **4.1.10**; `@vscode/test-electron` **3.0.0**; `@vscode/test-cli` **0.0.15**. Identical to the draft's 3-day-old snapshot where they overlap — pin exact versions at `npm install` time.
- **E2 — compact CLI:** `compact check` → compiler 0.31.1 up-to-date; `compact self check` → devtools 0.5.1 up-to-date; `compact compile --language-version` → 0.23.0. Matches M4's CI pin exactly.
- **E3 — cargo-dist state:** `axodotdev/cargo-dist` — NOT archived, pushed 2026-07-07, **latest release v0.32.0** (2026-05-22) == compactp's pin. `astral-sh/cargo-dist` fork is **archived** (2025-12-19). Pin **0.32.0**, axodotdev origin. `unix-archive`/`windows-archive` config keys confirmed in source/docs/integration-tests; both accept `".tar.gz"` (Resolved OQ4).
- **E4 — compactp dist config** (read locally at `../compactp/dist-workspace.toml`): `cargo-dist-version = "0.32.0"`, `ci = "github"`, `installers = ["shell","powershell","homebrew"]`, `publish-jobs = ["homebrew"]`, `tap = "aaronbassett/homebrew-tap"`, `publish-prereleases = true`, the exact 4 targets M5 needs, `install-updater = false`, `[dist.github-custom-runners] aarch64-apple-darwin = "macos-14"`. Its `release.yml` is dist-generated and installs dist from `axodotdev/cargo-dist/releases/download/v0.32.0/cargo-dist-installer.sh`.
- **E5 — upstream repos (Resolved OQ13):** they are **two different repos with two different roles**, not a rename. `LFDT-Minokawa/compact` = "The Compact programming language" — the **source monorepo** (has `editor-support/{vsc,vim}`, actively pushed 2026-07-11) → the pinned-commit **asset-fetch** origin. `midnightntwrk/compact` = "Compact Releases" — the **distribution** repo (NO `editor-support` at all; 404) → the **toolchain install-docs/CI** origin (M4's CI already uses it). Record BOTH roles in THIRD-PARTY-NOTICES/NOTES.md. **Pinned asset commit: `fdfc61cf8b1311ca9fc5f8d155e1017d483a8acd`** (main HEAD, 2026-07-11). Upstream manifest at that commit: v0.2.11, publisher `midnightnetwork`, license Apache-2.0, engines `^1.116.0`, contributes languages/grammars/snippets/breakpoints/3 problemMatchers; grammar `scopeName: "source.compact"`; repository field points at the historical `midnightntwrk/compactc`.
- **E6 — client-side didSave (source-verified, `microsoft/vscode-languageserver-node` `client/src/common/textSynchronization.ts`):** `DidSaveTextDocumentFeature.initialize` self-registers the didSave sender whenever the server's resolved `textDocumentSync.save` is truthy — a boolean `save: true` becomes `{ includeText: false }`, exactly our server's shape (G4). **Compile-on-save works with zero client config.** (Runtime sanity re-check happens anyway in Task 9's manual verification.)
- **E7 — problem matchers are dead (Resolved OQ8, probed against real compact 0.31.1):** plain (non-`--vscode`) stderr for a broken file is now TWO lines — `Exception: broken.compact line 5 char 8:` then the indented message on the next line — with **no comma** after the line number and **basename-only** file refs. The official matcher regex (`line (\d+), char (\d+)`, single-line, `fileLocation: ["absolute"]`) fails on all three counts. Per the draft's own criterion ("drop without argument if the stderr shape drifted"): **DROP the problem matchers for v1**; record the evidence in `assets/NOTES.md`. LSP compile-on-save already covers diagnostics natively.
- **E8 — grammar/snippet ground truth for the audit:** the pinned-compiler keyword source is local at `../compactp/crates/compactp_syntax/src/syntax_kind.rs` (builtin-type keywords near :103) with lexer keyword tests at `../compactp/crates/compactp_lexer/src/lib.rs` (:511, :624, :635, :734). Generate `assets/keywords-0.23.txt` from it (document the command in NOTES.md).

---

## Resolved Decisions (all 13 draft OQs — none remain open)

| # | Decision | Resolution | Source |
|---|---|---|---|
| OQ1 | Server delivery model | **Download-on-activate** (one universal VSIX; settings → PATH → cache → checksum-verified pinned download). Platform-bundled VSIXes remain a later additive option. | Human, 2026-07-13 |
| OQ2/H1 | Extension identity | Publisher **`aaronbassett`**; `name: "compact-analyzer"`, `displayName: "Compact Analyzer"`; description: "Language support for the Compact smart contract language — the community successor to the sunsetted official Compact extension." Keywords `compact`, `midnight`. | Human, 2026-07-13 |
| OQ3 | Release channel | Plain 0.x Marketplace releases (no pre-release channel — parity conventions would constrain lockstep numbering). | Draft recommendation adopted |
| OQ4 | Archive format | `unix-archive = ".tar.gz"` **and** `windows-archive = ".tar.gz"` — one `zlib`+tar extraction path for all four targets, zero runtime deps. Key confirmed in dist 0.32 source. | Evidence (E3) |
| OQ5 | Checksum source | Bake `server-manifest.json` (version + per-triple artifact name + sha256, generated from dist's checksum output) **into the VSIX**; trust anchor = the extension package. | Draft recommendation adopted |
| OQ6/H4 | Homebrew tap | **Reuse `aaronbassett/homebrew-tap`** (compactp's; one tap, one token). | Human, 2026-07-13 |
| OQ7 | Version handshake | Pre-spawn `--version` probe (2 s timeout) on every candidate + post-initialize `serverInfo.version` assert. 0.x ⇒ minor match; ≥1.0 ⇒ major match; patch skew accepted. | Draft recommendation adopted |
| OQ8 | Problem matchers | **Dropped** — current compiler stderr broke all three regex assumptions (E7). | Evidence (E7) |
| OQ9 | Unit-test runner | **vitest** (4.1.10). | Draft recommendation adopted |
| OQ10 | Coexistence with `midnightnetwork.compact` | Detect via `vscode.extensions.getExtension("midnightnetwork.compact")`; one-time "you can uninstall the old extension" hint + README note. Never hard-block. | Draft recommendation adopted |
| OQ11 | Notice wording | **README mapping only** — server text stays; the `compact-analyzer.toolchainPath` setting description + README state the mapping to `initializationOptions.toolchainPath`. No middleware, no server change. | Human, 2026-07-13 |
| OQ12 | Mid-session toolchain install | README + restart command only (no showMessage interception). Revisit if dogfooding shows friction. | Draft recommendation adopted |
| OQ13 | Canonical upstream | **Both, by role**: assets from `LFDT-Minokawa/compact` @ `fdfc61cf` (source monorepo); toolchain install links/CI from `midnightntwrk/compact` releases (distribution repo). Record both in THIRD-PARTY-NOTICES/NOTES.md. | Evidence (E5) |

---

## Human-owned gating steps (H1–H6) — status + what they block

None block any coding task. All publish/release **executions** stay dormant (secret-guarded) until done.

- [x] **H1 — extension identity:** RESOLVED 2026-07-13 (publisher `aaronbassett`, names above).
- [ ] **H2 — Marketplace publisher + PAT:** create/verify the `aaronbassett` publisher on the VS Code Marketplace; Azure DevOps PAT (Marketplace→Manage, all accessible orgs); store as `VSCE_PAT` secret on `devrelaicom/compact-analyzer`. **Blocks:** `vsce publish` leg only.
- [ ] **H3 — Open VSX namespace + token:** Eclipse account, publisher agreement, `ovsx create-namespace aaronbassett` (VERIFY flow, V7); store `OVSX_TOKEN`. **Blocks:** `ovsx publish` leg only.
- [ ] **H4 — tap token:** confirmed reuse of `aaronbassett/homebrew-tap`; ensure `HOMEBREW_TAP_TOKEN` (push access) exists as a secret **on this repo**. **Blocks:** homebrew publish job on the first real tag.
- [ ] **H5 — icon artwork:** original icon (NO Midnight logos). Any interim placeholder must be original. **Blocks:** final marketplace listing polish (vsce packs without an icon; VERIFY it's warning-not-error, V5).
- [ ] **H6 — signing:** confirm nothing beyond vsce's built-in flow for v1 (no macOS notarisation — cargo-dist ships unsigned like compactp; README notes Gatekeeper). **Blocks:** nothing unless the answer is "more signing".
- **First real `v0.1.0` tag:** human-triggered after H2–H4 and the dogfood pass (Task 12), or explicitly deferred with the pipeline dry-run green.

---

## Execution-time VERIFY items (the survivors — everything else in the draft's 15 is resolved above)

Record each finding as an Errata fixture when checked. Training data on these APIs is unreliable — read the installed artefact.

- **V1 (`vscode-languageclient` 10.1.0 API, Task 9):** exact `LanguageClient` constructor signature, `ServerOptions`/`Executable` shape for stdio, `LanguageClientOptions` fields (`documentSelector`, `initializationOptions`, `outputChannel`, error/close handlers), `start()`/`stop()`/dispose semantics, clean-restart pattern, and where `initializeResult.serverInfo` is exposed. Read `node_modules/vscode-languageclient/lib/node/main.d.ts`.
- **V2 (manifest rules, Task 1):** whether `activationEvents` may be omitted/auto-generated from `contributes` at engine ^1.91; `main` expectations for a bundled extension; vsce validation is the oracle (`npx vsce ls`).
- **V3 (vsce/ovsx flags, Tasks 1, 11):** `vsce package` with a bundler (`--no-dependencies` current name/need), `.vscodeignore` semantics, `vscode:prepublish` hook, LICENSE/README/icon requirements, secret-scan step, `vsce publish` flags/PAT scope; `ovsx publish` flags + LICENSE rule. Check `--help` of the pinned versions.
- **V4 (extension-host Node, Task 1):** which Node the minimum supported engine (^1.91.0) bundles — gates global `fetch`, `node:crypto`, `zlib` usage in Tasks 6–7. Set `engines.vscode` first, derive the floor.
- **V5 (Marketplace naming/icon rules, Tasks 1, 12):** "Compact" in name/displayName acceptability, icon size/format, trademark-adjacent constraints — feeds the H5 asset and listing copy.
- **V6 (dist outputs, Task 3):** tag format (expect `v{version}` for the single-app workspace), exact artifact names (expect versionless `compact-analyzer-<triple>.tar.gz` per dist 0.32's naming), checksum output shape (per-artifact `.sha256` files and/or `dist-manifest.json`), and how dist picks apps (must be exactly the one bin). `dist plan` output IS the fixture — back-fill Tasks 5/6/11.
- **V7 (Open VSX namespace flow, H3/Task 11):** `ovsx create-namespace`, publisher agreement, token acquisition.
- **V8 (E2E harness, Task 10):** current `@vscode/test-cli`/`test-electron` API — download/launch, `--extensionDevelopmentPath`/`--extensionTestsPath`, injecting workspace settings, `xvfb-run` on Linux CI.
- **V9 (Windows/Unix binary handling, Tasks 5–7):** `.exe` suffix + `PATHEXT` in PATH probing, no-chmod-on-Windows, `chmod 0o755` after extraction on Unix, `context.globalStorageUri` fs semantics.
- **V10 (GitHub Releases download, Task 6):** direct-download URL redirect behaviour with Node's global `fetch`; direct asset downloads are unmetered vs the API (prefer the direct URL).
- **V11 (semantic tokens client side, Task 12 README):** no client legend needed (server supplies it); `editor.semanticHighlighting.enabled` theme interplay for the README.

---

## Shared Interfaces (single source of truth for cross-task signatures)

Implementers see only their own task; every cross-task name/type lives here. All modules are plain TypeScript; **only `extension.ts` and `config.ts` import `vscode`** (G9).

```ts
// src/version.ts — Task 4
/** Pinned server version this extension release is built for (== package.json "version").
 *  Single source of truth: `import pkg from "../package.json"` (resolveJsonModule; esbuild bundles it). */
export const PINNED_SERVER_VERSION: string;
/** Parse `compact-analyzer X.Y.Z[extra]` stdout → "X.Y.Z…", or null if unrecognisable. */
export function parseServerVersion(stdout: string): string | null;
/** 0.x: minor must match pinned. >=1.0: major must match. Patch skew allowed. Non-semver → false. */
export function isCompatible(serverVersion: string, pinned?: string): boolean;

// src/platform.ts — Task 5
export interface PlatformTarget {
  rustTriple: string;            // e.g. "aarch64-apple-darwin"
  archiveExt: ".tar.gz";         // fixed by Resolved OQ4 (both unix-archive and windows-archive)
  binaryName: string;            // "compact-analyzer" | "compact-analyzer.exe"
}
/** Map (process.platform, process.arch) → target; null when unsupported (clear message upstream). */
export function currentTarget(platform?: NodeJS.Platform, arch?: string): PlatformTarget | null;
/** Artifact filename for a target, matching dist's real naming (fixture from Task 3's `dist plan`).
 *  NOTE: dist 0.32 artifact names are versionless (the tag carries the version) — the draft's
 *  version parameter was dropped; V6 confirms. */
export function artifactName(target: PlatformTarget): string;

// src/download.ts — Task 6
export interface ServerManifest {                 // baked into the VSIX at release (Task 11, OQ5)
  version: string;                                 // == PINNED_SERVER_VERSION
  artifacts: Record<string /* rustTriple */, { name: string; sha256: string }>;
}
export class DownloadError extends Error { readonly userGuidance: string; }
export function verifySha256(data: Uint8Array, expectedHex: string): boolean;
/** Download pinned artifact → verify sha256 → extract (.tar.gz via zlib + minimal tar reader) →
 *  chmod 0o755 (unix) → temp-file + atomic rename to `<destDir>/<version>/<binaryName>`.
 *  Throws DownloadError (with guidance); never leaves a partial binary at the final path. */
export function downloadAndInstall(opts: {
  manifest: ServerManifest; target: PlatformTarget; destDir: string;
  baseUrl?: string;              // default: `https://github.com/devrelaicom/compact-analyzer/releases/download/<tag(manifest.version)>/` (tag format per V6)
  fetchImpl?: typeof fetch;      // injected for tests
}): Promise<string /* absolute binary path */>;

// src/acquire.ts — Task 7
export type ServerSource = "settings" | "path" | "storage" | "downloaded";
export type Acquired = { ok: true; source: ServerSource; binaryPath: string; version: string };
export type AcquireFailure = { ok: false; reason: string; userGuidance: string };
export type ExecProbe = (binary: string, args: string[], timeoutMs: number)
  => Promise<{ ok: boolean; stdout: string }>;
/** Resolution order: explicit settings path → PATH probe → storage cache → download.
 *  Every candidate is validated by `--version` (2 s timeout) + isCompatible before acceptance.
 *  An explicit-settings binary that fails validation is a hard AcquireFailure naming the setting
 *  (respect user intent); PATH/storage misses fall through. NEVER throws. */
export function acquireServer(deps: {
  configuredPath: string | undefined;   // settings "compact-analyzer.serverPath"
  storageDir: string;                   // context.globalStorageUri.fsPath
  manifest: ServerManifest;
  env?: NodeJS.ProcessEnv;
  exec?: ExecProbe;
  fetchImpl?: typeof fetch;
}): Promise<Acquired | AcquireFailure>;

// src/config.ts — Task 8 (imports vscode)
export interface InitOptions {          // EXACTLY the keys the server reads (G1)
  importSearchPath?: string[];
  toolchainPath?: string;
  compileOnSave?: boolean;
  formatting?: boolean;
}
export function initializationOptionsFromConfig(): InitOptions;   // omits keys left at default/empty (G2!)
export function configuredServerPath(): string | undefined;

// src/extension.ts — Task 9 (imports vscode + vscode-languageclient)
export interface ExtensionApi {
  serverStatus(): "running" | "unavailable";
  serverSource(): ServerSource | null;
}
// activate(context) → resolve → LanguageClient(stdio) → start → post-init serverInfo assert;
// registers "compact-analyzer.restartServer"; returns ExtensionApi (consumed by Task 10 E2E).
```

**Settings surface (contributes.configuration, Task 8)** — right column = the server key it feeds:

| Setting | Type | Default | Feeds |
|---|---|---|---|
| `compact-analyzer.serverPath` | string | `""` | client-side acquisition only (machine-overridable scope) |
| `compact-analyzer.importSearchPath` | string[] | `[]` | `initializationOptions.importSearchPath` — **omitted when empty** (G2) |
| `compact-analyzer.toolchainPath` | string | `""` | `initializationOptions.toolchainPath` — omitted when `""`; description carries the OQ11 mapping + G5 semantics (file-or-dir; disables PATH search; typo ⇒ features silently off + one notice) |
| `compact-analyzer.compileOnSave` | boolean | `true` | `initializationOptions.compileOnSave` (sent explicitly) |
| `compact-analyzer.formatting` | boolean | `true` | `initializationOptions.formatting` (sent explicitly) |
| `compact-analyzer.trace.server` | enum off/messages/verbose | `off` | vscode-languageclient standard tracing |

---

## File Structure

**Create (extension, `editors/vscode/`):** `package.json` + `package-lock.json`; `tsconfig.json` (strict, resolveJsonModule), `eslint.config.mjs`, `esbuild.mjs`, `.vscodeignore`, `.gitignore`; `language-configuration.json`, `syntaxes/compact.tmLanguage.json`, `snippets/compact.code-snippets`, `THIRD-PARTY-NOTICES.md`, `LICENSE`, `assets/NOTES.md`, `assets/keywords-0.23.txt`; `src/{extension,config,acquire,download,platform,version}.ts`; `src/test/unit/*.test.ts` (vitest) + `src/test/unit/fixtures/`; `src/test/e2e/` (runner + suite + fixture workspace); `server-manifest.json` (dev placeholder; regenerated at release); `README.md` (marketplace page), `CHANGELOG.md`.

**Create (distribution/docs):** `dist-workspace.toml` (root); `.github/workflows/release.yml` (dist-generated — never hand-edit the generated parts); `.github/workflows/extension-release.yml` (lockstep leg); `scripts/gen-server-manifest.mjs`; `docs/editors.md`.

**Modify:** root `Cargo.toml` (`[profile.dist]` + whatever `dist init` dictates — nothing else); `.github/workflows/ci.yml` (append `extension` + `extension-e2e` jobs, additive-only); `README.md` (Task 12 overhaul); `.superpowers/milestones/README.md` + `milestone-status` memory (flip at the very end, committed on the branch).

---

## Task 1: Scaffold `editors/vscode` (manifest, TypeScript, lint, bundle, vitest)

**Files:** Create `editors/vscode/{package.json,tsconfig.json,eslint.config.mjs,esbuild.mjs,.vscodeignore,.gitignore}`, `src/extension.ts` (stub), `src/test/unit/smoke.test.ts`.

**Interfaces:** Establishes the npm scripts every later gate uses: `build` (esbuild → `dist/extension.js`), `check` (`tsc --noEmit`), `lint` (eslint), `test` (`vitest run`), `package` (build + `vsce package`).

**Context:** Identity is settled (Resolved OQ2): publisher `aaronbassett`, name `compact-analyzer`, displayName `Compact Analyzer`, version `0.1.0` (lockstep). `engines.vscode: "^1.91.0"` (languageclient 10.1.0 floor, E1) unless a needed API forces newer (V2/V4 — check, record in Errata). Pin `@types/vscode` **1.91.x** (E1 — must not exceed the engine floor). V2/V3/V4 are this task's VERIFY items; `npx vsce ls` is the manifest oracle.

- [ ] **Step 1:** `mkdir -p editors/vscode/src/test/unit`; write `package.json`: identity fields above, `license: "MIT"`, `categories: ["Programming Languages"]`, `keywords: ["compact","midnight"]`, `main: "./dist/extension.js"`, empty `contributes` (Tasks 2/8 fill it), `repository` pointing at `devrelaicom/compact-analyzer`, scripts `{build, check, lint, test, test:e2e (placeholder), package}`, devDependencies pinned to re-checked versions of `typescript`, `@types/vscode@~1.91`, `@types/node`, `esbuild`, `eslint` + `typescript-eslint`, `vitest`, `@vscode/vsce`; dependency `vscode-languageclient@10.1.0` (re-check with `npm view` first; record any drift in Errata).
- [ ] **Step 2:** `cd editors/vscode && npm install` — commits `package-lock.json`. Expected: lockfile created; note any audit criticals.
- [ ] **Step 3:** Write `tsconfig.json` (strict true, `resolveJsonModule: true`, `module`/`moduleResolution` per the languageclient docs — V1 informs; `types: ["node"]`, exclude `dist`), `esbuild.mjs` (entry `src/extension.ts`, `--external:vscode`, cjs, platform node, sourcemap, minify off for debuggability), `eslint.config.mjs` (typescript-eslint recommended flat config), `.vscodeignore` (ship `dist/`, assets, `server-manifest.json`, README/LICENSE/notices; exclude `src`, tests, configs, `node_modules`), `.gitignore` (`node_modules/`, `dist/`, `*.vsix`).
- [ ] **Step 4:** Stub `src/extension.ts` (`export function activate() {} export function deactivate() {}`) + `smoke.test.ts` asserting the vitest runner works (e.g. `expect(1 + 1).toBe(2)`).
- [ ] **Step 5 (gate):** `npm run lint && npm run check && npm test && npm run build` — all green, `dist/extension.js` produced. `npx vsce ls` — sane file list, no validation complaints (fix now; record V2/V3 findings in Errata). No root files touched → Rust gate untouched.
- [ ] **Step 6:** Commit: `feat(vscode): scaffold extension package with lint/typecheck/test/bundle gate`

---

## Task 2: Language assets — grammar, language-configuration, snippets (adapted, attributed, audited)

**Files:** Create `editors/vscode/{language-configuration.json,syntaxes/compact.tmLanguage.json,snippets/compact.code-snippets,THIRD-PARTY-NOTICES.md,LICENSE,assets/NOTES.md,assets/keywords-0.23.txt}`; modify `package.json` (`contributes.languages/grammars/snippets`); add `src/test/unit/assets.test.ts`.

**Interfaces:** None cross-task — static assets + manifest wiring.

**Context:** Source: `LFDT-Minokawa/compact` @ **pinned commit `fdfc61cf8b1311ca9fc5f8d155e1017d483a8acd`** (E5), path `editor-support/vsc/compact/`. Apache-2.0 ⇒ adaptation permitted with attribution: THIRD-PARTY-NOTICES.md lists upstream project, commit, file list, Apache-2.0 reference; `assets/NOTES.md` records the regeneration/audit procedure (M6 automates later) **and the E7 problem-matcher evidence** (dropped: no comma, two-line message, basenames vs `fileLocation: absolute`). Do NOT copy icons/logos. Language id `compact`, extensions `[".compact"]`, scope `source.compact` (matches upstream → theme compatibility). **Problem matchers are NOT carried** (Resolved OQ8). Keyword ground truth: `../compactp/crates/compactp_syntax/src/syntax_kind.rs` (E8).

- [ ] **Step 1 (failing tests, `assets.test.ts`):** (a) the three asset files parse (JSON/JSONC); (b) grammar `scopeName === "source.compact"`; (c) keyword-coverage: every line of `assets/keywords-0.23.txt` appears in the grammar's keyword-pattern alternations (textual regex parse — crude but drift-catching); (d) every snippet body's `$n`/`${n:…}` placeholders are well-formed; (e) `package.json` `contributes` has NO `problemMatchers` key (guards the OQ8 decision against upstream re-adoption drift).
- [ ] **Step 2:** `npm test` — expect FAIL (files absent).
- [ ] **Step 3:** Fetch the three assets at the pinned commit (e.g. `curl -fsSL https://raw.githubusercontent.com/LFDT-Minokawa/compact/fdfc61cf8b1311ca9fc5f8d155e1017d483a8acd/editor-support/vsc/compact/<file>`). Generate `assets/keywords-0.23.txt` from `../compactp/crates/compactp_syntax/src/syntax_kind.rs` (document the exact command in NOTES.md). Audit + adapt: update grammar keyword/type lists for language 0.23 per the coverage test; audit each snippet against 0.23 syntax against the compactp corpus/`compact` CLI (fix or drop stale ones; note every change in NOTES.md — snippets are v0.2.11-era). Wire `contributes.languages/grammars/snippets`. Write THIRD-PARTY-NOTICES.md + MIT LICENSE (matching the repo) + NOTES.md (both repo roles per Resolved OQ13).
- [ ] **Step 4:** `npm test` — expect PASS. Manual spot-check: `code --extensionDevelopmentPath=$PWD`, open a `.compact` file → highlighting + bracket behaviour + one snippet fire, with NO server present.
- [ ] **Step 5 (gate + commit):** full npm gate + `npx vsce ls`. Commit: `feat(vscode): Compact grammar, language config, snippets (adapted from official extension, Apache-2.0, attributed)`

---

## Task 3: cargo-dist release pipeline (binary distribution) — runs EARLY to fixture artifact names

**Files:** Create `dist-workspace.toml`, `.github/workflows/release.yml` (dist-generated); modify root `Cargo.toml` (`[profile.dist]` etc. as `dist init` dictates), `Cargo.lock` only if dist requires (expect not).

**Interfaces:** Produces the artifact-naming + checksum-shape **fixtures** consumed by Tasks 5/6/11 (this is why it runs third, per draft departure D7 — no dependency on Tasks 1–2).

**Context:** Crib E4's compactp config with two changes: `tap = "aaronbassett/homebrew-tap"` stays (Resolved OQ6), and **add `unix-archive = ".tar.gz"` + `windows-archive = ".tar.gz"`** (Resolved OQ4). Keep `publish-prereleases = true` (harmless, future-proof), `install-updater = false`, the 4 targets, `[dist.github-custom-runners] aarch64-apple-darwin = "macos-14"`. Install dist **0.32.0 from axodotdev** (E3 — the astral fork is archived): `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/axodotdev/cargo-dist/releases/download/v0.32.0/cargo-dist-installer.sh | sh`. **Never hand-edit the generated workflow** (future `dist init` runs overwrite it). V6 is this task's fixture harvest.

- [ ] **Step 1:** Install pinned dist locally; `dist init` at repo root, answering to produce exactly the target config above; inspect the generated `dist-workspace.toml` + `.github/workflows/release.yml` diff. If `dist init` writes `[profile.dist]` into root `Cargo.toml`, keep it verbatim; nothing else in Cargo.toml may change.
- [ ] **Step 2 (fixture harvest — V6):** `dist plan` — expected: exactly ONE app (`compact-analyzer`) × 4 targets + shell/powershell installers + homebrew formula. Record in this plan's Errata: the exact artifact names (expected versionless `compact-analyzer-<triple>.tar.gz`), the tag format (expected `v{version}`), and the checksum output shape (per-artifact `.sha256` and/or `dist-manifest.json`). These become Tasks 5/6/11 fixtures.
- [ ] **Step 3:** `dist build` for the host target — `tar tzf` the produced archive, extract, run `./compact-analyzer --version` → expect `compact-analyzer 0.1.0`. Confirms the .tar.gz override took effect.
- [ ] **Step 4 (gate):** root Rust gate (`cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked` — 272 baseline) still green.
- [ ] **Step 5:** Commit: `build(dist): cargo-dist release pipeline (4 targets, tar.gz archives, installers, homebrew tap)` — `git add dist-workspace.toml .github/workflows/release.yml Cargo.toml` (+ `Cargo.lock` only if changed).

---

## Task 4: Version handshake module (`src/version.ts`)

**Files:** Create `src/version.ts`, `src/test/unit/version.test.ts`.

**Interfaces:** Produces `PINNED_SERVER_VERSION`, `parseServerVersion`, `isCompatible` (Shared Interfaces). Pure — no `vscode` import.

**Context:** `PINNED_SERVER_VERSION` reads the extension's own `package.json` version via `import pkg from "../package.json"` (resolveJsonModule; esbuild bundles it; vitest resolves it natively) — single source of truth, never hand-duplicated. Policy per Resolved OQ7. `--version` fixture: `compact-analyzer 0.1.0` (G7).

- [ ] **Step 1 (failing tests):**

```ts
import { describe, it, expect } from "vitest";
import { PINNED_SERVER_VERSION, parseServerVersion, isCompatible } from "../../version";
import pkg from "../../../package.json";

describe("parseServerVersion", () => {
  it("parses the real --version line", () =>
    expect(parseServerVersion("compact-analyzer 0.1.0\n")).toBe("0.1.0"));
  it("tolerates suffixes", () =>
    expect(parseServerVersion("compact-analyzer 0.2.0-rc.1")).toBe("0.2.0-rc.1"));
  it("rejects other binaries", () =>
    expect(parseServerVersion("compactc 0.31.1")).toBeNull());
  it("rejects garbage/empty", () => {
    expect(parseServerVersion("")).toBeNull();
    expect(parseServerVersion("segfault")).toBeNull();
  });
});

describe("isCompatible (0.x minor-match, >=1.0 major-match, patch skew ok)", () => {
  it.each([
    ["0.1.5", "0.1.0", true],
    ["0.2.0", "0.1.0", false],
    ["1.2.3", "1.0.0", true],
    ["2.0.0", "1.9.9", false],
    ["0.1.0", "0.1.0", true],
  ])("%s vs pin %s -> %s", (server, pin, want) =>
    expect(isCompatible(server, pin)).toBe(want));
  it("rejects non-semver", () => expect(isCompatible("not-a-version", "0.1.0")).toBe(false));
});

it("PINNED_SERVER_VERSION mirrors package.json", () =>
  expect(PINNED_SERVER_VERSION).toBe(pkg.version));
```

- [ ] **Step 2:** `npm test` — FAIL (module absent).
- [ ] **Step 3 (implement):**

```ts
import pkg from "../package.json";

export const PINNED_SERVER_VERSION: string = pkg.version;

export function parseServerVersion(stdout: string): string | null {
  const m = /^compact-analyzer (\d+\.\d+\.\d+\S*)\s*$/m.exec(stdout.trim());
  return m ? m[1] : null;
}

function split(v: string): { major: number; minor: number } | null {
  const m = /^(\d+)\.(\d+)\.\d+/.exec(v);
  return m ? { major: Number(m[1]), minor: Number(m[2]) } : null;
}

export function isCompatible(serverVersion: string, pinned: string = PINNED_SERVER_VERSION): boolean {
  const s = split(serverVersion);
  const p = split(pinned);
  if (!s || !p) return false;
  // Pre-1.0 the minor is the breaking axis; from 1.0 the major is.
  return p.major === 0 ? s.major === 0 && s.minor === p.minor : s.major === p.major;
}
```

(Hand-rolled — no `semver` dep needed for this comparison.)
- [ ] **Step 4:** `npm test` — PASS. **Step 5 (gate + commit):** `feat(vscode): pinned server-version compatibility policy (0.x minor-match)`

---

## Task 5: Platform/artifact mapping (`src/platform.ts`)

**Files:** Create `src/platform.ts`, `src/test/unit/platform.test.ts`.

**Interfaces:** Produces `PlatformTarget`, `currentTarget`, `artifactName` (Shared Interfaces). Pure. Test fixtures = Task 3's recorded `dist plan` names (Errata).

**Context:** Supported matrix: `darwin/arm64 → aarch64-apple-darwin`, `darwin/x64 → x86_64-apple-darwin`, `linux/x64 → x86_64-unknown-linux-gnu`, `win32/x64 → x86_64-pc-windows-msvc` (binary `compact-analyzer.exe`). Everything else → `null` (upstream turns it into "unsupported platform — install manually"; `linux/arm64` is the plausible future add, note it). `archiveExt` is `".tar.gz"` for ALL targets (Resolved OQ4). `artifactName` returns the Task 3 fixture names verbatim — a literal table, no cleverness.

- [ ] **Step 1 (failing tests):** the four supported mappings (triple + binaryName + archiveExt `.tar.gz` incl. `win32/x64 → compact-analyzer.exe`), `currentTarget("linux","arm64") === null`, and `artifactName(<each target>)` equal to the four names recorded in Task 3's Errata fixture (write them in literally).
- [ ] **Step 2:** FAIL. **Step 3:** implement the literal table. **Step 4:** PASS.
- [ ] **Step 5 (gate + commit):** `feat(vscode): platform-to-release-artifact mapping for the four v1 targets`

---

## Task 6: Download, checksum-verify, install (`src/download.ts`) — OPUS review

**Files:** Create `src/download.ts`, `src/test/unit/download.test.ts`, fixture archives under `src/test/unit/fixtures/` (generated by a small script or checked-in bytes — document which).

**Interfaces:** Produces `ServerManifest`, `DownloadError`, `verifySha256`, `downloadAndInstall` (Shared Interfaces). Pure Node (injected `fetchImpl`) — no `vscode` import. This is the security trust anchor — review on opus.

**Context:** Flow: build URL (`baseUrl` default `https://github.com/devrelaicom/compact-analyzer/releases/download/<tag>/` with the Task 3 tag-format fixture), `fetchImpl` (follow redirects — V10), sha256 via `node:crypto` against `manifest.artifacts[triple].sha256` **before extraction**, extract `.tar.gz` via `node:zlib.gunzipSync` + the minimal tar reader below (dist archives contain the binary at top level or under a single directory — Task 3's Step-3 `tar tzf` output is the fixture for the entry path), `chmod 0o755` on Unix (V9), write via temp-file + `fs.renameSync` atomic rename to `<destDir>/<version>/<binaryName>`. Every failure throws `DownloadError` whose `userGuidance` names the manual alternatives (brew, shell installer, Releases page, the `compact-analyzer.serverPath` setting).

Minimal tar reader (complete — ustar, regular files only, which is all dist archives contain):

```ts
interface TarEntry { name: string; data: Uint8Array; }

function readString(buf: Uint8Array, off: number, len: number): string {
  const slice = buf.subarray(off, off + len);
  const end = slice.indexOf(0);
  return new TextDecoder().decode(end === -1 ? slice : slice.subarray(0, end));
}

/** Iterate regular-file entries of an uncompressed tar buffer. */
export function* tarEntries(buf: Uint8Array): Generator<TarEntry> {
  let off = 0;
  while (off + 512 <= buf.length) {
    const header = buf.subarray(off, off + 512);
    if (header.every((b) => b === 0)) break;                    // end-of-archive
    const name = readString(header, 0, 100);
    const size = Number.parseInt(readString(header, 124, 12).trim() || "0", 8);
    const typeflag = header[156];
    const prefix = readString(header, 345, 155);                // ustar long-name prefix
    const full = prefix ? `${prefix}/${name}` : name;
    if (typeflag === 0x30 || typeflag === 0) {                  // '0' or NUL = regular file
      yield { name: full, data: buf.subarray(off + 512, off + 512 + size) };
    }
    off += 512 + Math.ceil(size / 512) * 512;
  }
}
```

- [ ] **Step 1 (failing tests — injected fake `fetch` serving fixture bytes; `fs.mkdtempSync` temp dirs):** (a) happy path installs, returns the binary path, file is executable on Unix (`describe.skipIf(process.platform === "win32")` for the mode assert); (b) checksum mismatch ⇒ `DownloadError`, NOTHING at the final path; (c) HTTP 404 ⇒ `DownloadError`, guidance mentions manual install + `serverPath`; (d) triple missing from manifest ⇒ `DownloadError`; (e) re-install over an existing version dir succeeds (idempotent); (f) tarEntries round-trips a fixture archive built with system `tar czf` containing one executable.
- [ ] **Step 2:** FAIL. **Step 3:** implement (keep extraction behind `extractArchive(bytes, destDir)` so the format decision stays localised). **Step 4:** PASS.
- [ ] **Step 5 (gate + commit):** `feat(vscode): checksum-verified server download into extension storage`

---

## Task 7: Server acquisition orchestration (`src/acquire.ts`)

**Files:** Create `src/acquire.ts`, `src/test/unit/acquire.test.ts`.

**Interfaces:** Produces `acquireServer` + types (Shared Interfaces). Consumes Tasks 4/5/6. Pure Node with injected `env`/`exec`/`fetchImpl`.

**Context:** Order (milestone-fixed): settings path → `PATH` → storage cache → download. Every candidate: `exec(binary, ["--version"], 2000)` → `parseServerVersion` → `isCompatible`. Semantics: explicitly-configured `serverPath` that is missing/unrunnable/incompatible ⇒ **hard `AcquireFailure` naming the setting** (departure D5 — respect user intent, never silently override); PATH/storage failures log and fall through; download failure after all fallthroughs returns the `DownloadError` guidance. PATH probe scans `env.PATH` entries for `binaryName` (V9: `.exe`/`PATHEXT` on Windows). Default `ExecProbe` = `child_process.spawn` with a timeout + kill (M4's hand-rolled-poll precedent). Test approach mirrors M4's `Toolchain::discover`: fake executables in temp dirs — a shell script printing a chosen `--version` line; `describe.skipIf(process.platform === "win32")` where exec bits matter.

- [ ] **Step 1 (failing tests):** (a) configured path + compatible fake ⇒ `{ok:true, source:"settings"}`; (b) configured path + WRONG minor ⇒ `AcquireFailure` whose reason mentions both the version and `compact-analyzer.serverPath` (no fallthrough — assert download was never attempted via a throwing fetchImpl); (c) no config + fake on PATH ⇒ `source:"path"`; (d) incompatible PATH binary + valid storage cache ⇒ `source:"storage"`; (e) nothing anywhere + fake fetch serving the Task-6 fixture ⇒ `source:"downloaded"` and the installed binary re-validated; (f) nothing + failing fetch ⇒ `AcquireFailure` with manual-install guidance; (g) hanging exec probe ⇒ timeout treated as invalid candidate, acquisition never wedges (test with a probe that resolves after the timeout).
- [ ] **Step 2:** FAIL. **Step 3:** implement incl. the real default `ExecProbe`. **Step 4:** PASS.
- [ ] **Step 5 (gate + commit):** `feat(vscode): server acquisition (settings, PATH, cache, pinned download) with version handshake`

---

## Task 8: Settings surface + initializationOptions mapping (`src/config.ts` + contributes)

**Files:** Create `src/config.ts`, `src/test/unit/config.test.ts` (mapping tested via an injected config-reader so no extension host is needed); modify `package.json` (`contributes.configuration`).

**Interfaces:** Produces `initializationOptionsFromConfig`, `configuredServerPath` + the settings table (Shared Interfaces). Consumed by Task 9.

**Context:** The mapping must equal G1's keys **exactly**. Load-bearing rules: `importSearchPath` omitted when empty (G2 — an explicit `[]` would suppress the server's `COMPACT_PATH` fallback); `toolchainPath` omitted when `""`; `compileOnSave`/`formatting` sent explicitly (decouples client defaults from server defaults). `contributes.configuration`: the six settings with markdown descriptions; `serverPath` scope `machine-overridable`; `toolchainPath`'s description carries the OQ11 mapping ("the server refers to this as `toolchainPath` in `initializationOptions`") + G5's three semantics (file-or-dir; disables PATH search; unresolvable ⇒ features silently off + one INFO notice). Config changes require a server restart (G3) — the listener itself lands in Task 9; this task only ships the mapping + schema.

- [ ] **Step 1 (failing tests, fake config getter):** defaults ⇒ exactly `{ compileOnSave: true, formatting: true }` (NO `importSearchPath`/`toolchainPath` keys — assert via `Object.keys`); populated values pass through; `importSearchPath: []` and `toolchainPath: ""` are omitted; non-default booleans pass through as `false`.
- [ ] **Step 2:** FAIL. **Step 3:** implement mapping (a thin injected-reader core + a `vscode.workspace.getConfiguration`-backed wrapper) + add the six settings to `contributes.configuration`. **Step 4:** PASS + `npx vsce ls` still validates.
- [ ] **Step 5 (gate + commit):** `feat(vscode): settings surface mapped to server initializationOptions`

---

## Task 9: `activate()` — LanguageClient wiring, restart command, graceful failure

**Files:** Modify `src/extension.ts`; extract host-free logic to keep `extension.ts` under ~150 lines (unit-test extracted logic only if any emerges).

**Interfaces:** Consumes Tasks 7/8 + `vscode-languageclient` (**V1 — read the installed `.d.ts` FIRST, record the actual shapes in Errata**). Produces `activate`/`deactivate`, `ExtensionApi`, command `compact-analyzer.restartServer`.

**Context:** Sequence: load `server-manifest.json` → `acquireServer(...)`; on `AcquireFailure` → ONE `window.showErrorMessage` with a "How to install" button (opens the README anchor), status "unavailable", **return normally** (assets keep working); on success → `LanguageClient` with stdio `Executable`, `documentSelector: [{ language: "compact" }]`, `initializationOptions` from Task 8, dedicated output channel (server stderr lands there — G7's startup toolchain line becomes the first troubleshooting surface), `client.start()`. Post-initialize: assert `serverInfo.version` compatibility (belt-and-braces, Resolved OQ7; mismatch ⇒ stop + actionable message). First-run download UX: `window.withProgress` ("Downloading compact-analyzer 0.1.0…"). `deactivate` → `client.stop()` (FAST-EXIT holds server-side, G8 — no timeout gymnastics). Restart command: stop → re-acquire (config may have changed) → start; register a `workspace.onDidChangeConfiguration` listener that offers "Restart server" on any `compact-analyzer.*` change (G3 — mandatory). Coexistence (Resolved OQ10): if `vscode.extensions.getExtension("midnightnetwork.compact")` is present, show a ONE-TIME (globalState-flagged) hint that the old extension can be uninstalled.

- [ ] **Step 1:** Read `node_modules/vscode-languageclient/lib/node/main.d.ts` (+ `common`) for the exact constructor/`Executable`/options/start/stop shapes; record as an Errata entry. Then write the wiring per Context.
- [ ] **Step 2 (manual verification, pre-E2E):** `cargo build -p compact-analyzer`; F5 / `code --extensionDevelopmentPath` with `compact-analyzer.serverPath` → `target/debug/compact-analyzer`; open a `.compact` file with a syntax error → diagnostics appear; hover/completion/semantic tokens work; output channel shows the server's startup stderr incl. the toolchain line; save with the real toolchain present → a `compactc`-sourced diagnostic on a compile error (E6 proof live). Then: unset `serverPath`, scrub PATH, point the manifest at a bogus URL → activation completes; exactly ONE guidance message; grammar still highlights. Toggle a setting → restart prompt appears; restart command works mid-session.
- [ ] **Step 3 (gate + commit):** `feat(vscode): LSP client activation with acquisition, handshake, restart command`

---

## Task 10: Activation smoke test (E2E, CI-runnable)

**Files:** Create `src/test/e2e/` (runner + suite per **V8** — verify the current `@vscode/test-cli`/`test-electron` harness shape first), fixture workspace `src/test/e2e/fixture/` (one valid + one invalid `.compact`); modify `package.json` (`test:e2e` script), `.vscodeignore` (exclude e2e).

**Interfaces:** Consumes `ExtensionApi` (Task 9).

**Context:** Milestone: "minimal activation smoke test; primary validation is daily dogfooding." TWO scenarios, hermetic (locally-built server, no network): (1) `compact-analyzer.serverPath` = fresh `cargo build -p compact-analyzer` binary → opening the invalid fixture yields ≥1 diagnostic with `source` containing `compact-analyzer` within 30 s, and `api.serverStatus() === "running"` (CR-4's cap is invisible here — the fixture produces far fewer than 100 diagnostics, G8); (2) `serverPath` = nonexistent path → activation completes, `api.serverStatus() === "unavailable"`, no unhandled rejection. Linux CI runs under `xvfb-run` (V8).

- [ ] **Step 1:** Verify the harness (V8), then write the two tests + runner (VS Code download step; fixture workspace + injected settings).
- [ ] **Step 2:** `cargo build -p compact-analyzer && npm run test:e2e` — both PASS locally.
- [ ] **Step 3 (gate + commit):** `test(vscode): activation smoke E2E against the locally built server`

---

## Task 11: Lockstep release plumbing — manifest baking, VSIX packaging, publish jobs, CI

**Files:** Create `scripts/gen-server-manifest.mjs`, `.github/workflows/extension-release.yml`; modify `.github/workflows/ci.yml` (two additive jobs), `editors/vscode/package.json` (release scripts), `editors/vscode/server-manifest.json` (checked-in dev placeholder for `0.1.0`).

**Interfaces:** Consumes Task 3's checksum-shape fixture; produces `ServerManifest` JSON (Task 6's schema) + the publish pipeline.

**Context:** Ordering problem: the VSIX must embed the FINAL artifact checksums, which exist only after dist builds. Chosen approach (draft option b): a **separate workflow** `extension-release.yml` on `release: { types: [published] }` — download the release's checksum assets for the tag → `gen-server-manifest` → `npm ci && npm run build && npx vsce package` → upload the VSIX to the same release → **secret-guarded** `vsce publish` / `npx ovsx publish`. Decoupled from dist's generated workflow, re-runnable when a publish leg fails. Secret guarding: the `secrets` context is not usable directly in step `if:`s — use env indirection at job level (`env: VSCE_PAT: ${{ secrets.VSCE_PAT }}` … `if: env.VSCE_PAT != ''`) — VERIFY against current GHA docs and record. **Every vsce/ovsx flag verified against `--help` of the pinned versions (V3), never memory.** CI additions (additive-only, matching M4's Task-11 discipline — zero deletions in existing jobs): `extension` (ubuntu: `npm ci && npm run lint && npm run check && npm test && npm run build && npx vsce package`) + `extension-e2e` (ubuntu: `cargo build -p compact-analyzer --locked`, then `xvfb-run -a npm run test:e2e`); both `timeout-minutes: 15`, working-directory `editors/vscode`.

- [ ] **Step 1:** Write `gen-server-manifest.mjs` + a vitest test driven by a fixture copy of Task 3's checksum output (Errata fixture) → expected `server-manifest.json` (schema of Task 6); npm script `gen-manifest`. Also write the `0.1.0` dev-placeholder `server-manifest.json` (real artifact names, all-zero sha256 values, clearly commented as placeholder — the E2E never downloads).
- [ ] **Step 2:** Write `extension-release.yml` per Context (download checksums → gen-manifest → package → upload VSIX → guarded publishes).
- [ ] **Step 3:** Append the two CI jobs to `ci.yml` (verify existing jobs byte-unchanged via `git diff --stat`). Push the branch; confirm `extension` + `extension-e2e` pass on GitHub Actions. The release workflow is exercised by the first real tag (H-gated) — add a `workflow_dispatch` input for a dry-run (skip-upload/skip-publish) if cheap; else record "first-tag-verified" as an explicit DoD deferral.
- [ ] **Step 4 (gate + commit):** root Rust gate (ci.yml touched). Commit: `build(release): lockstep VSIX packaging with baked server manifest; guarded marketplace/open-vsx publish; extension CI`

---

## Task 12: Docs, editor guides, dogfood checklist, milestone close-out

**Files:** Modify `README.md`; create `docs/editors.md`, `editors/vscode/README.md` (marketplace page), `editors/vscode/CHANGELOG.md`; modify `.superpowers/milestones/README.md` + `milestone-status` memory (at the very end, committed on the branch).

**Context:** README overhaul: what it is, install paths (VS Code extension → Marketplace/Open VSX; binary → brew / shell installer / PowerShell / GitHub Releases / build-from-source), feature table (M1–M4), configuration reference (the settings table incl. the OQ11 mapping and G5's toolchainPath semantics), the G3 restart caveat ("config is read at server start; installing `compact` mid-session needs a server restart"), unsigned-binary/Gatekeeper note (H6), troubleshooting via the output channel (G7 stderr line). Toolchain install links point at `midnightntwrk/compact` releases (Resolved OQ13). `docs/editors.md`: setup-only for Zed / Neovim / Helix — migrate the README's Neovim snippet; **VERIFY each editor's current LSP-config schema at write time**; test Neovim locally (known-working), mark the others "verified against <editor> <version>" or "untested". Marketplace README: user-facing succession statement (OQ2 wording), attribution note, settings table, the "no edit = already formatted" formatting note (G8).

- [ ] **Step 1:** Write all docs; every editor snippet carries a "verified against <editor> <version> on <date>" line.
- [ ] **Step 2 (dogfood checklist — the v1 release bar):** on a real multi-file Compact project via the packaged VSIX (`code --install-extension *.vsix`): open/edit/save; cross-file goto/rename; completion incl. `.`-trigger; semantic tokens visible; folding/selection; compile-on-save diagnostics with toolchain present (`source: "compactc"`); formatting; toolchain-absent one-time notice (scrubbed PATH); restart command; acquisition download path once a real release exists (else explicitly deferred). Record outcomes in the progress ledger.
- [ ] **Step 3:** Flip M5 → Done in `.superpowers/milestones/README.md`; update the `milestone-status` memory (v1 = M1–M5); final ledger entry.
- [ ] **Step 4 (commit):** `docs: installation + configuration + other-editor setup; flip M5 to Done`

---

## Deliberate departures / decisions already made (reviewers: these are intentional)

1. **Host-agnostic acquisition modules with injected deps** (fetch/exec/env) — mirrors the Rust layering rule (G9); unit-testable without an extension host.
2. **E2E runs against the locally-built server, not a download** — hermetic CI; the download path is covered by unit tests + the dogfood checklist against a real release.
3. **`.tar.gz` on ALL targets (including Windows)** — diverges from compactp's defaults AND from the draft's zip-on-Windows assumption; one dependency-free extraction path. dist supports it (E3).
4. **Icons/logos NOT adapted** — Apache-2.0 covers the code/assets, Midnight branding is a trademark concern; original artwork is H5.
5. **Explicit `serverPath` failing validation is a hard failure, not a fallthrough** — user intent over availability. Deliberate asymmetry with the server's soft-failing `toolchainPath` (G5): document both, "fix" neither.
6. **Problem matchers dropped** (Resolved OQ8) — with the E7 evidence recorded in NOTES.md; revisit only if a future compiler restores a stable single-line absolute-path shape.
7. **Task 3 (dist) runs before the TS modules** so artifact names/checksum shapes are real fixtures, not stubs (draft self-review 7 adopted).
8. **vitest + esbuild** over the official extension's jest/webpack; `artifactName(target)` dropped the draft's unused `version` parameter (dist names are versionless — confirm at V6).
9. **`@types/vscode` pinned to the engine floor (1.91.x), not latest** — vsce enforces consistency (E1).

## Errata

_(During execution: when a fixture/API/flag turns out wrong, fix it empirically, record the correction here with task number and root cause, and COMMIT the Errata edit promptly. Expected heavy hitters: V1 languageclient shapes (Task 9), V6 dist naming (Task 3 → 5/6/11), V3 vsce/ovsx flags (Task 11).)_

- **Task 1 / V2 (manifest rules):** `activationEvents` is MANDATORY at `engines.vscode: ^1.91.0` whenever `main` is present — `vsce` hard-errors ("Manifest needs the 'activationEvents' property, given it has a 'main' property") without it. Auto-generation from `contributes` does not remove the requirement when `contributes` is empty. Scaffold ships `"activationEvents": []`; later tasks add `onLanguage:compact`.
- **Task 1 / V3 (vsce, partial — full at Task 11):** `.vscodeignore` is exclude-only (gitignore-style); everything unmatched ships. `vsce` bundles production-dependency `node_modules` by default, so `node_modules/**` MUST be excluded for an esbuild-bundled extension. `vsce package` runs the `vscode:prepublish` npm script first (wired to `npm run build` → dist always fresh). Missing LICENSE is a non-blocking WARNING at `package` time (LICENSE lands in Task 2). `--no-dependencies` is the right `vsce package` flag for a bundled extension (Task 11).
- **Task 1 / V4 (extension-host Node):** VS Code 1.91.0 bundles Electron 29.4.0 → Node **20.9.0** (Chromium 122). So the extension RUNTIME floor is Node 20.9 — global `fetch` (stable ≥18), `node:crypto`, `node:zlib` are ALL available (unblocks Tasks 6–7 without polyfills). Dev-tooling floor is higher — Node **20.19** — because `eslint@10.7.0` engines are `^20.19.0 || ^22.13.0 || >=24`; pinned via `engines.node >=20.19.0` + `.nvmrc`.
- **Task 1 / toolchain deviation:** `typescript` pinned **6.0.3** (not latest 7.0.2): `typescript-eslint@8.63.0` peer-caps `typescript >=4.8.4 <6.1.0`; no stable typescript-eslint supports TS7 yet. `eslint@10.7.0` (major) is in-range for typescript-eslint's `eslint ^8.57||^9||^10` peer. `engines.node` is empirically vsce-safe (no reject/warn; VSIX file-list unchanged).
- **Task 2 / attribution (file-list correction + §4(a) compliance):** the File Structure list omitted `LICENSE-Apache-2.0.txt` — Apache-2.0 §4(a) *requires shipping a COPY of the licence text*, not a URL. The extension therefore ships THREE licence-related files: `LICENSE` (MIT, the extension's own), `LICENSE-Apache-2.0.txt` (verbatim upstream Apache-2.0, fetched from the pinned commit — the §4(a) copy), and `THIRD-PARTY-NOTICES.md` (attribution + change table = §4(b–d)). `LICENSE-Apache-2.0.txt` lives at the extension root (NOT under `assets/`, which is `.vscodeignore`-excluded) and must appear in `npx vsce ls`. **M6 note:** any asset-regeneration automation must re-fetch/preserve this bundled licence copy, not just the notices.
- **Task 2 / grammar 0.23 deltas (empirically compiler-verified):** removed phantom builtin types `JubjubScalar`/`Secp256k1Base`/`Secp256k1Scalar` (real compiler → `unbound identifier`); added `Unsigned`/`Integer`; removed `emit`/`implements` (not 0.23 keywords) + the JS-reserved-word pattern. 42-keyword ground-truth set generated from `../compactp/crates/compactp_lexer/src/lib.rs` (command in `assets/NOTES.md`; controller-reproduced byte-identical). Snippet 0.23 fixes: `include "std";`→`import CompactStandardLibrary;`; bare `assert cond "msg";`→`assert(cond, "msg");`; `pragma language_version 0.23;`.

## Definition of Done

- All 12 tasks complete; every extension gate + the root Rust gate green (272-test baseline intact); per-task conventional commits; `package-lock.json` committed.
- Ground-truth §G1–G10 held (or divergences recorded as Errata + reconciled); thin-client invariant: zero analysis logic in TypeScript; Rust pins untouched; only dist-related Cargo changes.
- Acquisition matrix proven by tests: settings (incl. hard-fail), PATH, storage, download (checksum-verified, atomic), never-broken-activation failure path, version handshake refusing a minor-mismatched server.
- E2E smoke green in CI (xvfb) against the locally-built server; existing Rust CI jobs byte-unchanged and green.
- Release pipeline proven: `dist plan`/`dist build` (host) succeed with `.tar.gz` archives; the tag-driven workflow builds 4-target artifacts + installers + tap formula; the extension leg bakes real checksums into the VSIX and uploads it; publish steps exist secret-guarded. **First real `v0.1.0` tag executed end-to-end OR every leg dry-run and the tag explicitly deferred by the human.**
- H2–H6 complete or consciously deferred (publish legs dormant).
- Attribution in place: THIRD-PARTY-NOTICES.md (upstream repo + commit `fdfc61cf…` + file list); NOTES.md regeneration/audit procedure + E7 matcher evidence + both-repos-by-role record; no Midnight logos.
- Docs shipped: README overhaul, marketplace README, `docs/editors.md` (Zed/Neovim/Helix), CHANGELOG.
- Dogfood checklist executed on a real multi-file project via the packaged VSIX, outcomes in the ledger.
- Final whole-branch review on **opus** (base = `git merge-base main HEAD`); one fix wave for Critical/Important; re-verify; ff-merge to `main`; flip M5 → Done + update `milestone-status` memory — closing out v1.
