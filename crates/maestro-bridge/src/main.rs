use std::path::PathBuf;

use clap::Parser;
use concerto_maestro_bridge::relay;

#[derive(Parser, Debug)]
#[command(name = "concerto-maestro-bridge", version, about = "Stdio↔UDS relay for the Concerto Maestro MCP server.")]
struct Cli {
    /// The Maestro-MCP unix socket the Core listens on.
    #[arg(long)]
    socket: PathBuf,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    relay(&cli.socket, tokio::io::stdin(), tokio::io::stdout()).await
}
