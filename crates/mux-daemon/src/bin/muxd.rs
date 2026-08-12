use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use directories::ProjectDirs;
use mux_daemon::{DaemonConfig, DaemonServer};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "muxd", about = "Persistent mux workspace daemon")]
struct Arguments {
    /// Directory for the local socket and daemon metadata.
    #[arg(long)]
    state_dir: Option<PathBuf>,

    /// Maximum raw replay retained per pane between terminal checkpoints.
    #[arg(long, default_value_t = 4 * 1024 * 1024)]
    replay_bytes_per_pane: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("mux_daemon=info")),
        )
        .with_target(false)
        .init();

    let arguments = Arguments::parse();
    let state_dir = arguments
        .state_dir
        .or_else(default_state_dir)
        .context("could not determine a per-user state directory")?;
    let mut config = DaemonConfig::in_state_dir(state_dir);
    config.replay_bytes_per_pane = arguments.replay_bytes_per_pane;
    DaemonServer::new(config).run().await?;
    Ok(())
}

fn default_state_dir() -> Option<PathBuf> {
    ProjectDirs::from("io", "mux", "Mux").map(|dirs| dirs.data_local_dir().to_path_buf())
}
