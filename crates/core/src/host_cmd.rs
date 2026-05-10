//! Build `Command` instances that run on the host even when we're inside a Flatpak sandbox.
//!
//! Inside the sandbox, host tools like `xvfb-run`, `fusermount3`,
//! `update-desktop-database`, `gtk-update-icon-cache`, `appimageupdatetool`, and the
//! AppImages themselves aren't reachable from the GNOME runtime. When `/.flatpak-info`
//! exists we wrap each call in `flatpak-spawn --host …` to escape the sandbox.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

static IN_FLATPAK: OnceLock<bool> = OnceLock::new();

pub fn in_flatpak() -> bool {
    *IN_FLATPAK.get_or_init(|| Path::new("/.flatpak-info").exists())
}

/// Build a `Command` for `program` that runs on the host even from inside Flatpak.
///
/// Outside Flatpak this is identical to `Command::new(program)`. Inside Flatpak the
/// returned command is `flatpak-spawn --host <program>`, so subsequent `.arg(...)` /
/// `.args(...)` calls append further arguments to the host-side program.
pub fn host_command<P: AsRef<OsStr>>(program: P) -> Command {
    if in_flatpak() {
        let mut cmd = Command::new("flatpak-spawn");
        cmd.arg("--host").arg(program);
        cmd
    } else {
        Command::new(program)
    }
}
