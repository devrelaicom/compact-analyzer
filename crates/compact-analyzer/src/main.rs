mod lsp_utils;
mod server;

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--version") {
        println!("compact-analyzer {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    eprintln!(
        "compact-analyzer {}: starting LSP server on stdio",
        env!("CARGO_PKG_VERSION")
    );
    server::run()
}
