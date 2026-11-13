//! What every pane's shell is told about the Mux it runs in, beside its
//! own `MUX_PANE_ID`: where the daemon listens, and the `muxctl` to reach
//! it with. The hooks Mux installs in coding agents run
//! `"$MUX_CTL" hook …`, so they follow whichever Mux owns the pane and
//! name no path of their own.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::OnceLock,
};

static ENVIRONMENT: OnceLock<Vec<(OsString, OsString)>> = OnceLock::new();

/// Note where this daemon listens. Panes started before this is called
/// start without it.
pub fn install(socket_path: &Path) {
    let mut environment = vec![(
        OsString::from("MUX_SOCKET"),
        socket_path.as_os_str().to_owned(),
    )];
    if let Some(control) = control_program() {
        environment.push((OsString::from("MUX_CTL"), control.into_os_string()));
    }
    let _ = ENVIRONMENT.set(environment);
}

pub fn environment() -> impl Iterator<Item = (OsString, OsString)> {
    ENVIRONMENT.get().into_iter().flatten().cloned()
}

/// `muxctl` beside the program this daemon is, as it is in `Mux.app`; else
/// the program itself, which answers to `hook` too.
fn control_program() -> Option<PathBuf> {
    let own = std::env::current_exe().ok()?;
    let beside = own.with_file_name("muxctl");
    Some(if beside.is_file() { beside } else { own })
}
