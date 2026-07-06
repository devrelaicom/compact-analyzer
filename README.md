# compact-analyzer

A [rust-analyzer](https://rust-analyzer.github.io/)-style language server for
the [Compact](https://docs.midnight.network/compact) smart contract language
(Midnight Network).

Built on [compactp](https://github.com/devrelaicom/compactp), a lossless,
error-tolerant parser frontend for Compact.

## Status: M1 — real-time syntax diagnostics

The server currently provides:

- Live syntax error diagnostics as you type (no compiler round-trip)
- Correct UTF-16 positions, resilient to any input (the server never dies
  on malformed code)

Name resolution, navigation, completion, compiler-integrated diagnostics,
and a VS Code extension are on the roadmap — see
`docs/superpowers/specs/2026-07-06-compact-analyzer-design.md`.

## Build

Requires Rust 1.90+.

```sh
cargo build --release
./target/release/compact-analyzer --version
```

The server speaks LSP over stdio.

## Try it in Neovim (0.10+)

```lua
vim.filetype.add({ extension = { compact = "compact" } })
vim.api.nvim_create_autocmd("FileType", {
  pattern = "compact",
  callback = function()
    vim.lsp.start({
      name = "compact-analyzer",
      cmd = { "/path/to/compact-analyzer" },
    })
  end,
})
```

Open a `.compact` file and introduce a syntax error — diagnostics appear as
you type.

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

## License

MIT
