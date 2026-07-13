<p align="center">
  <img src="assets/compact-analyzer-logo.svg" alt="compact-analyzer logo" width="128" height="128">
</p>

<h1 align="center">compact-analyzer</h1>

A [rust-analyzer](https://rust-analyzer.github.io/)-style language server for
the [Compact](https://docs.midnight.network/compact) smart contract language
(Midnight Network). It gives any LSP-capable editor live diagnostics,
navigation, completion, and more for `.compact` files.

compact-analyzer is the community successor to the official Compact VS Code
extension, which has been sunsetted. It ships its own thin-client
[VS Code extension](editors/vscode/) and works with any editor that speaks the
Language Server Protocol (see [docs/editors.md](docs/editors.md) for Zed,
Neovim, and Helix).

Built on [compactp](https://github.com/devrelaicom/compactp), a lossless,
error-tolerant parser frontend for Compact. The server speaks LSP over stdio.

## Features

| Feature | What it does |
| --- | --- |
| Live syntax diagnostics | Reports syntax errors as you type — no compiler round-trip. |
| Navigation | Goto-definition, find-references, rename, hover, and document symbols. |
| Cross-file awareness | Resolves cross-file `import`s and provides workspace symbols. |
| Code completion | Context-aware completion, including `.`-triggered members, ledger ADT methods, struct fields, enum variants, and import-prefix/alias completion. |
| Semantic tokens | Full-document semantic highlighting. |
| Folding ranges | Structural code folding. |
| Selection ranges | Smart (expand/shrink) selection. |
| Compile-on-save diagnostics | Runs the `compact` CLI (which sources `compactc`) on save for compiler diagnostics. |
| Document formatting | Formats a document via `compact format`. |

Positions are reported in correct UTF-16 offsets, and the server never dies on
malformed code.

The last two features (compile-on-save diagnostics and formatting) require the
Compact toolchain to be installed and discoverable — see
[Configuration](#configuration).

## Install

compact-analyzer has two pieces: the **editor client** (e.g. the VS Code
extension) and the **language server binary**. In VS Code the extension
acquires the server for you; other editors need the binary on disk.

### VS Code extension

The intended install path is the **VS Code Marketplace** and **Open VSX**
(search for "Compact Analyzer", publisher `aaronbassett`). The first publish is
still pending; until it lands you can build and install the VSIX locally:

```sh
cd editors/vscode
npm ci
npx vsce package --no-dependencies
code --install-extension compact-analyzer-*.vsix
```

The extension bundles a small client and downloads a matching server binary on
first use; you can also point it at your own binary with
`compact-analyzer.serverPath` (see [Configuration](#configuration)).

### Language server binary

For editors other than VS Code — or to override the extension's bundled
server — install the `compact-analyzer` binary directly:

- **Homebrew** (macOS/Linux) — available once the first release is published
  (the tap formula is created by the release pipeline):

  ```sh
  brew install aaronbassett/homebrew-tap/compact-analyzer
  ```

- **Shell / PowerShell installers and prebuilt archives** from
  [GitHub Releases](https://github.com/devrelaicom/compact-analyzer/releases).
  Each release is built with [cargo-dist](https://opensource.axo.dev/cargo-dist/)
  and carries copy-paste install commands plus `.tar.gz` archives for
  macOS (aarch64/x86_64), Linux (x86_64), and Windows (x86_64). The one-liners
  look like this (available once the first release is published):

  ```sh
  # macOS / Linux
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/devrelaicom/compact-analyzer/releases/latest/download/compact-analyzer-installer.sh | sh
  ```

  ```powershell
  # Windows
  powershell -ExecutionPolicy Bypass -c "irm https://github.com/devrelaicom/compact-analyzer/releases/latest/download/compact-analyzer-installer.ps1 | iex"
  ```

  The shell/PowerShell installers place the binary in `CARGO_HOME`
  (typically `~/.cargo/bin/compact-analyzer`). Prefer the archives if you want
  to place the binary yourself.

- **Build from source** (requires a recent stable Rust toolchain, 1.90+):

  ```sh
  cargo build --release -p compact-analyzer
  ./target/release/compact-analyzer --version
  ```

Once the binary is installed, point your editor at it — see
[docs/editors.md](docs/editors.md) for Zed, Neovim, and Helix, or set
`compact-analyzer.serverPath` in VS Code.

### Compact toolchain (optional)

Compile-on-save diagnostics and formatting shell out to the `compact` CLI.
Install it from the Compact distribution repository,
[`midnightntwrk/compact`](https://github.com/midnightntwrk/compact/releases)
(follow the install instructions on the releases page). If the toolchain is
absent, compact-analyzer still provides everything that does not need the
compiler (diagnostics-as-you-type, navigation, completion, and so on).

## Configuration

All settings live under the `compact-analyzer.*` namespace. In VS Code they are
regular workspace/user settings; other editors pass the same values through
their LSP `initializationOptions` (see [docs/editors.md](docs/editors.md)).

| Setting | Type | Default | Meaning |
| --- | --- | --- | --- |
| `compact-analyzer.serverPath` | `string` | `""` | Absolute path to a `compact-analyzer` binary; overrides auto-acquisition (settings → `PATH` → download cache → download). Machine-overridable. Client-side only (not sent to the server). |
| `compact-analyzer.importSearchPath` | `string[]` | `[]` | Import search directories, sent to the server as `initializationOptions.importSearchPath`. **Left empty, the key is omitted and the server falls back to the `COMPACT_PATH` environment variable** — sending an explicit `[]` would suppress that fallback. |
| `compact-analyzer.toolchainPath` | `string` | `""` | Path to the `compact` CLI — **a file _or_ a directory** containing it. The server refers to this as `toolchainPath` in its `initializationOptions`. When set, the server does **not** search `PATH`; an unresolvable value silently disables the toolchain features (compile-on-save, formatting) and shows a single informational notice. |
| `compact-analyzer.compileOnSave` | `boolean` | `true` | Run `compact compile` on save for compiler diagnostics (needs the toolchain). |
| `compact-analyzer.formatting` | `boolean` | `true` | Provide document formatting via `compact format` (needs the toolchain). |
| `compact-analyzer.trace.server` | `"off"` \| `"messages"` \| `"verbose"` | `"off"` | LSP trace verbosity (standard `vscode-languageclient` tracing). Client-side only. |

### Configuration changes require a restart

The server reads its configuration and discovers the toolchain **only at
startup** — there is no live config reload. Changing any `compact-analyzer.*`
setting, or installing the `compact` toolchain mid-session, takes effect only
after the server restarts. In VS Code, run **"Compact Analyzer: Restart
Server"** from the command palette (the extension also offers to restart when a
setting changes). Other editors have their own LSP-restart command.

## macOS Gatekeeper (unsigned binaries)

The released binaries are **unsigned** — cargo-dist ships them without a
Developer ID signature or notarisation (the same as the Compact toolchain
itself). There is no notarisation for v1.

Homebrew and the shell/PowerShell installers fetch the binary with `curl`/`irm`,
which does **not** apply the `com.apple.quarantine` flag, so you generally will
not hit Gatekeeper via those paths. You mainly see it when you download an
archive **through a browser**: macOS quarantines the `.tar.gz`, and the flag can
carry over to the `compact-analyzer` binary you extract from it.

To allow the extracted binary, use any one of these:

- **Right-click the binary in Finder → Open**, then confirm; or
- **System Settings → Privacy & Security → "Open Anyway"** after the first
  blocked launch; or
- Clear the quarantine flag from the extracted binary (adjust the path to
  wherever you placed it):

  ```sh
  xattr -d com.apple.quarantine ./compact-analyzer
  ```

Use the non-recursive `-d` here: cargo-dist ships a single CLI binary inside a
`.tar.gz`, not a `.app` bundle, so you are clearing the attribute on one file.
The recursive form (`xattr -dr com.apple.quarantine <dir>`) is only needed when
you want to clear an entire directory tree or an application bundle.

After allowing the binary, point the extension at it with
`compact-analyzer.serverPath`, or make sure it is on your editor's `PATH`.

## Troubleshooting

On startup the server writes a few status lines to stderr, which the VS Code
extension pipes into its **"Compact Analyzer"** output channel (open the Output
panel and pick "Compact Analyzer" from the dropdown). One of them reports the
resolved toolchain, for example:

```
compact-analyzer: compact toolchain <version> (language <version>) at <path>
```

or, when no toolchain was found:

```
no compact toolchain found; compile-on-save disabled
```

Check that channel first: it tells you which server binary is running and
whether the toolchain was discovered. If compile-on-save diagnostics or
formatting are missing, the toolchain line is the place to look — confirm
`compact-analyzer.toolchainPath` (or `PATH`) resolves to a real `compact` CLI,
then restart the server.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

To develop against a local compactp checkout, add (but never commit):

```toml
# Cargo.toml (workspace root)
[patch.crates-io]
compactp_parser = { path = "../compactp/crates/compactp_parser" }
compactp_syntax = { path = "../compactp/crates/compactp_syntax" }
compactp_diagnostics = { path = "../compactp/crates/compactp_diagnostics" }
```

The VS Code extension lives in [editors/vscode/](editors/vscode/); see its
[README](editors/vscode/README.md) and
[THIRD-PARTY-NOTICES.md](editors/vscode/THIRD-PARTY-NOTICES.md).

## License

MIT
