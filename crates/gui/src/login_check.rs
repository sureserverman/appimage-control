//! The login update check: registers `appimage-control --check-updates --notify` to run at
//! login (OD-2: on by default), and the D-Bus service file that lets a notification click start
//! the app. Natively this writes XDG files; inside Flatpak, which cannot write
//! `~/.config/autostart`, it asks the Background portal instead.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

const APP_ID: &str = crate::APP_ID;
/// What the login entry runs. Inside Flatpak the portal resolves the program in the sandbox.
const COMMANDLINE: [&str; 3] = ["appimage-control", "--check-updates", "--notify"];

/// Native file locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPaths {
    pub autostart: PathBuf,
    pub dbus_service: PathBuf,
}

impl LoginPaths {
    pub fn new(config_home: &Path, data_home: &Path) -> Self {
        LoginPaths {
            autostart: config_home
                .join("autostart")
                .join(format!("{APP_ID}.desktop")),
            dbus_service: data_home
                .join("dbus-1/services")
                .join(format!("{APP_ID}.service")),
        }
    }

    /// From `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` (with the usual fallbacks).
    pub fn from_env() -> Self {
        Self::new(&glib::user_config_dir(), &glib::user_data_dir())
    }
}

/// Characters the desktop-entry spec reserves in Exec (besides space, which is quoted), plus
/// `%` (field codes). Refused rather than escaped, for the D-Bus service parser too.
const EXEC_RESERVED: &[char] = &[
    '"', '\'', '\\', '$', '`', '%', ';', '&', '|', '<', '>', '~', '*', '?', '#', '(', ')',
];

/// `exe` as one Exec token. Absolute paths only; a path with a space is double-quoted. Any
/// other character that would need escaping in a desktop-entry or D-Bus Exec line is refused
/// rather than escaped, so a hostile folder name can never add an argument or a shell expansion.
fn exec_program(exe: &Path) -> io::Result<String> {
    let s = exe
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "program path is not UTF-8"))?;
    if !exe.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("program path is not absolute: {s}"),
        ));
    }
    if let Some(c) = s
        .chars()
        .find(|c| c.is_control() || EXEC_RESERVED.contains(c))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("program path contains {c:?}, which an Exec line cannot carry safely: {s}"),
        ));
    }
    Ok(if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    })
}

/// The autostart entry that runs the check at login.
pub fn autostart_desktop_text(exe: &Path) -> io::Result<String> {
    let program = exec_program(exe)?;
    let exe = exe.display();
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=AppImage Control update check\n\
         Comment=Tells you at login which installed AppImages have newer releases\n\
         TryExec={exe}\n\
         Exec={program} --check-updates --notify\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n\
         X-GNOME-Autostart-Delay=30\n"
    ))
}

/// The D-Bus service file that starts the app for a notification click.
pub fn dbus_service_text(exe: &Path) -> io::Result<String> {
    let program = exec_program(exe)?;
    Ok(format!(
        "[D-BUS Service]\nName={APP_ID}\nExec={program} --gapplication-service\n"
    ))
}

/// Writes `text` to `path` unless it already holds exactly that. Returns whether it wrote.
fn write_if_changed(path: &Path, text: &str) -> io::Result<bool> {
    if fs::read(path).ok().as_deref() == Some(text.as_bytes()) {
        return Ok(false);
    }
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(dir)?;
    // Write a sibling, then rename, so a crash never leaves a half-written entry.
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("entry"),
        std::process::id()
    ));
    let _ = fs::remove_file(&tmp); // a leftover from a crashed run
    {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        let written = f.write_all(text.as_bytes()).and_then(|_| f.sync_all());
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(true)
}

/// Brings the native files in line with `enabled`. Returns whether any file changed.
///
/// The service file is kept current either way: a notification already shown still needs it.
/// Turning the check off removes only the autostart entry, and needs no usable program path:
/// a path refused for the service file never keeps an old entry running at login.
pub fn reconcile_native(paths: &LoginPaths, exe: &Path, enabled: bool) -> io::Result<bool> {
    if !enabled {
        let mut changed = remove_autostart(paths)?;
        if let Ok(service) = dbus_service_text(exe) {
            changed |= write_if_changed(&paths.dbus_service, &service)?;
        }
        return Ok(changed);
    }
    // Build both texts first, so a path we refuse writes nothing at all.
    let service = dbus_service_text(exe)?;
    let autostart = autostart_desktop_text(exe)?;
    let mut changed = write_if_changed(&paths.dbus_service, &service)?;
    changed |= write_if_changed(&paths.autostart, &autostart)?;
    Ok(changed)
}

/// Removes the autostart entry. Returns whether one was there.
pub fn remove_autostart(paths: &LoginPaths) -> io::Result<bool> {
    match fs::remove_file(&paths.autostart) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// True when the autostart entry exists and the user turned it off outside this app (GNOME
/// Tweaks / Startup Applications write `X-GNOME-Autostart-enabled=false` or `Hidden=true`).
pub fn autostart_disabled_by_user(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    text.lines().map(str::trim).any(|l| {
        l.eq_ignore_ascii_case("Hidden=true")
            || l.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
    })
}

/// `RequestBackground` options (`a{sv}`).
pub fn background_request_options(enable: bool, handle_token: &str) -> glib::Variant {
    let d = glib::VariantDict::new(None);
    d.insert_value("handle_token", &handle_token.to_variant());
    d.insert_value(
        "reason",
        &crate::i18n::tr("Check installed apps for newer releases at login").to_variant(),
    );
    d.insert_value("autostart", &enable.to_variant());
    d.insert_value("commandline", &COMMANDLINE.to_vec().to_variant());
    d.insert_value("dbus-activatable", &false.to_variant());
    d.end()
}

/// What the Preferences switch shows after the portal answered a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginSwitch {
    pub active: bool,
    /// The desktop refused (or the request failed): show the refusal subtitle.
    pub refused: bool,
}

/// Reads a portal `Response(u response, a{sv} results)` for a request that asked for
/// `requested`.
pub fn portal_outcome(requested: bool, response: u32, results: &glib::Variant) -> LoginSwitch {
    if !requested {
        return LoginSwitch {
            active: false,
            refused: false,
        };
    }
    let granted = response == 0
        && glib::VariantDict::new(Some(results))
            .lookup::<bool>("autostart")
            .ok()
            .flatten()
            .unwrap_or(false);
    LoginSwitch {
        active: granted,
        refused: !granted,
    }
}

/// Set when the desktop refused the last request in this process, for the Preferences subtitle.
static LAST_REFUSED: AtomicBool = AtomicBool::new(false);

pub fn last_request_refused() -> bool {
    LAST_REFUSED.load(Ordering::Relaxed)
}

/// The last native setup error in this process, for the Preferences subtitle.
static LAST_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn last_error() -> Option<String> {
    LAST_ERROR.lock().ok().and_then(|g| g.clone())
}

fn set_last_error(e: Option<String>) {
    if let Ok(mut g) = LAST_ERROR.lock() {
        *g = e;
    }
}

fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// Refuses a program path that will not exist at the next login: a binary replaced while it
/// runs (`… (deleted)`), an AppImage's temporary mount, or a path that is not a file.
fn usable_program(p: &Path) -> io::Result<()> {
    let s = p.to_string_lossy();
    let why = if s.ends_with(" (deleted)") {
        Some("the running binary was replaced or deleted")
    } else if s.starts_with("/tmp/.mount_") {
        Some("it is inside an AppImage's temporary mount")
    } else if !p.is_file() {
        Some("it is not a file")
    } else {
        None
    };
    match why {
        Some(why) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("will not register {s} to run at login: {why}"),
        )),
        None => Ok(()),
    }
}

/// The program the login entry should start: the AppImage itself when we run from one (its
/// mount point is gone after exit), else this executable.
fn login_program() -> io::Result<PathBuf> {
    if let Some(p) = std::env::var_os("APPIMAGE").map(PathBuf::from) {
        let exe = std::env::current_exe()?;
        // Only when this process really runs from that AppImage's mount: a terminal opened
        // from an AppImage passes a stale $APPIMAGE to everything started in it.
        if p.is_absolute() && p.is_file() && exe.starts_with("/tmp/.mount_") {
            return Ok(p);
        }
    }
    let exe = std::env::current_exe()?;
    usable_program(&exe)?;
    Ok(exe)
}

/// A debug build registers only when asked, so a `cargo run` on a real home never takes over
/// the installed app's autostart entry and D-Bus service file.
fn dev_build_registration_allowed() -> bool {
    !cfg!(debug_assertions)
        || std::env::var_os("APPIMAGE_CONTROL_DEV_LOGIN_CHECK").is_some_and(|v| v == "1")
}

/// Registers or unregisters the login check. Natively this finishes before it returns;
/// inside Flatpak `on_answer` runs when the portal answers.
pub fn apply(enabled: bool, on_answer: impl Fn(LoginSwitch) + 'static) -> Result<(), String> {
    if in_flatpak() {
        return request_background(enabled, on_answer);
    }
    if !dev_build_registration_allowed() {
        eprintln!(
            "appimage-control: debug build; login check not registered \
             (set APPIMAGE_CONTROL_DEV_LOGIN_CHECK=1 to allow)"
        );
        return Ok(());
    }
    let paths = LoginPaths::from_env();
    let res = match login_program() {
        Ok(exe) => reconcile_native(&paths, &exe, enabled),
        // Turning it off needs no program path.
        Err(_) if !enabled => remove_autostart(&paths),
        Err(e) => Err(e),
    }
    .map(|_| ())
    .map_err(|e| e.to_string());
    set_last_error(res.as_ref().err().cloned());
    res
}

/// The object path the portal answers on: the sender's unique name without `:` and with `.`
/// as `_`, then the handle token.
fn portal_request_path(unique_name: &str, token: &str) -> String {
    format!(
        "/org/freedesktop/portal/desktop/request/{}/{}",
        unique_name.trim_start_matches(':').replace('.', "_"),
        token
    )
}

/// Sends `org.freedesktop.portal.Background.RequestBackground` and calls `on_answer` with its
/// outcome (a failed call counts as refused when enabling).
fn request_background(
    enable: bool,
    on_answer: impl Fn(LoginSwitch) + 'static,
) -> Result<(), String> {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|e| e.to_string())?;
    let sender = conn
        .unique_name()
        .ok_or_else(|| "no D-Bus unique name".to_string())?;
    let token = format!(
        "aic_login_{}_{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let request_path = portal_request_path(&sender, &token);
    let on_answer = Rc::new(on_answer);
    let finish: Rc<dyn Fn(LoginSwitch)> = {
        let on_answer = Rc::clone(&on_answer);
        Rc::new(move |sw: LoginSwitch| {
            LAST_REFUSED.store(sw.refused, Ordering::Relaxed);
            (on_answer)(sw);
        })
    };
    // Subscribe before calling, so a fast answer is not missed.
    let subscription: Rc<RefCell<Option<gio::SignalSubscription>>> = Rc::new(RefCell::new(None));
    let sub = conn.subscribe_to_signal(
        Some("org.freedesktop.portal.Desktop"),
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(&request_path),
        None,
        gio::DBusSignalFlags::NONE,
        {
            let subscription = Rc::clone(&subscription);
            let finish = Rc::clone(&finish);
            move |signal| {
                let params = signal.parameters;
                let response = params
                    .try_child_value(0)
                    .and_then(|v| v.get::<u32>())
                    .unwrap_or(2);
                let results = params
                    .try_child_value(1)
                    .unwrap_or_else(|| glib::VariantDict::new(None).end());
                (finish)(portal_outcome(enable, response, &results));
                subscription.borrow_mut().take();
            }
        },
    );
    *subscription.borrow_mut() = Some(sub);
    let args = glib::Variant::tuple_from_iter([
        "".to_variant(),
        background_request_options(enable, &token),
    ]);
    conn.call(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Background",
        "RequestBackground",
        Some(&args),
        Some(glib::VariantTy::new("(o)").expect("type string")),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        move |res| {
            if let Err(e) = res {
                eprintln!("appimage-control: background portal request failed: {e}");
                subscription.borrow_mut().take();
                (finish)(LoginSwitch {
                    active: false,
                    refused: enable,
                });
            }
        },
    );
    Ok(())
}

/// On startup: register (or unregister) per the saved preference. When the desktop refuses,
/// the preference is saved as off, so Preferences shows the truth.
///
/// Runs once per process (every new window would otherwise send another portal request). A
/// native entry the user turned off outside the app is their choice: the preference follows it.
pub fn reconcile_at_startup(paths: &std::rc::Rc<appimage_control_core::AppPaths>) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let enabled = appimage_control_core::GuiConfig::load(paths).check_updates_at_login;
    if enabled && !in_flatpak() && autostart_disabled_by_user(&LoginPaths::from_env().autostart) {
        let mut cfg = appimage_control_core::GuiConfig::load(paths);
        cfg.check_updates_at_login = false;
        if let Err(e) = cfg.save(paths) {
            eprintln!("appimage-control: could not save preferences: {e}");
        }
        // Keep the user's entry as it is; only the service file is refreshed.
        if let Ok(exe) = login_program() {
            if dev_build_registration_allowed() {
                let p = LoginPaths::from_env();
                if let Err(e) =
                    dbus_service_text(&exe).and_then(|t| write_if_changed(&p.dbus_service, &t))
                {
                    eprintln!("appimage-control: could not write the D-Bus service file: {e}");
                }
            }
        }
        return;
    }
    let paths = std::rc::Rc::clone(paths);
    let res = apply(enabled, move |sw| {
        if enabled && !sw.active {
            let mut cfg = appimage_control_core::GuiConfig::load(&paths);
            cfg.check_updates_at_login = false;
            if let Err(e) = cfg.save(&paths) {
                eprintln!("appimage-control: could not save preferences: {e}");
            }
        }
    });
    if let Err(e) = res {
        eprintln!("appimage-control: could not set up the login update check: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> (tempfile::TempDir, LoginPaths) {
        let t = tempfile::tempdir().unwrap();
        let p = LoginPaths::new(&t.path().join("config"), &t.path().join("data"));
        (t, p)
    }

    const EXE: &str = "/opt/aic/bin/appimage-control";

    #[test]
    fn login_check_paths_follow_xdg_dirs() {
        let p = LoginPaths::new(Path::new("/c"), Path::new("/d"));
        assert_eq!(
            p.autostart,
            Path::new("/c/autostart/io.github.sureserverman.AppImageControl.desktop")
        );
        assert_eq!(
            p.dbus_service,
            Path::new("/d/dbus-1/services/io.github.sureserverman.AppImageControl.service")
        );
    }

    #[test]
    fn login_check_enable_writes_autostart_and_service() {
        let (_t, p) = tmp();
        assert!(reconcile_native(&p, Path::new(EXE), true).unwrap());
        let auto = fs::read_to_string(&p.autostart).unwrap();
        assert!(auto.starts_with("[Desktop Entry]\n"), "{auto}");
        assert!(
            auto.lines()
                .any(|l| l == format!("Exec={EXE} --check-updates --notify")),
            "{auto}"
        );
        assert!(auto.lines().any(|l| l == "X-GNOME-Autostart-enabled=true"));
        assert!(auto.lines().any(|l| l == "Type=Application"));
        let svc = fs::read_to_string(&p.dbus_service).unwrap();
        assert_eq!(
            svc,
            format!(
                "[D-BUS Service]\nName=io.github.sureserverman.AppImageControl\nExec={EXE} --gapplication-service\n"
            )
        );
    }

    #[test]
    fn login_check_reconcile_twice_changes_nothing() {
        let (_t, p) = tmp();
        reconcile_native(&p, Path::new(EXE), true).unwrap();
        let a1 = fs::read(&p.autostart).unwrap();
        let s1 = fs::read(&p.dbus_service).unwrap();
        assert!(!reconcile_native(&p, Path::new(EXE), true).unwrap());
        assert_eq!(fs::read(&p.autostart).unwrap(), a1);
        assert_eq!(fs::read(&p.dbus_service).unwrap(), s1);
    }

    #[test]
    fn login_check_disable_removes_autostart_keeps_service() {
        let (_t, p) = tmp();
        reconcile_native(&p, Path::new(EXE), true).unwrap();
        assert!(reconcile_native(&p, Path::new(EXE), false).unwrap());
        assert!(!p.autostart.exists());
        assert!(
            p.dbus_service.exists(),
            "a notification click still needs the service"
        );
        assert!(!reconcile_native(&p, Path::new(EXE), false).unwrap());
    }

    #[test]
    fn login_check_disable_needs_no_usable_program() {
        let (_t, p) = tmp();
        reconcile_native(&p, Path::new(EXE), true).unwrap();
        for bad in ["relative/appimage-control", "/x/a;b/appimage-control"] {
            fs::write(&p.autostart, "[Desktop Entry]\n").unwrap();
            assert!(
                reconcile_native(&p, Path::new(bad), false).unwrap(),
                "{bad}"
            );
            assert!(!p.autostart.exists(), "{bad}");
            // The service file written from the good path is left alone.
            assert!(fs::read_to_string(&p.dbus_service).unwrap().contains(EXE));
        }
        fs::write(&p.autostart, "x").unwrap();
        assert!(remove_autostart(&p).unwrap());
        assert!(!remove_autostart(&p).unwrap());
    }

    #[test]
    fn login_check_rewrites_a_stale_exec() {
        let (_t, p) = tmp();
        reconcile_native(&p, Path::new("/old/appimage-control"), true).unwrap();
        assert!(reconcile_native(&p, Path::new(EXE), true).unwrap());
        assert!(fs::read_to_string(&p.autostart).unwrap().contains(EXE));
        assert!(fs::read_to_string(&p.dbus_service).unwrap().contains(EXE));
    }

    #[test]
    fn login_check_quotes_a_path_with_spaces() {
        let exe = Path::new("/home/a b/appimage-control");
        let auto = autostart_desktop_text(exe).unwrap();
        assert!(auto
            .lines()
            .any(|l| l == "Exec=\"/home/a b/appimage-control\" --check-updates --notify"));
        let svc = dbus_service_text(exe).unwrap();
        assert!(svc
            .lines()
            .any(|l| l == "Exec=\"/home/a b/appimage-control\" --gapplication-service"));
    }

    #[test]
    fn login_check_refuses_paths_it_cannot_quote() {
        for bad in [
            "relative/appimage-control",
            "/x/\"q/appimage-control",
            "/x/'q/appimage-control",
            "/x/$HOME/appimage-control",
            "/x/`id`/appimage-control",
            "/x/\\b/appimage-control",
            "/x/%f/appimage-control",
            "/x/new\nline/appimage-control",
        ] {
            assert!(autostart_desktop_text(Path::new(bad)).is_err(), "{bad:?}");
            assert!(dbus_service_text(Path::new(bad)).is_err(), "{bad:?}");
            let (_t, p) = tmp();
            assert!(
                reconcile_native(&p, Path::new(bad), true).is_err(),
                "{bad:?}"
            );
            assert!(!p.autostart.exists() && !p.dbus_service.exists(), "{bad:?}");
        }
    }

    #[test]
    fn login_check_refuses_every_reserved_exec_character() {
        for c in [';', '&', '|', '<', '>', '~', '*', '?', '#', '(', ')', '\t'] {
            let bad = format!("/x/a{c}b/appimage-control");
            assert!(autostart_desktop_text(Path::new(&bad)).is_err(), "{bad:?}");
            assert!(dbus_service_text(Path::new(&bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn login_check_autostart_has_tryexec() {
        let auto = autostart_desktop_text(Path::new(EXE)).unwrap();
        assert!(
            auto.lines().any(|l| l == format!("TryExec={EXE}")),
            "{auto}"
        );
    }

    #[test]
    fn login_check_respects_an_entry_the_user_turned_off() {
        let (_t, p) = tmp();
        assert!(!autostart_disabled_by_user(&p.autostart));
        reconcile_native(&p, Path::new(EXE), true).unwrap();
        assert!(!autostart_disabled_by_user(&p.autostart));
        for off in ["X-GNOME-Autostart-enabled=false", "Hidden=true"] {
            let text = fs::read_to_string(&p.autostart)
                .unwrap()
                .replace("X-GNOME-Autostart-enabled=true", off);
            fs::write(&p.autostart, text).unwrap();
            assert!(autostart_disabled_by_user(&p.autostart), "{off}");
        }
    }

    #[test]
    fn login_check_refuses_programs_gone_by_next_login() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("appimage-control");
        fs::write(&real, b"x").unwrap();
        assert!(usable_program(&real).is_ok());
        // Each refused for its own reason (the first two exist on disk, so `is_file` passes).
        let deleted = t.path().join("appimage-control (deleted)");
        fs::write(&deleted, b"x").unwrap();
        let err = |p: &Path| usable_program(p).unwrap_err().to_string();
        assert!(err(&deleted).contains("replaced or deleted"));
        assert!(
            err(Path::new("/tmp/.mount_abc123/usr/bin/appimage-control"))
                .contains("temporary mount")
        );
        assert!(err(&t.path().join("missing")).contains("not a file"));
    }

    #[test]
    fn login_check_write_leaves_no_temp_file() {
        let (_t, p) = tmp();
        reconcile_native(&p, Path::new(EXE), true).unwrap();
        for dir in [
            p.autostart.parent().unwrap(),
            p.dbus_service.parent().unwrap(),
        ] {
            let names: Vec<_> = fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            assert!(names.iter().all(|n| !n.contains(".tmp-")), "{names:?}");
        }
    }

    #[test]
    fn login_check_portal_request_path() {
        assert_eq!(
            portal_request_path(":1.42", "aic_login_7_0"),
            "/org/freedesktop/portal/desktop/request/1_42/aic_login_7_0"
        );
    }

    #[test]
    fn login_check_config_without_the_field_reads_enabled() {
        let cfg: appimage_control_core::GuiConfig =
            serde_json::from_str(r#"{"check_for_updates": false}"#).unwrap();
        assert!(cfg.check_updates_at_login);
        assert!(appimage_control_core::GuiConfig::default().check_updates_at_login);
    }

    #[test]
    fn login_check_background_options() {
        let v = background_request_options(true, "aic1");
        assert_eq!(v.type_(), glib::VariantTy::VARDICT);
        let d = glib::VariantDict::new(Some(&v));
        assert_eq!(d.lookup::<bool>("autostart").unwrap(), Some(true));
        assert_eq!(d.lookup::<bool>("dbus-activatable").unwrap(), Some(false));
        assert_eq!(
            d.lookup::<Vec<String>>("commandline").unwrap(),
            Some(vec![
                "appimage-control".to_string(),
                "--check-updates".to_string(),
                "--notify".to_string()
            ])
        );
        assert_eq!(
            d.lookup::<String>("handle_token").unwrap(),
            Some("aic1".to_string())
        );
        let reason = d.lookup::<String>("reason").unwrap().unwrap();
        assert!(!reason.trim().is_empty());
        let off = glib::VariantDict::new(Some(&background_request_options(false, "aic2")));
        assert_eq!(off.lookup::<bool>("autostart").unwrap(), Some(false));
    }

    fn results(autostart: Option<bool>) -> glib::Variant {
        let d = glib::VariantDict::new(None);
        d.insert_value("background", &true.to_variant());
        if let Some(a) = autostart {
            d.insert_value("autostart", &a.to_variant());
        }
        d.end()
    }

    #[test]
    fn login_check_portal_refusal_turns_the_switch_off() {
        assert_eq!(
            portal_outcome(true, 0, &results(Some(false))),
            LoginSwitch {
                active: false,
                refused: true
            }
        );
        assert_eq!(
            portal_outcome(true, 0, &results(Some(true))),
            LoginSwitch {
                active: true,
                refused: false
            }
        );
        // Cancelled by the user, other error, or no autostart key: not allowed.
        for (code, r) in [
            (1, results(Some(true))),
            (2, results(None)),
            (0, results(None)),
        ] {
            assert_eq!(
                portal_outcome(true, code, &r),
                LoginSwitch {
                    active: false,
                    refused: true
                },
                "response {code}"
            );
        }
        // Turning it off is never a refusal.
        assert_eq!(
            portal_outcome(false, 0, &results(Some(false))),
            LoginSwitch {
                active: false,
                refused: false
            }
        );
    }
}
