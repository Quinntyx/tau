//! tau — command-line entrypoint.
//!
//! `tau serve` runs the long-running daemon. `tau ping` / `tau health` are
//! debug helpers that connect via `tau-client`. The TUI/auth/config subcommands
//! remain staged for later milestones; `tau gui` launches the local model-turn
//! browser client.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tau", version, about = "tau agent daemon + clients")]
struct Cli {
    /// Path to the daemon Unix socket (overrides default resolution).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the long-running daemon.
    Serve,
    /// Connect and round-trip a ping.
    Ping,
    /// Connect and fetch daemon health info.
    Health,
    /// Manage API credentials (env / keyring / file fallback).
    Auth {
        #[command(subcommand)]
        cmd: AuthCmd,
    },
    /// Launch the TUI (not yet implemented).
    Tui,
    /// Launch the local model-turn GUI.
    Gui,
    /// Edit configuration (not yet implemented).
    Config,
    /// Resume a previous session (not yet implemented).
    Resume,
}

#[derive(Subcommand)]
enum AuthCmd {
    /// Store an API key for a provider (keyring, file fallback).
    Set { provider: String, key: String },
    /// Show whether a provider has a credential (never prints the full key).
    Get { provider: String },
    /// Delete a provider's stored credential.
    Delete { provider: String },
    /// List providers that currently resolve to a credential.
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tau=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let socket = match cli.socket.clone() {
        Some(s) => s,
        None => tau_core::default_socket_path()?,
    };

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => tau_server::run(socket).await,
        Command::Ping => {
            let mut c = tau_client::Client::connect(&socket)
                .await
                .context("connecting to daemon (is it running? try `tau serve`)")?;
            println!("{}", c.ping().await?);
            Ok(())
        }
        Command::Health => {
            let mut c = tau_client::Client::connect(&socket)
                .await
                .context("connecting to daemon (is it running? try `tau serve`)")?;
            let h = c.health().await?;
            println!(
                "version={} uptime_ms={} pid={}",
                h.version, h.uptime_ms, h.pid
            );
            Ok(())
        }
        Command::Auth { cmd } => {
            let store = tau_core::credentials::CredentialStore::new()?;
            match cmd {
                AuthCmd::Set { provider, key } => {
                    store.set(&provider, &key)?;
                    println!("stored credential for `{provider}`");
                }
                AuthCmd::Get { provider } => {
                    let present = store.get(&provider, None).is_some();
                    println!("{provider}: {}", if present { "set" } else { "not set" });
                }
                AuthCmd::Delete { provider } => {
                    store.delete(&provider)?;
                    println!("deleted credential for `{provider}`");
                }
                AuthCmd::List => {
                    for p in store.list() {
                        println!("{p}");
                    }
                }
            }
            Ok(())
        }
        Command::Tui => bail!("`tau tui` is not implemented yet"),
        Command::Gui => tau_gui::run(socket),
        Command::Config => bail!("`tau config` is not implemented yet"),
        Command::Resume => bail!("`tau resume` is not implemented yet"),
    }
}
