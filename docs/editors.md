# Editor setup

compact-analyzer speaks the Language Server Protocol over stdio, so any
LSP-capable editor can use it. The [VS Code extension](../editors/vscode/) is
the turn-key path; this guide covers registering the server manually in **Zed**,
**Neovim**, and **Helix**.

All three need the `compact-analyzer` binary on disk — install it first (see the
[Install](../README.md#language-server-binary) section of the root README), and
either put it on your `PATH` or use the absolute path in the snippets below.

Settings map onto the server's LSP `initializationOptions`. The keys are the
same ones documented in the root
[Configuration](../README.md#configuration) table:
`importSearchPath`, `toolchainPath`, `compileOnSave`, and `formatting`.
(`serverPath` and `trace.server` are VS Code client concerns and have no
server-side equivalent.) Leave `importSearchPath` unset to let the server fall
back to the `COMPACT_PATH` environment variable.

> The server reads its configuration and discovers the toolchain **only at
> startup**. After changing any option — or installing the `compact` toolchain
> mid-session — restart the language server (each editor's restart command is
> noted below).

---

## Neovim

*Config schema verified against Neovim's official LSP documentation
(`runtime/doc/lsp.txt`: `vim.lsp.config` / `vim.lsp.enable`, and the
`cmd` / `filetypes` / `root_markers` / `init_options` fields) on 2026-07-13.
**Not run in a live Neovim** — `nvim` was unavailable in the authoring
environment, so treat this as untested and verify before relying on it.*

Neovim has no built-in `compact` filetype, so first map the extension, then
register the server. The example below uses the modern `vim.lsp.config` /
`vim.lsp.enable` API introduced in **Neovim 0.11**.

```lua
-- 1. Teach Neovim that .compact files are the "compact" filetype.
vim.filetype.add({ extension = { compact = "compact" } })

-- 2. Define the language server (Neovim 0.11+).
vim.lsp.config("compact-analyzer", {
  cmd = { "compact-analyzer" }, -- or an absolute path to the binary
  filetypes = { "compact" },
  root_markers = { ".git" },    -- no canonical Compact project file; .git is a sane default
  -- Sent to the server as initializationOptions:
  init_options = {
    compileOnSave = true,
    formatting = true,
    -- toolchainPath = "/path/to/compact",  -- a file OR a directory; omit to search PATH
    -- importSearchPath = { "/extra/imports" }, -- omit entirely to fall back to COMPACT_PATH
  },
})

-- 3. Enable it — it attaches automatically to every "compact" buffer.
vim.lsp.enable("compact-analyzer")
```

Open a `.compact` file and introduce a syntax error — diagnostics appear as you
type. Restart the server with `:lua vim.lsp.enable("compact-analyzer", false)`
followed by `:lua vim.lsp.enable("compact-analyzer")`, or simply restart Neovim.

<details>
<summary>Neovim 0.10 (legacy <code>vim.lsp.start</code>)</summary>

On 0.10, `vim.lsp.config`/`vim.lsp.enable` do not exist. Start the server from
a `FileType` autocommand instead:

```lua
vim.filetype.add({ extension = { compact = "compact" } })
vim.api.nvim_create_autocmd("FileType", {
  pattern = "compact",
  callback = function()
    vim.lsp.start({
      name = "compact-analyzer",
      cmd = { "compact-analyzer" }, -- or an absolute path
      init_options = { compileOnSave = true, formatting = true },
    })
  end,
})
```

</details>

---

## Helix

*Config schema verified against the current Helix documentation
([docs.helix-editor.com/languages.html](https://docs.helix-editor.com/languages.html))
on 2026-07-13. **Not run in a live Helix install** — untested; verify before
use.*

Add the language server and a `[[language]]` entry to your Helix
`languages.toml` (`~/.config/helix/languages.toml` on macOS/Linux,
`%AppData%\helix\languages.toml` on Windows; or `.helix/languages.toml` for a
single project):

```toml
[language-server.compact-analyzer]
command = "compact-analyzer" # must be on $PATH, or give an absolute path
# initializationOptions go under `config`:
config = { compileOnSave = true, formatting = true }
# config = { toolchainPath = "/path/to/compact", compileOnSave = true, formatting = true }

[[language]]
name = "compact"
scope = "source.compact"
file-types = ["compact"]
language-servers = ["compact-analyzer"]
```

Restart the server with `:lsp-restart` after editing `languages.toml`.

> **Syntax highlighting:** Helix ships no Compact tree-sitter grammar, so it may
> log a "no grammar found" notice and highlighting will be absent. The language
> server features (diagnostics, goto, references, completion, formatting) still
> work. To add highlighting you would supply a `[[grammar]]` for `compact` and
> the matching queries, which is out of scope for this guide.

---

## Zed

*Verified against the Zed language-extension documentation
([zed.dev/docs/extensions/languages](https://zed.dev/docs/extensions/languages))
on 2026-07-13. **Not built or run** — untested; verify before use.*

Unlike Neovim and Helix, **Zed cannot register a brand-new language and its
language server from settings alone** — it requires a compiled **language
extension** (written in Rust, compiled to WebAssembly). No Compact extension is
published for Zed yet, so there is no drop-in configuration to paste today.

If you want to build one, a minimal Compact extension needs:

- `extension.toml` declaring the language server:

  ```toml
  id = "compact"
  name = "Compact"
  version = "0.0.1"
  schema_version = 1

  [language_servers.compact-analyzer]
  name = "Compact Analyzer"
  languages = ["Compact"]
  ```

- `languages/compact/config.toml` describing the language:

  ```toml
  name = "Compact"
  grammar = "compact"
  path_suffixes = ["compact"]
  ```

- a `[[grammar]]` entry plus tree-sitter queries, and Rust code implementing the
  extension's `language_server_command` method to return the path to the
  `compact-analyzer` binary.

Follow Zed's official
[language extension guide](https://zed.dev/docs/extensions/languages) for the
full structure and how to install it as a local dev extension. Once such an
extension exists, per-server `initializationOptions` are supplied under
`lsp."compact-analyzer".initialization_options` in Zed's `settings.json`, and
you restart the server from the command palette.

---

See also the [dogfood checklist](dogfood-checklist.md) for a manual smoke test
of the VS Code extension against a real multi-file Compact project.
