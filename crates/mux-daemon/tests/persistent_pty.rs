use std::process::{Child, Command, Stdio};
use std::time::Duration;

use mux_client::Client;
use mux_protocol::{CreateSession, ServerEvent, SessionSelector, SpawnCommand};
#[cfg(all(feature = "ghostty", unix))]
use mux_terminal::TerminalEngine as _;
use mux_terminal::TerminalSize;
#[cfg(all(feature = "ghostty", unix))]
use mux_terminal_ghostty::GhosttyEngine;
use mux_workspace::PaneId;
#[cfg(feature = "ghostty")]
use mux_workspace::WorkspaceCommand;
use tempfile::TempDir;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_ptys_survive_client_disconnect_and_reattach() {
    let state_dir = TempDir::new().expect("temporary state directory");
    let socket_path = state_dir.path().join("daemon.sock");
    let daemon = Command::new(env!("CARGO_BIN_EXE_muxd"))
        .arg("--state-dir")
        .arg(state_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");
    let _daemon = ChildGuard(daemon);

    let mut first_client = connect_with_retry(&socket_path).await;
    let summary = first_client
        .create_session(CreateSession {
            name: "persistent-test".to_owned(),
            cwd: std::env::current_dir().expect("current directory"),
            command: SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".to_owned(),
                    "while IFS= read -r line; do printf 'reply:%s\\n' \"$line\"; done".to_owned(),
                ],
                environment: Vec::new(),
            },
            initial_panes: 2,
            initial_size: TerminalSize::default(),
        })
        .await
        .expect("create session");

    let first_attachment = first_client
        .attach(SessionSelector::Id(summary.id))
        .await
        .expect("first attach");
    assert_eq!(first_attachment.panes.len(), 2);
    let first_pane = first_attachment.panes[0].pane_id;
    let second_pane = first_attachment.panes[1].pane_id;

    first_client
        .write_input(first_pane, b"before-disconnect\n".to_vec())
        .await
        .expect("write before disconnect");
    await_output(&mut first_client, first_pane, b"reply:before-disconnect").await;

    // This is the product invariant under test: dropping every client does
    // not own or terminate either PTY process.
    drop(first_client);

    let mut second_client = Client::connect(&socket_path, "integration-test-reattach")
        .await
        .expect("reconnect");
    let second_attachment = second_client
        .attach(SessionSelector::Id(summary.id))
        .await
        .expect("reattach");
    assert!(attachment_carries_history(
        &second_attachment,
        first_pane,
        b"reply:before-disconnect"
    ));
    assert!(attachment_is_independent(
        &second_attachment,
        first_pane,
        second_pane,
        b"reply:before-disconnect"
    ));

    second_client
        .write_input(first_pane, b"after-disconnect\n".to_vec())
        .await
        .expect("write after reconnect");
    await_output(&mut second_client, first_pane, b"reply:after-disconnect").await;

    second_client
        .write_input(second_pane, b"second-pane\n".to_vec())
        .await
        .expect("write second pane");
    await_output(&mut second_client, second_pane, b"reply:second-pane").await;
}

#[cfg(all(feature = "ghostty", unix))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_reentry_clears_a_childs_orphaned_kitty_keyboard_mode() {
    let state_dir = TempDir::new().expect("temporary state directory");
    let socket_path = state_dir.path().join("daemon.sock");
    let daemon = Command::new(env!("CARGO_BIN_EXE_muxd"))
        .arg("--state-dir")
        .arg(state_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");
    let _daemon = ChildGuard(daemon);

    let mut client = connect_with_retry(&socket_path).await;
    let summary = client
        .create_session(CreateSession {
            name: "keyboard-recovery".to_owned(),
            cwd: std::env::current_dir().expect("current directory"),
            command: SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-i".to_owned()],
                environment: vec![("PS1".to_owned(), "MUX> ".to_owned())],
            },
            initial_panes: 1,
            initial_size: TerminalSize::default(),
        })
        .await
        .expect("create session");
    let attachment = client
        .attach(SessionSelector::Id(summary.id))
        .await
        .expect("attach");
    let pane_id = attachment.panes[0].pane_id;

    // The child opts into release reporting, stays foreground long enough for
    // the daemon to observe it, and then dies without popping the mode.
    client
        .write_input(
            pane_id,
            b"sh -c 'printf \"\\033[>3u\"; sleep 0.2; kill -9 $$'\n".to_vec(),
        )
        .await
        .expect("start crashing TUI stand-in");
    await_output(&mut client, pane_id, b"\x1b[=0u").await;

    let attachment = client
        .attach(SessionSelector::Id(summary.id))
        .await
        .expect("reattach after recovery");
    let checkpoint = attachment.panes[0]
        .terminal
        .checkpoint
        .as_ref()
        .expect("Ghostty checkpoint");
    let restored = GhosttyEngine::restore(checkpoint).expect("restore recovered terminal");
    assert_eq!(
        restored
            .kitty_keyboard_flags()
            .expect("read recovered keyboard flags"),
        0
    );
}

#[cfg(feature = "ghostty")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sustained_terminal_output_is_not_backpressured() {
    let state_dir = TempDir::new().expect("temporary state directory");
    let socket_path = state_dir.path().join("daemon.sock");
    let daemon = Command::new(env!("CARGO_BIN_EXE_muxd"))
        .arg("--state-dir")
        .arg(state_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");
    let _daemon = ChildGuard(daemon);

    let mut client = connect_with_retry(&socket_path).await;
    let summary = client
        .create_session(CreateSession {
            name: "throughput-test".to_owned(),
            cwd: std::env::current_dir().expect("current directory"),
            command: SpawnCommand {
                program: "/bin/sh".into(),
                args: Vec::new(),
                environment: Vec::new(),
            },
            initial_panes: 1,
            initial_size: TerminalSize {
                cols: 100,
                rows: 40,
                cell_width_px: 8,
                cell_height_px: 20,
            },
        })
        .await
        .expect("create session");
    let attachment = client
        .attach(SessionSelector::Id(summary.id))
        .await
        .expect("attach");
    let pane_id = attachment.panes[0].pane_id;

    let started = std::time::Instant::now();
    client
        .write_input(
            pane_id,
            b"seq 1 20000; printf '\\036__MUX_%s__\\037\\n' DONE\n".to_vec(),
        )
        .await
        .expect("start output");
    let mut output = Vec::new();
    let mut event_count = 0_usize;
    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.next_event().await.expect("output event") {
                ServerEvent::PaneOutput {
                    pane_id: event_pane,
                    bytes,
                    ..
                } if event_pane == pane_id => {
                    event_count += 1;
                    output.extend_from_slice(&bytes);
                    if output
                        .windows(b"__MUX_DONE__".len())
                        .any(|window| window == b"__MUX_DONE__")
                    {
                        break;
                    }
                }
                ServerEvent::ResyncRequired { .. } => panic!(
                    "stream desynchronized after {event_count} events and {} bytes",
                    output.len()
                ),
                _ => {}
            }
        }
    })
    .await;
    let elapsed = started.elapsed();
    assert!(
        completed.is_ok(),
        "sustained output exceeded five seconds after {event_count} events and {} bytes",
        output.len()
    );
    assert!(
        output.len() > 100_000,
        "received only {} bytes",
        output.len()
    );
    assert!(
        event_count < 256,
        "sustained output was fragmented across {event_count} events"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "{} bytes in {event_count} events took {elapsed:?}",
        output.len()
    );
}

#[cfg(feature = "ghostty")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_churn_during_output_preserves_terminal_sequence() {
    let state_dir = TempDir::new().expect("temporary state directory");
    let socket_path = state_dir.path().join("daemon.sock");
    let daemon = Command::new(env!("CARGO_BIN_EXE_muxd"))
        .arg("--state-dir")
        .arg(state_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");
    let _daemon = ChildGuard(daemon);

    let mut client = connect_with_retry(&socket_path).await;
    let summary = client
        .create_session(CreateSession {
            name: "workspace-output-race".to_owned(),
            cwd: std::env::current_dir().expect("current directory"),
            command: SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".to_owned(),
                    "i=0; while [ \"$i\" -lt 10000 ]; do printf 'line:%s\\n' \"$i\"; i=$((i + 1)); if [ $((i % 50)) -eq 0 ]; then sleep 0.005; fi; done; sleep 2"
                        .to_owned(),
                ],
                environment: Vec::new(),
            },
            initial_panes: 1,
            initial_size: TerminalSize::default(),
        })
        .await
        .expect("create session");
    client
        .attach(SessionSelector::Id(summary.id))
        .await
        .expect("attach");

    client
        .workspace_command(summary.id, WorkspaceCommand::NewTab)
        .await
        .expect("create second busy tab");
    await_workspace_change_without_resync(&mut client, summary.id).await;

    for command in (0..80).map(|index| {
        if index % 2 == 0 {
            WorkspaceCommand::PreviousTab
        } else {
            WorkspaceCommand::NextTab
        }
    }) {
        let attachment = client
            .workspace_command(summary.id, command)
            .await
            .expect("switch tab while output is active");
        assert_eq!(attachment.session.tabs.len(), 2);
        assert!(
            attachment
                .panes
                .iter()
                .all(|pane| pane.terminal.validate_sequence_contract().is_ok()),
            "workspace snapshot contained an inconsistent terminal cursor"
        );
        await_workspace_change_without_resync(&mut client, summary.id).await;

        if let Ok(event) = tokio::time::timeout(Duration::from_millis(5), client.next_event()).await
        {
            assert_not_resync(&event.expect("event after workspace change"));
        }
    }
}

#[cfg(feature = "ghostty")]
async fn await_workspace_change_without_resync(
    client: &mut Client,
    session_id: mux_workspace::SessionId,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = client.next_event().await.expect("workspace event");
            let workspace_changed = matches!(
                &event,
                ServerEvent::WorkspaceChanged {
                    session_id: event_session,
                } if *event_session == session_id
            );
            assert_not_resync(&event);
            if workspace_changed {
                break;
            }
        }
    })
    .await
    .expect("timed out waiting for workspace change");
}

#[cfg(feature = "ghostty")]
fn assert_not_resync(event: &ServerEvent) {
    if let ServerEvent::ResyncRequired { session_id } = event {
        panic!("terminal output desynchronized for session {session_id}");
    }
}

async fn connect_with_retry(socket_path: &std::path::Path) -> Client {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match Client::connect(socket_path, "integration-test").await {
            Ok(client) => return client,
            Err(error) if tokio::time::Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("daemon did not become ready: {error}"),
        }
    }
}

async fn await_output(client: &mut Client, pane_id: PaneId, needle: &[u8]) {
    let future = async {
        let mut output = Vec::new();
        loop {
            match client.next_event().await.expect("next daemon event") {
                ServerEvent::PaneOutput {
                    pane_id: event_pane,
                    bytes,
                    ..
                } if event_pane == pane_id => {
                    output.extend_from_slice(&bytes);
                    if output.windows(needle.len()).any(|window| window == needle) {
                        return;
                    }
                }
                ServerEvent::PaneExited {
                    pane_id: event_pane,
                    status,
                    ..
                } if event_pane == pane_id => {
                    panic!("pane exited before expected output: {status:?}");
                }
                ServerEvent::ResyncRequired { .. } => panic!("test client fell behind"),
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("timed out waiting for pane output");
}

fn replay_contains(
    attachment: &mux_protocol::SessionAttachment,
    pane_id: PaneId,
    needle: &[u8],
) -> bool {
    let bytes: Vec<_> = attachment
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .expect("pane attachment")
        .terminal
        .replay
        .iter()
        .flat_map(|chunk| chunk.bytes.iter().copied())
        .collect();
    bytes.windows(needle.len()).any(|window| window == needle)
}

fn attachment_carries_history(
    attachment: &mux_protocol::SessionAttachment,
    pane_id: PaneId,
    needle: &[u8],
) -> bool {
    let terminal = &attachment
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .expect("pane attachment")
        .terminal;
    terminal.checkpoint.as_ref().map_or_else(
        || replay_contains(attachment, pane_id, needle),
        |checkpoint| checkpoint.next_sequence > 1 && !checkpoint.payload.is_empty(),
    )
}

fn attachment_is_independent(
    attachment: &mux_protocol::SessionAttachment,
    first_pane: PaneId,
    second_pane: PaneId,
    needle: &[u8],
) -> bool {
    let first = &attachment
        .panes
        .iter()
        .find(|pane| pane.pane_id == first_pane)
        .expect("first pane")
        .terminal;
    let second = &attachment
        .panes
        .iter()
        .find(|pane| pane.pane_id == second_pane)
        .expect("second pane")
        .terminal;

    match (&first.checkpoint, &second.checkpoint) {
        (Some(first), Some(second)) => first.payload != second.payload,
        (None, None) => !replay_contains(attachment, second_pane, needle),
        _ => false,
    }
}
