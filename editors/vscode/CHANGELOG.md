# Changelog

All notable changes to the Compact Analyzer extension are documented here. The
format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.0

Initial release — the community successor to the sunsetted official Compact
VS Code extension. A thin client for the `compact-analyzer` language server.

### Added

- Live syntax diagnostics as you type (no compiler round-trip).
- Navigation: goto-definition, find-references, rename, hover, and document
  symbols.
- Cross-file `import` resolution and workspace symbols.
- Code completion, including `.`-triggered member completion (ledger ADT
  methods, struct fields, enum variants) and import-prefix/alias completion.
- Full-document semantic tokens.
- Folding ranges and selection ranges.
- Compile-on-save diagnostics via the `compact` CLI (sourced from `compactc`),
  when the toolchain is available.
- Document formatting via `compact format`, when the toolchain is available.
- Automatic language-server acquisition (settings path → `PATH` → download
  cache → pinned download), overridable with `compact-analyzer.serverPath`.
- Settings under `compact-analyzer.*`: `serverPath`, `importSearchPath`,
  `toolchainPath`, `compileOnSave`, `formatting`, and `trace.server`.
- **Compact Analyzer: Restart Server** command, plus an offer to restart when a
  setting changes (the server reads configuration only at startup).
- TextMate grammar, language configuration, and snippets for `.compact` files,
  adapted from the official upstream editor support under Apache-2.0 (see
  `THIRD-PARTY-NOTICES.md`).
