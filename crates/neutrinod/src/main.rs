use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Neutrino's daemon. On a workstation it reads and changes the working
/// copies you register; with --server it also hosts repositories and answers
/// paired devices.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Where to listen. Default `$NEUTRINO_SOCKET`, else ~/.neutrino/neutrinod.sock.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Where to keep state. Default `$NEUTRINO_HOME`, else ~/.neutrino.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Run as the server: host repositories and accept paired devices.
    #[arg(long)]
    server: bool,
    /// Server: the TLS port for paired devices.
    #[arg(long, default_value_t = neutrino_proto::REMOTE_PORT)]
    port: u16,
    /// Server: don't open the network endpoint.
    #[arg(long)]
    no_remote: bool,
    /// Server: don't advertise over Bonjour.
    #[arg(long)]
    no_bonjour: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run by git inside a push to a hosted repository (installed as its hooks).
    Hook {
        /// `pre-receive` or `post-receive`.
        kind: String,
        #[arg(long)]
        repo: String,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    let socket = args.socket.clone().unwrap_or_else(neutrinod::socket_path);
    if let Some(Cmd::Hook { kind, repo }) = &args.command {
        std::process::exit(neutrinod::hosting::run_hook(kind, repo, &socket));
    }
    tokio::runtime::Runtime::new()?.block_on(run(args, socket))
}

async fn run(args: Args, socket: PathBuf) -> Result<()> {
    let state = args.state_dir.unwrap_or_else(neutrinod::state_dir);
    let options = neutrinod::Options {
        role: if args.server { neutrinod::Role::Server } else { neutrinod::Role::Workstation },
        remote_port: if args.server && !args.no_remote { Some(args.port) } else { None },
        advertise: !args.no_bonjour,
    };
    let daemon = neutrinod::Daemon::with(&state, options)?;
    eprintln!(
        "neutrinod {} ({}) listening on {}",
        env!("CARGO_PKG_VERSION"),
        if args.server { "server" } else { "workstation" },
        socket.display()
    );
    let shutdown = async {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    };
    neutrinod::serve(daemon, &socket, shutdown).await
}
