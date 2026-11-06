#!/usr/bin/env python3
"""Drive the AppImage Control GUI through AT-SPI on a live desktop session (the test VM).

Run from the repo root on a machine with a logged-in GNOME session, python3-pyatspi and the
Rust toolchain:

    python3 scripts/atspi-smoke.py details       # one app's detail page: Launch, Open folder
    python3 scripts/atspi-smoke.py details-all   # every card has a working details button
    python3 scripts/atspi-smoke.py activation    # D-Bus activation of app.show-updates

Each mode builds the binaries, makes a throwaway HOME with fixture installs, starts the GUI
with GTK_A11Y=atspi, checks the result, and stops the GUI. Exit 0 means every check passed.
Set AIC_DUMP=1 to print the dialog's accessibility tree when the details check runs.
"""

import json
import os
import pwd
import shutil
import signal
import subprocess
import sys
import tempfile
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GUI = os.path.join(REPO, "target", "debug", "appimage-control")
REPAIR = os.path.join(REPO, "target", "debug", "repair-launchers")
CATALOG = os.path.join(REPO, "data", "catalog.json")
UID = os.getuid()
HOME_PREFIX = "aic-smoke-home-"
# The logged-in session's defaults; values already in the environment win.
for k, v in {
    "XDG_RUNTIME_DIR": f"/run/user/{UID}",
    "WAYLAND_DISPLAY": "wayland-0",
    "DBUS_SESSION_BUS_ADDRESS": f"unix:path=/run/user/{UID}/bus",
}.items():
    os.environ.setdefault(k, v)
import pyatspi  # noqa: E402  (needs the session bus address above)


class Fail(Exception):
    pass


def log(msg):
    print(msg, flush=True)


def check(cond, msg):
    if not cond:
        raise Fail(msg)
    log(f"ok: {msg}")


def build():
    subprocess.run(
        ["cargo", "build", "-q", "-p", "appimage-control", "-p", "appimage_control_core",
         "--bin", "appimage-control", "--bin", "repair-launchers"],
        cwd=REPO, check=True)


def catalog_apps():
    with open(CATALOG) as f:
        return json.load(f)["apps"]


def fixture_home(home, installs):
    """Fill `home` with one instance per (app id, AppImage filename, version, script body)."""
    env = gui_env(home)
    for app_id, filename, version, body in installs:
        d = os.path.join(home, ".local/share/appimages/apps", app_id, "1")
        os.makedirs(d)
        p = os.path.join(d, filename)
        with open(p, "w") as f:
            f.write("#!/bin/sh\n" + body.replace("{home}", home) + "\n")
        os.chmod(p, 0o755)
        if version:
            with open(os.path.join(d, ".appimage-control-version"), "w") as f:
                f.write(version + "\n")
    cfg = os.path.join(home, ".config/appimage-control")
    os.makedirs(cfg, exist_ok=True)
    with open(os.path.join(cfg, "config.json"), "w") as f:
        json.dump({"check_for_updates": False, "repair_launchers_auto": False}, f)
    # The real launcher writer: launch script + .desktop for each instance.
    subprocess.run([REPAIR], env=env, check=True, capture_output=True)


def gui_env(home):
    env = dict(os.environ)
    env.update({
        "HOME": home,
        "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        "XDG_DATA_HOME": os.path.join(home, ".local/share"),
        "XDG_CACHE_HOME": os.path.join(home, ".cache"),
        "APPIMAGE_CONTROL_CATALOG": CATALOG,
        "GTK_A11Y": "atspi",
        "LANGUAGE": "en",
        "LANG": "C.UTF-8",
    })
    return env


def running_guis():
    """(pid, HOME) of every AppImage Control GUI this user runs."""
    out = []
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            with open(f"/proc/{pid}/comm") as f:
                if f.read().strip() != "appimage-contro":
                    continue
            with open(f"/proc/{pid}/environ", "rb") as f:
                env = dict(kv.split(b"=", 1) for kv in f.read().split(b"\0") if b"=" in kv)
            out.append((int(pid), env.get(b"HOME", b"").decode(errors="replace")))
        except OSError:
            continue
    return out


def clear_the_way():
    """The GUI is single-instance, so a running one would take our activation. Stop only GUIs a
    smoke run left behind; refuse to touch anybody's real session."""
    for pid, home in running_guis():
        if os.path.basename(home).startswith(HOME_PREFIX):
            os.kill(pid, signal.SIGTERM)
        else:
            raise Fail(f"AppImage Control is already running (pid {pid}, HOME {home}); "
                       "close it first")
    end = time.time() + 5
    while running_guis() and time.time() < end:
        time.sleep(0.2)


def start_gui(home, args=()):
    clear_the_way()
    log_path = os.path.join(home, "gui-stderr.log")
    proc = subprocess.Popen([GUI, *args], env=gui_env(home), stdout=subprocess.DEVNULL,
                            stderr=open(log_path, "w"))
    proc.log_path = log_path
    return proc


def stop_gui(proc):
    if proc.poll() is None:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()


def gui_criticals(proc):
    try:
        with open(proc.log_path) as f:
            return [l.rstrip() for l in f if "-CRITICAL" in l]
    except OSError:
        return []


def show_gui_log(proc):
    try:
        with open(proc.log_path) as f:
            tail = f.readlines()[-20:]
    except OSError:
        return
    print("--- GUI stderr (last 20 lines) ---", file=sys.stderr)
    sys.stderr.writelines(tail)


def app_root(pid, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        desktop = pyatspi.Registry.getDesktop(0)
        for i in range(desktop.childCount):
            a = desktop.getChildAtIndex(i)
            try:
                if a is not None and a.get_process_id() == pid:
                    return a
            except Exception:
                pass
        time.sleep(0.5)
    raise Fail(f"GUI pid {pid} never appeared on the accessibility bus")


def walk(node, depth=0, limit=60):
    if node is None or depth > limit:
        return
    yield node
    try:
        n = node.childCount
    except Exception:
        return
    for i in range(n):
        try:
            child = node.getChildAtIndex(i)
        except Exception:
            continue
        yield from walk(child, depth + 1, limit)


def role(node):
    try:
        return node.getRoleName()
    except Exception:
        return ""


def name(node):
    try:
        return node.name or ""
    except Exception:
        return ""


def find_all(root, pred):
    return [n for n in walk(root) if pred(n)]


def wait_for(root, pred, what, timeout=15):
    end = time.time() + timeout
    while time.time() < end:
        found = find_all(root, pred)
        if found:
            return found[0]
        time.sleep(0.3)
    raise Fail(f"timed out waiting for {what}")


def click(node):
    try:
        node.queryAction().doAction(0)
    except Exception:
        pass  # a click that opens a dialog can time out on the reply; the effect is checked


def sensitive(node):
    return node.getState().contains(pyatspi.STATE_SENSITIVE)


def is_dialog(n, title):
    return role(n) in ("dialog", "frame", "window") and name(n) == title


def open_details(root, display_name):
    btn = wait_for(root, lambda n: role(n) == "button" and name(n) == f"Details for {display_name}",
                   f"details button for {display_name}")
    click(btn)
    return wait_for(root, lambda n: is_dialog(n, display_name), f"dialog '{display_name}'")


def close_dialog(root, dialog, title):
    close = [n for n in walk(dialog) if role(n) == "button" and name(n) == "Close"]
    if not close:
        raise Fail(f"dialog '{title}' has no Close button")
    click(close[0])
    end = time.time() + 10
    while time.time() < end:
        if not find_all(root, lambda n: is_dialog(n, title)):
            return
        time.sleep(0.3)
    raise Fail(f"dialog '{title}' did not close")


def run_with_gui(installs, body):
    """Make a fixture HOME, start the GUI on it, run `body(root, home)`, always clean up."""
    home = tempfile.mkdtemp(prefix=HOME_PREFIX)
    proc = None
    try:
        fixture_home(home, installs)
        proc = start_gui(home)
        try:
            body(app_root(proc.pid), home)
        except Fail:
            show_gui_log(proc)
            raise
        crit = gui_criticals(proc)
        check(not crit, "no GTK/Adwaita CRITICAL messages" + (f": {crit[:3]}" if crit else ""))
    finally:
        if proc is not None:
            stop_gui(proc)
        # Launched fixtures run in their own process group; stop every process using the home.
        subprocess.run(["pkill", "-f", "--", home], check=False)
        shutil.rmtree(home, ignore_errors=True)


def wait_file(path, secs):
    end = time.time() + secs
    while time.time() < end and not os.path.exists(path):
        time.sleep(0.2)
    return os.path.exists(path)


def mode_details():
    def body(root, home):
        marker = os.path.join(home, "launched")
        dialog = open_details(root, "KeePassXC")
        try:
            wait_for(dialog, lambda n: name(n) == "Instance #1", "row 'Instance #1'")
        finally:
            if os.environ.get("AIC_DUMP"):
                for n in walk(dialog):
                    log(f"  {role(n)!r} {name(n)!r}")
        check(True, "the KeePassXC window lists 'Instance #1'")
        launch = wait_for(dialog, lambda n: role(n) == "button" and name(n) == "Launch instance #1",
                          "button 'Launch instance #1'")
        opener = wait_for(dialog, lambda n: role(n) == "button"
                          and name(n) == "Open the folder of instance #1", "Open folder button")
        check(sensitive(opener), "Open folder is sensitive")
        check(sensitive(launch), "Launch is sensitive")
        wait_for(dialog, lambda n: name(n) == "Latest release", "row 'Latest release'")
        check(not any("Not known yet" in name(n) for n in walk(dialog)),
              "'Latest release' shows a version, not 'Not known yet'")
        bad = [name(n) for n in walk(dialog) if role(n) == "button"
               and any(w in name(n) for w in ("Remove", "Delete", "Uninstall"))]
        check(not bad, f"no removal button on the page (OD-1){': ' + str(bad) if bad else ''}")
        click(launch)
        check(wait_file(marker, 5), "Launch started the instance within 5 s")
        check(find_all(root, lambda n: is_dialog(n, "KeePassXC")) != [],
              "the window stays open after Launch")
        # A failed launch: the launcher disappears while the page is open.
        launcher = os.path.join(home, ".local/share/applications/appimage-control-keepassxc-1.desktop")
        os.remove(launcher)
        end = time.time() + 6
        while time.time() < end and not sensitive(launch):
            time.sleep(0.3)  # Launch is held for a few seconds after a start
        click(launch)
        wait_for(dialog, lambda n: "Could not launch KeePassXC #1" in name(n),
                 "toast 'Could not launch KeePassXC #1'")
        check(True, "a failed launch shows a toast naming the app and instance")
        show = wait_for(dialog, lambda n: role(n) == "button" and name(n) == "Show log",
                        "toast button 'Show log'")
        click(show)
        logwin = wait_for(root, lambda n: role(n) in ("dialog", "frame", "window")
                          and name(n) == "Activity log", "the activity log window")
        wait_for(logwin, lambda n: "1 error" in name(n), "the log's '1 error' summary")
        check(True, "Show log opens the activity log with the launch error")
        check(find_all(root, lambda n: is_dialog(n, "KeePassXC")) != [],
              "the window stays open after a failed launch")

    run_with_gui([("keepassxc", "KeePassXC.AppImage", "2.7.9",
                   'touch "{home}/launched"\nsleep 30')], body)


def display_name(app):
    """CatalogApp::display_name_resolved."""
    return app.get("display_name") or app["id"].replace("-", " ")


def mode_details_all():
    apps = catalog_apps()
    titles = [display_name(a) for a in apps]
    if len(set(titles)) != len(titles):
        raise Fail("two catalog apps share a display name; the check would be ambiguous")

    def details_buttons(root):
        return sorted(name(n)[len("Details for "):] for n in walk(root)
                      if role(n) == "button" and name(n).startswith("Details for "))

    def body(root, home):
        end = time.time() + 30
        got = []
        while time.time() < end:
            got = details_buttons(root)
            if len(got) >= len(titles):
                break
            time.sleep(0.5)
        check(got == sorted(titles),
              f"one details button per catalog app ({len(got)} of {len(titles)})")
        for title in titles:
            dialog = open_details(root, title)
            wait_for(dialog, lambda n: "No instances installed" in name(n),
                     f"{title}: the empty state")
            check(True, f"{title}: the window opens and shows the empty state")
            close_dialog(root, dialog, title)

    run_with_gui([], body)


APP_ID = "io.github.sureserverman.AppImageControl"
OBJECT_PATH = "/" + APP_ID.replace(".", "/")
# The session bus reads service files from the real user's data dir, never a test HOME.
# $HOME may be a build home (the VM helper sets one), so take the passwd entry.
REAL_SERVICE = os.path.join(pwd.getpwuid(UID).pw_dir,
                            f".local/share/dbus-1/services/{APP_ID}.service")


def bus_call(dest, path, method, *args, timeout=30):
    return subprocess.run(["gdbus", "call", "--session", "--timeout", str(timeout), "--dest", dest,
                           "--object-path", path, "--method", method, *args],
                          capture_output=True, text=True)


def bus_owner_pid():
    """PID of the process owning the app's bus name, or None."""
    r = bus_call("org.freedesktop.DBus", "/org/freedesktop/DBus",
                 "org.freedesktop.DBus.GetConnectionUnixProcessID", APP_ID)
    if r.returncode != 0:
        return None
    return int(r.stdout.strip().strip("(),").split()[-1])


def activate_show_updates():
    """What GNOME Shell does when the update notification is clicked."""
    r = bus_call(APP_ID, OBJECT_PATH, "org.freedesktop.Application.ActivateAction",
                 "show-updates", "[]", "{}")
    if r.returncode != 0:
        raise Fail(f"ActivateAction show-updates failed: {r.stderr.strip()}")


class DevService:
    """Points the app's D-Bus service file at the dev binary, run on a fixture HOME. Any file
    already there is put back afterwards."""

    def __init__(self, home):
        self.home = home
        self.backup = None

    def __enter__(self):
        os.makedirs(os.path.dirname(REAL_SERVICE), exist_ok=True)
        if os.path.exists(REAL_SERVICE):
            self.backup = REAL_SERVICE + ".aic-smoke-backup"
            shutil.move(REAL_SERVICE, self.backup)
        env = gui_env(self.home)
        keep = ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
                "APPIMAGE_CONTROL_CATALOG", "GTK_A11Y", "LANGUAGE", "LANG", "WAYLAND_DISPLAY"]
        assigns = " ".join(f"{k}={env[k]}" for k in keep)
        self.log_path = os.path.join(self.home, "gui-stderr.log")
        with open(REAL_SERVICE, "w") as f:
            f.write("[D-BUS Service]\n"
                    f"Name={APP_ID}\n"
                    f"Exec=/bin/sh -c 'exec env {assigns} {GUI} --gapplication-service "
                    f"2>>{self.log_path}'\n")
        subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
                        "--object-path", "/org/freedesktop/DBus",
                        "--method", "org.freedesktop.DBus.ReloadConfig"], capture_output=True)
        return self

    def __exit__(self, *exc):
        pid = bus_owner_pid()
        if pid:
            try:
                os.kill(pid, signal.SIGTERM)
            except OSError:
                pass
        os.remove(REAL_SERVICE)
        if self.backup:
            shutil.move(self.backup, REAL_SERVICE)
        subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
                        "--object-path", "/org/freedesktop/DBus",
                        "--method", "org.freedesktop.DBus.ReloadConfig"], capture_output=True)


def is_checked(node):
    st = node.getState()
    return st.contains(pyatspi.STATE_CHECKED) or st.contains(pyatspi.STATE_PRESSED)


def updates_toggle(root):
    return wait_for(root, lambda n: role(n) == "toggle button" and name(n) == "Updates",
                    "the 'Updates' filter toggle")


def main_windows(root):
    return find_all(root, lambda n: role(n) == "frame" and name(n) == "AppImage Control")


def wait_checked(node, secs=10):
    end = time.time() + secs
    while time.time() < end and not is_checked(node):
        time.sleep(0.3)
    return is_checked(node)


def mode_activation():
    home = tempfile.mkdtemp(prefix=HOME_PREFIX)
    try:
        fixture_home(home, [])
        clear_the_way()
        check(bus_owner_pid() is None, "no instance owns the app's bus name before the call")
        with DevService(home) as svc:
            activate_show_updates()
            end = time.time() + 15
            pid = None
            while time.time() < end and pid is None:
                pid = bus_owner_pid()
                time.sleep(0.3)
            check(pid is not None, "D-Bus activation started the app")
            root = app_root(pid)
            wait_for(root, lambda n: role(n) == "frame" and name(n) == "AppImage Control",
                     "the main window")
            check(True, "a window appears after ActivateAction show-updates")
            check(wait_checked(updates_toggle(root)), "the 'Updates' filter is on")
            # The window is already open: switch the filter away, then click again.
            all_toggle = wait_for(root, lambda n: role(n) == "toggle button" and name(n) == "All",
                                  "the 'All' filter toggle")
            click(all_toggle)
            end = time.time() + 5
            while time.time() < end and is_checked(updates_toggle(root)):
                time.sleep(0.3)
            check(not is_checked(updates_toggle(root)), "the filter moved to 'All'")
            activate_show_updates()
            check(bus_owner_pid() == pid, "the open instance handled the second call")
            check(wait_checked(updates_toggle(root)),
                  "the open window switches back to 'Updates'")
            check(len(main_windows(root)) == 1, "still one main window")
            try:
                with open(svc.log_path) as f:
                    crit = [l.rstrip() for l in f if "-CRITICAL" in l]
            except OSError:
                crit = []
            check(not crit, "no GTK/Adwaita CRITICAL messages" + (f": {crit[:3]}" if crit else ""))
    finally:
        subprocess.run(["pkill", "-f", "--", home], check=False)
        shutil.rmtree(home, ignore_errors=True)


MODES = {
    "details": mode_details,
    "details-all": mode_details_all,
    "activation": mode_activation,
}


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in MODES:
        print(f"usage: {sys.argv[0]} {{{'|'.join(MODES)}}}", file=sys.stderr)
        return 2
    build()
    try:
        MODES[sys.argv[1]]()
    except Fail as e:
        print(f"FAIL: {e}", file=sys.stderr)
        return 1
    log("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
