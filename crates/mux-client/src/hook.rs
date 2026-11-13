//! The command the hooks Mux installs in coding agents run, as
//! `"$MUX_CTL" hook <agent> <event>`: it reads what the agent hands a hook
//! on standard input and passes it to the daemon as a report on the agent
//! in this pane.
//!
//! It runs inside the agent's own turn, so it is quick and quiet above all:
//! it prints nothing, since an agent can take a hook's output for
//! instructions; it gives up within a moment if the daemon does not
//! answer; and whatever goes wrong, it says the hook succeeded, so Mux can
//! never be the reason an agent stalls or fails.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mux_fleet::AgentKind;
use mux_workspace::PaneId;
use tokio::io::AsyncReadExt as _;

use crate::Client;

/// The most of a hook's input read: a tool's arguments can be a whole file.
const MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_millis(1_500);
const SEND_TIMEOUT: Duration = Duration::from_millis(1_500);

/// Report what the hook was told, if this is a Mux pane and the event is
/// one Mux follows. Never fails.
pub async fn run(agent: &str, event: Option<&str>) {
    let _ = report(agent, event).await;
}

async fn report(agent: &str, event: Option<&str>) -> Option<()> {
    let at_ms = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis(),
    )
    .ok()?;
    let pane_id: PaneId = std::env::var("MUX_PANE_ID").ok()?.parse().ok()?;
    let socket = PathBuf::from(std::env::var_os("MUX_SOCKET")?);
    let kind = AgentKind::from_command(agent)?;

    let mut input = Vec::new();
    // An agent that hands its hook nothing, or never closes the pipe,
    // still has its event reported.
    let _ = tokio::time::timeout(
        READ_TIMEOUT,
        tokio::io::stdin()
            .take(MAX_INPUT_BYTES)
            .read_to_end(&mut input),
    )
    .await;
    let report = mux_fleet::hooks::parse(kind, event, &String::from_utf8_lossy(&input), at_ms)?;

    tokio::time::timeout(SEND_TIMEOUT, async {
        let mut client = Client::connect(&socket, "mux-hook").await.ok()?;
        client.report_agent(pane_id, report).await.ok()
    })
    .await
    .ok()?
}
