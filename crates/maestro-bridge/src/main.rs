//! `concerto-maestro-bridge` binary — the stdio↔UDS relay the Maestro CLI spawns.
//! Unix-only: it relays over a `UnixStream` (see `lib.rs`). On a non-unix target
//! the lib is empty, so the bin is a typed "unix-only" stub that exits non-zero.

#[cfg(unix)]
mod unix_main {
    use std::path::PathBuf;

    use clap::Parser;
    use concerto_maestro_bridge::relay;

    #[derive(Parser, Debug)]
    #[command(
        name = "concerto-maestro-bridge",
        version,
        about = "Stdio↔UDS relay for the Concerto Maestro MCP server."
    )]
    struct Cli {
        /// The Maestro-MCP unix socket the Core listens on.
        #[arg(long)]
        socket: PathBuf,
    }

    pub async fn run() -> std::io::Result<()> {
        let cli = Cli::parse();
        relay(&cli.socket, tokio::io::stdin(), tokio::io::stdout()).await
    }
}

#[cfg(unix)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    unix_main::run().await
}

#[cfg(not(unix))]
fn main() {
    eprintln!("concerto-maestro-bridge is unix-only (the Concerto Maestro is #[cfg(unix)])");
    std::process::exit(1);
}
