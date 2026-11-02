//! Thin runner around the host's `portmux-port configure-app` CLI.
//!
//! portmux owns per-app proxy setup: it picks the port and writes it into
//! the app's own config. This module is the *only* place in
//! `appimage-control-core` that knows about that CLI. Seeding calls
//! [`configure_app`] with the recipe id, the instance, the config path and
//! the desktop id, and holds no port, proxy host or isolation class.
//!
//! Flatpak: we shell out via [`crate::host_cmd::host_command`], which
//! transparently wraps `flatpak-spawn --host` inside a sandbox.
//!
//! Test mode: set `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/path/to/stub`
//! to point the runner at a script that returns canned JSON. The
//! same env var is the user-facing override for hosts that install
//! portmux outside the default `PATH`.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::host_cmd::host_command;

/// Env var that overrides the binary name. Defaults to `portmux-port`
/// which `host_command` resolves through the host shell's `PATH`.
pub const ENV_PORTMUX_PORT_BIN: &str = "APPIMAGE_CONTROL_PORTMUX_PORT_BIN";

/// User-facing summary of the runtime dependency. Surfaced when
/// [`probe_runtime`] fails so installers / GUIs can suggest the right
/// command without each call site having to spell it out. Worded to
/// keep the underlying helper opaque to end users — only an install
/// recipe + an optional override env var for power users.
pub const INSTALL_HINT: &str = "\
A required system helper for seeding is not installed, or is too old. Install \
or upgrade the support package with:\n\
  sudo apt install portmux\n\
Power users can point at a non-default location by setting \
APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/path/to/binary.";

/// One-shot probe of the host's `portmux-port` binary. Result is
/// cached for the lifetime of the process so subsequent calls (one
/// per seed) don't re-spawn the helper just to verify it's still
/// installed.
///
/// Callers should invoke this **before** the first [`configure_app`]
/// call in a seed run so the user gets a clear "install portmux"
/// message instead of a per-instance failure deep in the seeding loop.
pub fn probe_runtime() -> std::result::Result<ProbeOutcome, String> {
    static CACHE: std::sync::OnceLock<std::result::Result<ProbeOutcome, String>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(probe_runtime_uncached).clone()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    /// Resolved binary path or invocation prefix (e.g.
    /// `"portmux-port"`, or `"flatpak-spawn --host portmux-port"`
    /// when running inside a sandbox). Surfaced for diagnostics
    /// only — callers should still drive subsequent calls through
    /// [`configure_app`].
    pub invocation: String,
}

fn probe_runtime_uncached() -> std::result::Result<ProbeOutcome, String> {
    let bin =
        std::env::var_os(ENV_PORTMUX_PORT_BIN).unwrap_or_else(|| OsString::from("portmux-port"));
    // Asks for the subcommand, not the top-level help: a portmux older than
    // `configure-app` still answers plain `--help`.
    let status = host_command(&bin)
        .arg("configure-app")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(ProbeOutcome {
            invocation: bin.to_string_lossy().into_owned(),
        }),
        Ok(s) => Err(format!(
            "Seeding helper exited {} on the host.\n{INSTALL_HINT}",
            s.code().unwrap_or(-1)
        )),
        Err(e) => Err(format!(
            "Could not start seeding helper: {e}.\n{INSTALL_HINT}"
        )),
    }
}

/// What a successful [`configure_app`] leaves for the user to see. Holds no
/// port or host: those are portmux's, and it already wrote them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfiguredApp {
    /// A step the user must take in the app, for an app portmux cannot
    /// write (Telegram). `None` when the setup is complete.
    pub manual_action: Option<String>,
    /// portmux's stderr on success, such as a tor reload warning.
    pub warnings: String,
}

#[derive(Deserialize)]
struct ConfigureOutput {
    #[serde(default)]
    manual_action: Option<String>,
}

/// Ask portmux to allocate `app`'s listener for `instance` and write it into
/// `config`, stamping `desktop_app_id` on the assignment. Idempotent: the same
/// app and instance keep their port.
///
/// `app` is a portmux recipe id. A failure leaves `config` as it was, except
/// that portmux may keep the allocation so a retry finds it.
pub fn configure_app(
    app: &str,
    instance: u32,
    config: &Path,
    desktop_app_id: &str,
) -> Result<ConfiguredApp> {
    let bin =
        std::env::var_os(ENV_PORTMUX_PORT_BIN).unwrap_or_else(|| OsString::from("portmux-port"));
    let output = host_command(&bin)
        .arg("configure-app")
        .arg("--app")
        .arg(app)
        .arg("--instance")
        .arg(instance.to_string())
        .arg("--config")
        .arg(config)
        .arg("--desktop-app-id")
        .arg(desktop_app_id)
        .arg("--json-v2")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::InvalidPath(format!(
                    "Seeding helper not found on host PATH. Install the \
                     support package (sudo apt install portmux) or set \
                     {ENV_PORTMUX_PORT_BIN}=/path/to/binary"
                ))
            } else {
                Error::Io(e)
            }
        })?;

    // portmux prints the reason on stderr with or without --json-v2, so a
    // real error is shown as portmux worded it.
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        return Err(Error::InvalidPath(match code {
            2 => format!("Seeding helper: no free port for {app}#{instance} — {stderr}"),
            _ => format!("Seeding helper exited {code}: {stderr}"),
        }));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: ConfigureOutput = serde_json::from_str(stdout.trim()).map_err(Error::Json)?;
    Ok(ConfiguredApp {
        manual_action: parsed.manual_action,
        warnings: stderr,
    })
}

/// Test-only mutex gate around the
/// `APPIMAGE_CONTROL_PORTMUX_PORT_BIN` env var so the runner tests
/// in this module and the `port_verb` tests in `seed_pack` don't
/// race. Cargo runs both modules' tests in parallel within one
/// process; serialise here.
#[cfg(test)]
pub(crate) fn env_gate() -> &'static std::sync::Mutex<()> {
    static GATE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod portmux_port_runner_tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn write_stub(dir: &std::path::Path, body: &str) -> PathBuf {
        let p = dir.join("portmux-port");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&p, perms).unwrap();
        p
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "aic-mp-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn with_bin<R>(stub: &std::path::Path, f: impl FnOnce() -> R) -> R {
        let guard = super::env_gate().lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(ENV_PORTMUX_PORT_BIN);
        // SAFETY: `env_gate` serialises this process-wide env var
        // across parallel test threads; we restore prev on exit.
        unsafe { std::env::set_var(ENV_PORTMUX_PORT_BIN, stub) };
        let out = f();
        unsafe {
            match prev {
                Some(p) => std::env::set_var(ENV_PORTMUX_PORT_BIN, p),
                None => std::env::remove_var(ENV_PORTMUX_PORT_BIN),
            }
        }
        drop(guard);
        out
    }

    /// A stub that logs its argv, one argument per line, then runs `body`.
    fn logging_stub(dir: &std::path::Path, body: &str) -> (PathBuf, PathBuf) {
        let log = dir.join("argv.log");
        let stub = write_stub(
            dir,
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\" >> '{}'; done\n{body}\n",
                log.display()
            ),
        );
        (stub, log)
    }

    #[test]
    fn argv_is_exactly_the_configure_app_contract() {
        let dir = tmpdir("argv");
        let (stub, log) = logging_stub(
            &dir,
            r#"echo '{"version":2,"host":"h","port":1,"manual_action":null}'"#,
        );
        let cfg = dir.join("settings.json");
        let done = with_bin(&stub, || {
            configure_app("feather", 4, &cfg, "appimage-control-feather-4").unwrap()
        });
        let argv = std::fs::read_to_string(&log).unwrap();
        let want = format!(
            "configure-app\n--app\nfeather\n--instance\n4\n--config\n{}\n--desktop-app-id\nappimage-control-feather-4\n--json-v2\n",
            cfg.display()
        );
        assert_eq!(argv, want);
        assert_eq!(done, ConfiguredApp::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn success_carries_manual_action_and_warnings() {
        let dir = tmpdir("manual");
        let stub = write_stub(
            &dir,
            r#"#!/bin/sh
echo 'tor is not listening yet' 1>&2
echo '{"version":2,"manual_action":"set the port in Telegram"}'
"#,
        );
        let done = with_bin(&stub, || {
            configure_app("telegram", 1, &dir.join("c"), "d").unwrap()
        });
        assert_eq!(
            done.manual_action.as_deref(),
            Some("set the port in Telegram")
        );
        assert_eq!(done.warnings, "tor is not listening yet");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_two_maps_to_the_no_free_port_message() {
        let dir = tmpdir("exh");
        let stub = write_stub(
            &dir,
            "#!/bin/sh\necho 'portmux-port: pool exhausted 9160-9189' 1>&2\nexit 2\n",
        );
        let err = with_bin(&stub, || {
            configure_app("feather", 999, &dir.join("c"), "d").unwrap_err()
        });
        let msg = format!("{err}");
        assert!(msg.contains("no free port for feather#999"), "got: {msg}");
        assert!(msg.contains("pool exhausted 9160-9189"), "got: {msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// BL-007: the old `--json` fallback retried on any failure, so the
    /// retry's "unknown flag" hid the real error.
    #[test]
    fn exit_one_stderr_is_surfaced_verbatim_with_no_retry() {
        let dir = tmpdir("err");
        let why = "cannot change tor's config: if you were just added to the portmux group, log out and in";
        let (stub, log) = logging_stub(
            &dir,
            &format!(
                r#"echo '{{"version":2,"error":{{"exit_code":1,"message":"x"}}}}'
echo "{why}" 1>&2
exit 1"#
            ),
        );
        let err = with_bin(&stub, || {
            configure_app("feather", 1, &dir.join("c"), "d").unwrap_err()
        });
        let msg = format!("{err}");
        assert!(msg.contains(why), "got: {msg}");
        assert!(msg.contains("exited 1"), "got: {msg}");
        let argv = std::fs::read_to_string(&log).unwrap();
        assert_eq!(argv.matches("configure-app").count(), 1, "retried: {argv}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_binary_yields_install_hint() {
        let dir = tmpdir("miss");
        let phantom = dir.join("does-not-exist");
        let err = with_bin(&phantom, || {
            configure_app("feather", 1, &dir.join("c"), "d").unwrap_err()
        });
        let msg = format!("{err}");
        assert!(
            msg.contains("Seeding helper not found") || msg.contains("IO error"),
            "got: {msg}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_runtime_uncached_returns_ok_when_stub_knows_configure_app() {
        let dir = tmpdir("probe-ok");
        let stub = write_stub(
            &dir,
            "#!/bin/sh\n[ \"$1 $2\" = 'configure-app --help' ] && exit 0\nexit 64\n",
        );
        with_bin(&stub, || {
            let r = super::probe_runtime_uncached();
            assert!(r.is_ok(), "expected Ok, got {r:?}");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A portmux older than `configure-app`: top-level `--help` still works.
    #[test]
    fn stub_with_no_configure_app_yields_the_upgrade_hint() {
        let dir = tmpdir("probe-old");
        let stub = write_stub(
            &dir,
            "#!/bin/sh\n[ \"$1\" = '--help' ] && exit 0\necho \"unknown subcommand\" 1>&2\nexit 1\n",
        );
        with_bin(&stub, || {
            let err = super::probe_runtime_uncached().unwrap_err();
            assert!(err.contains("too old"), "got: {err}");
            assert!(err.contains("sudo apt install portmux"), "got: {err}");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_runtime_uncached_includes_install_hint_on_failure() {
        let dir = tmpdir("probe-bad");
        let phantom = dir.join("does-not-exist");
        with_bin(&phantom, || {
            let r = super::probe_runtime_uncached();
            let err = r.unwrap_err();
            assert!(
                err.contains("Seeding helper") || err.contains("seeding helper"),
                "missing helper mention in error: {err}"
            );
            assert!(
                err.contains("sudo apt install portmux")
                    || err.contains("APPIMAGE_CONTROL_PORTMUX_PORT_BIN"),
                "missing install hint in error: {err}"
            );
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_hint_mentions_both_install_path_and_env_override() {
        assert!(super::INSTALL_HINT.contains("sudo apt install portmux"));
        assert!(super::INSTALL_HINT.contains("APPIMAGE_CONTROL_PORTMUX_PORT_BIN"));
    }
}
