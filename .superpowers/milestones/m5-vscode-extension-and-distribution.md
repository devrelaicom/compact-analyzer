# M5 — VS Code extension + distribution

## Goal

Ship the first-party VS Code extension (`editors/vscode`) and the binary release
pipeline, making compact-analyzer installable by a stranger in one step: Marketplace /
Open VSX for the extension, cargo-dist GitHub Releases for the server binary.

## Why

This is the milestone that turns the project from "works on Aaron's machine" into the
successor of the sunsetted official Compact extension. Everything before M5 is
server-side; M5 is the delivery vehicle and the public face.

## Includes

- **Extension** (TypeScript, `vscode-languageclient` over stdio):
  - Server acquisition order: user-configured path → `PATH` → download the platform
    binary for the extension's **pinned** server version from GitHub Releases
    (checksum-verified) into extension storage.
  - Extension and server release in lockstep; the extension refuses server versions
    outside its pinned compatible range — for 0.x releases the **minor version must
    match**.
  - TextMate grammar, language-configuration, and snippets bundled so highlighting
    works before/without the server — adapted from the official extension's Apache-2.0
    assets **with attribution** — progressively superseded by semantic tokens (M3).
  - Settings surface: everything from M2b/M4 config (import path, toolchain override,
    compile-on-save toggle, formatting toggle) plus server path override.
  - Minimal activation smoke test; primary validation is daily dogfooding.
- **Publishing:** VS Code Marketplace + Open VSX.
- **Server distribution:** cargo-dist on GitHub Releases — macOS arm64/x64, Linux x64,
  Windows x64 — with shell/PowerShell installers and a Homebrew tap (the same pipeline
  compactp uses; crib its config).
- README/docs for other editors (Zed, Neovim, Helix): setup instructions only, no
  first-party plugins.

## Excludes

- First-party plugins for any editor other than VS Code (docs only, per spec decision).
- Publishing workspace crates to crates.io (the binary is the product; stay unpublished
  until there's a concrete reason).
- Debugging support, test runners, or anything beyond LSP features.
- Auto-updating the server outside the pinned-version download flow.

## Facts / constraints to honor

- **Branding is fully independent:** the binary is `compact-analyzer`. The reserved
  `compact-language-server` name from the official Zed extension is deliberately
  ignored. Extension ID/display name should make the succession from the official
  extension clear without claiming to be official.
- The official extension provided only TextMate grammar + snippets + regex problem
  matchers; its Apache-2.0 assets may be adapted with attribution (license file entry).
- Downloads must be checksum-verified; failed acquisition degrades to a clear message
  with manual-install instructions, never a broken activation.
- Publisher accounts (Marketplace PAT, Open VSX namespace) and release signing are
  human-owned setup steps — flag them early, they gate the final release.

## Dependencies / notes

- Meaningful only after M2–M4: the extension is thin; its value is the server behind it.
  cargo-dist scaffolding could land earlier if a distributable binary helps dogfooding.
- Version-compatibility policy (pinned minor for 0.x) needs a version handshake —
  consider putting the server version in the `initialize` response `serverInfo` and
  checking it client-side.
