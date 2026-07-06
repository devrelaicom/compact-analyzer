mod lsp_utils;

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--version") {
        println!("compact-analyzer {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    eprintln!(
        "compact-analyzer {}: LSP server not implemented yet",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
