# Compact Analyzer

Language support for the [Compact](https://docs.midnight.network/compact) smart
contract language (Midnight Network) — the community successor to the sunsetted
official Compact extension.

Compact Analyzer is a thin client for
[`compact-analyzer`](https://github.com/devrelaicom/compact-analyzer), a
[rust-analyzer](https://rust-analyzer.github.io/)-style language server for
Compact. The extension acquires and launches the server for you; you can also
point it at your own binary.

## Features

- **Live syntax diagnostics** as you type — no compiler round-trip.
- **Navigation:** goto-definition, find-references, rename, hover, and document
  symbols.
- **Cross-file awareness:** resolves cross-file `import`s and provides workspace
  symbols.
- **Code completion,** including `.`-triggered members, ledger ADT methods,
  struct fields, enum variants, and import-prefix/alias completion.
- **Full-document semantic tokens.**
- **Folding ranges** and **selection ranges.**
- **Compile-on-save diagnostics** via the `compact` CLI (which sources
  `compactc`).
- **Document formatting** via `compact format`.

The last two features require the Compact toolchain — install it from the
[`midnightntwrk/compact`](https://github.com/midnightntwrk/compact/releases)
releases. Without it, everything that does not need the compiler still works.

## Settings

All settings live under `compact-analyzer.*`.

| Setting | Type | Default | Meaning |
| --- | --- | --- | --- |
| `compact-analyzer.serverPath` | `string` | `""` | Absolute path to a `compact-analyzer` binary; overrides auto-acquisition (settings → `PATH` → download cache → download). Machine-overridable. Client-side only. |
| `compact-analyzer.importSearchPath` | `string[]` | `[]` | Import search directories, sent to the server as `initializationOptions.importSearchPath`. Left empty, the key is omitted and the server falls back to the `COMPACT_PATH` environment variable. |
| `compact-analyzer.toolchainPath` | `string` | `""` | Path to the `compact` CLI — a file _or_ a directory containing it. When set, the server does not search `PATH`; an unresolvable value silently disables the toolchain features and shows a single informational notice. |
| `compact-analyzer.compileOnSave` | `boolean` | `true` | Run `compact compile` on save for compiler diagnostics (needs the toolchain). |
| `compact-analyzer.formatting` | `boolean` | `true` | Provide document formatting via `compact format` (needs the toolchain). |
| `compact-analyzer.trace.server` | `off` \| `messages` \| `verbose` | `off` | LSP trace verbosity. Client-side only. |

### Configuration changes need a restart

The server reads its settings and finds the toolchain **only at startup**.
After changing any `compact-analyzer.*` setting — or installing the `compact`
toolchain mid-session — run **"Compact Analyzer: Restart Server"** from the
command palette. The extension also offers to restart when it detects a setting
change.

### About formatting

Formatting is delegated to `compact format`. If **Format Document** appears to
do nothing, that means either the file is **already formatted** (a no-op is the
expected result) or the toolchain is **disabled or absent** (`formatting` set to
`false`, or no `compact` CLI resolved). Check the **"Compact Analyzer"** output
channel to see whether the toolchain was discovered.

## Troubleshooting

Open the **Output** panel and select **"Compact Analyzer"**. The server logs
startup lines there — including which toolchain was found — for example
`compact-analyzer: compact toolchain <version> ... at <path>`, or
`no compact toolchain found; compile-on-save disabled`. That toolchain line is
the first thing to check when a feature is missing.

## macOS Gatekeeper (unsigned binaries)

The server binaries are **unsigned** — they ship without a Developer ID
signature or notarisation. When the extension downloads the server, or when you
install it via Homebrew or the shell installer, macOS generally does not
interfere (those paths do not set the `com.apple.quarantine` flag). You mainly
hit Gatekeeper if you download a server **archive through a browser** and set it
as `compact-analyzer.serverPath`: macOS quarantines the download, and the flag
can carry over to the `compact-analyzer` binary you extract.

To allow it: right-click the binary in Finder → **Open**, or use **System
Settings → Privacy & Security → "Open Anyway"**, or clear the flag from the
extracted binary directly (adjust the path):

```sh
xattr -d com.apple.quarantine ./compact-analyzer
```

Use the non-recursive `-d`: the download is a single CLI binary inside a
`.tar.gz`, not a `.app` bundle. The recursive form
(`xattr -dr com.apple.quarantine <dir>`) is only for clearing a whole directory
tree or an application bundle.

## Attribution

The TextMate grammar, language configuration, and code snippets are adapted from
the official Compact VS Code editor support in the `LFDT-Minokawa/compact`
monorepo, used under the Apache License 2.0. See
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) for the full attribution,
the statement of modifications, and the bundled licence text.

The extension itself is licensed under the MIT License.
