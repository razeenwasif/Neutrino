use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

/// Neutrino's daemon. On this Mac it reads the working copies you register and
/// answers the Neutrino app and `ntr`.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Where to listen. Default `$NEUTRINO_SOCKET`, else ~/.neutrino/neutrinod.sock.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Where to keep state. Default `$NEUTRINO_HOME`, else ~/.neutrino.
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let state = args.state_dir.unwrap_or_else(neutrinod::state_dir);
    let socket = args.socket.unwrap_or_else(neutrinod::socket_path);
    let daemon = neutrinod::Daemon::new(&state)?;
    eprintln!("neutrinod {} listening on {}", env!("CARGO_PKG_VERSION"), socket.display());

    let shutdown = async {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    };
    neutrinod::serve(daemon, &socket, shutdown).await
}
