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
    host_command_for(program, in_flatpak())
}

/// [`host_command`] with the sandbox check passed in, so both shapes are testable.
pub(crate) fn host_command_for<P: AsRef<OsStr>>(program: P, flatpak: bool) -> Command {
    if flatpak {
        let mut cmd = Command::new("flatpak-spawn");
        cmd.arg("--host").arg(program);
        cmd
    } else {
        Command::new(program)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_instance_host_command_shapes() {
        let c = host_command_for("/a/run.sh", false);
        assert_eq!(c.get_program(), "/a/run.sh");
        assert_eq!(c.get_args().count(), 0);
        let c = host_command_for("/a/run.sh", true);
        assert_eq!(c.get_program(), "flatpak-spawn");
        let args: Vec<&OsStr> = c.get_args().collect();
        assert_eq!(args, ["--host", "/a/run.sh"]);
    }
}
