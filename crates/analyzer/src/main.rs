//! zust-analyzer: the LSP server (stdio). Editors launch `zust-analyzer`

fn main() {
    std::process::exit(match zust_analyzer::run(std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("zust-analyzer: {e}");
            1
        }
    });
}
