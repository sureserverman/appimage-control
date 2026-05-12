//! Thin runner around the host's `portmux-port ensure` CLI.
//!
//! On both Ubuntu and Whonix the canonical port-assignment tool is
//! `portmux-port`, shipped by the portmux `.deb`. This module is
//! the *only* place in `appimage-control-core` that knows about that
//! CLI. Higher-level seeding code calls [`ensure_port`] with the
//! `(app, instance, purpose, isolation)` tuple and gets back an
//! [`EnsuredPort`] describing the host and port the seeded app
//! should be pointed at.
//!
//! Flatpak: we shell out via [`crate::host_cmd::host_command`], which
//! transparently wraps `flatpak-spawn --host` inside a sandbox.
//!
//! Test mode: set `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/path/to/stub`
//! to point the runner at a script that returns canned JSON. The
//! same env var is the user-facing override for hosts that install
//! portmux outside the default `PATH`.

use std::ffi::OsString;
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
A required system helper for seeding is not installed. Install the \
support package with:\n\
  sudo apt install portmux\n\
Power users can point at a non-default location by setting \
APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/path/to/binary.";

/// One-shot probe of the host's `portmux-port` binary. Result is
/// cached for the lifetime of the process so subsequent calls (one
/// per seed) don't re-spawn the helper just to verify it's still
/// installed.
///
/// Callers should invoke this **before** the first
/// [`ensure_port`] call in a seed run so the user gets a clear
/// "install portmux" message instead of a per-instance spawn
/// failure deep in the seeding loop.
///
/// Pass the result to the user; do not branch on it for control
/// flow — `ensure_port` itself returns the same error if invoked
/// after a failed probe.
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
    /// [`ensure_port`].
    pub invocation: String,
}

fn probe_runtime_uncached() -> std::result::Result<ProbeOutcome, String> {
    let bin = std::env::var_os(ENV_PORTMUX_PORT_BIN)
        .unwrap_or_else(|| OsString::from("portmux-port"));
    let status = host_command(&bin)
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

/// Tor listener kind, mirrored from `portmux-port`'s `--purpose` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Purpose {
    Socks = 0,
    Http = 1,
    Trans = 2,
    Dns = 3,
}

impl Purpose {
    pub fn as_cli_arg(self) -> &'static str {
        match self {
            Purpose::Socks => "socks",
            Purpose::Http => "http",
            Purpose::Trans => "trans",
            Purpose::Dns => "dns",
        }
    }
}

/// Stream-isolation class, mirrored from `portmux-port`'s
/// `--isolation` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IsolationClass {
    None = 0,
    DestAddr = 1,
    DestPort = 2,
    Both = 3,
    HttpTunnel = 4,
}

impl IsolationClass {
    pub fn as_cli_arg(self) -> &'static str {
        match self {
            IsolationClass::None => "none",
            IsolationClass::DestAddr => "dest-addr",
            IsolationClass::DestPort => "dest-port",
            IsolationClass::Both => "both",
            IsolationClass::HttpTunnel => "http-tunnel",
        }
    }

    #[allow(dead_code)]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "none" => IsolationClass::None,
            "dest-addr" | "destaddr" => IsolationClass::DestAddr,
            "dest-port" | "destport" => IsolationClass::DestPort,
            "both" => IsolationClass::Both,
            "http-tunnel" | "httptunnel" => IsolationClass::HttpTunnel,
            _ => return None,
        })
    }
}

/// Mirror of portmux's `port_ensure::EnsuredPort`. Fields with
/// `#[serde(default)]` keep us resilient to additive changes on the
/// portmux side.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct EnsuredPort {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub isolation: String,
    #[serde(default)]
    pub source_file: String,
    #[serde(default)]
    pub tor_reload_done: bool,
    #[serde(default)]
    pub already_assigned: bool,
}

/// Look up — or, on first call, allocate — the host's port dedicated
/// to `(app_label, instance)`. Idempotent: calling twice with the
/// same arguments returns the same `EnsuredPort` and
/// `already_assigned = true` the second time.
///
/// `host_loopback` is forwarded as portmux-port's `--host-loopback`.
/// `127.0.0.1` is the right value for Ubuntu seed-time use; on Whonix
/// the gateway address is baked into portmux and the loopback hint
/// is ignored.
pub fn ensure_port(
    app_label: &str,
    instance: u32,
    purpose: Purpose,
    isolation: IsolationClass,
    host_loopback: &str,
) -> Result<EnsuredPort> {
    let bin = std::env::var_os(ENV_PORTMUX_PORT_BIN)
        .unwrap_or_else(|| OsString::from("portmux-port"));
    let output = host_command(&bin)
        .arg("ensure")
        .arg("--app")
        .arg(app_label)
        .arg("--instance")
        .arg(instance.to_string())
        .arg("--purpose")
        .arg(purpose.as_cli_arg())
        .arg("--isolation")
        .arg(isolation.as_cli_arg())
        .arg("--host-loopback")
        .arg(host_loopback)
        .arg("--json")
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

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(match code {
            2 => Error::InvalidPath(format!(
                "Seeding helper: no free port for {app_label}#{instance} — {stderr}"
            )),
            _ => Error::InvalidPath(format!(
                "Seeding helper exited {code}: {stderr}"
            )),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let ensured: EnsuredPort = serde_json::from_str(stdout.trim())
        .map_err(Error::Json)?;
    Ok(ensured)
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

    #[test]
    fn happy_path_parses_json_output() {
        let dir = tmpdir("happy");
        let stub = write_stub(
            &dir,
            r#"#!/bin/sh
echo '{"host":"10.152.152.10","port":9168,"isolation":"dest-addr","source_file":"/tmp/wp.conf","tor_reload_done":false,"already_assigned":false}'
exit 0
"#,
        );
        let ep = with_bin(&stub, || {
            ensure_port(
                "feather",
                4,
                Purpose::Socks,
                IsolationClass::DestAddr,
                "127.0.0.1",
            )
            .unwrap()
        });
        assert_eq!(ep.host, "10.152.152.10");
        assert_eq!(ep.port, 9168);
        assert_eq!(ep.isolation, "dest-addr");
        assert!(!ep.tor_reload_done);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_two_surfaces_as_pool_exhausted_error() {
        let dir = tmpdir("exh");
        let stub = write_stub(
            &dir,
            "#!/bin/sh\necho 'portmux-port: pool exhausted 9160-9189' 1>&2\nexit 2\n",
        );
        let err = with_bin(&stub, || {
            ensure_port(
                "feather",
                999,
                Purpose::Socks,
                IsolationClass::DestAddr,
                "127.0.0.1",
            )
            .unwrap_err()
        });
        let msg = format!("{err}");
        assert!(msg.contains("no free port for feather#999"), "got: {msg}");
        // The error prefix our code generates must not lecture about
        // the helper's identity — passthrough of the helper's own
        // stderr is allowed (the stub here is intentionally noisy).
        assert!(msg.starts_with("invalid path: Seeding helper"), "got: {msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_nonzero_surfaces_error_message() {
        let dir = tmpdir("err");
        let stub = write_stub(
            &dir,
            "#!/bin/sh\necho 'boom' 1>&2\nexit 7\n",
        );
        let err = with_bin(&stub, || {
            ensure_port(
                "feather",
                1,
                Purpose::Socks,
                IsolationClass::DestAddr,
                "127.0.0.1",
            )
            .unwrap_err()
        });
        let msg = format!("{err}");
        assert!(msg.contains("exited 7"), "got: {msg}");
        assert!(msg.contains("boom"), "got: {msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_binary_yields_install_hint() {
        // Point at a path that doesn't exist.
        let dir = tmpdir("miss");
        let phantom = dir.join("does-not-exist");
        let err = with_bin(&phantom, || {
            ensure_port(
                "feather",
                1,
                Purpose::Socks,
                IsolationClass::DestAddr,
                "127.0.0.1",
            )
            .unwrap_err()
        });
        let msg = format!("{err}");
        assert!(
            msg.contains("Seeding helper not found") || msg.contains("IO error"),
            "got: {msg}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_runtime_uncached_returns_ok_when_stub_exits_zero() {
        let dir = tmpdir("probe-ok");
        let stub = write_stub(&dir, "#!/bin/sh\nexit 0\n");
        with_bin(&stub, || {
            let r = super::probe_runtime_uncached();
            assert!(r.is_ok(), "expected Ok, got {r:?}");
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

    #[test]
    fn isolation_class_round_trips_through_cli_strings() {
        for c in [
            IsolationClass::None,
            IsolationClass::DestAddr,
            IsolationClass::DestPort,
            IsolationClass::Both,
            IsolationClass::HttpTunnel,
        ] {
            let s = c.as_cli_arg();
            assert_eq!(IsolationClass::parse(s), Some(c));
        }
    }
}
