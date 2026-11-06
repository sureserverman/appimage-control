#!/usr/bin/env python3
"""Drive the AppImage Control GUI through AT-SPI on a live desktop session (the test VM).

Run from the repo root on a machine with a logged-in GNOME session, python3-pyatspi and the
Rust toolchain:

    python3 scripts/atspi-smoke.py details       # one app's detail page: Launch, Open folder
    python3 scripts/atspi-smoke.py details-all   # every card has a working details button

Each mode builds the binaries, makes a throwaway HOME with fixture installs, starts the GUI
with GTK_A11Y=atspi, checks the result, and stops the GUI. Exit 0 means every check passed.
Set AIC_DUMP=1 to print the dialog's accessibility tree when the details check runs.
"""

import json
import os
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
SESSION_ENV = {
    "XDG_RUNTIME_DIR": f"/run/user/{UID}",
    "WAYLAND_DISPLAY": "wayland-0",
    "DBUS_SESSION_BUS_ADDRESS": f"unix:path=/run/user/{UID}/bus",
}
LAUNCH_MARKER = "/tmp/aic-launched"

os.environ.update(SESSION_ENV)
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


def fixture_home(installs):
    """A HOME holding one instance per (app id, AppImage filename, version, script body)."""
    home = tempfile.mkdtemp(prefix="aic-smoke-home-")
    env = gui_env(home)
    for app_id, filename, version, body in installs:
        d = os.path.join(home, ".local/share/appimages/apps", app_id, "1")
        os.makedirs(d)
        p = os.path.join(d, filename)
        with open(p, "w") as f:
            f.write("#!/bin/sh\n" + body + "\n")
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
    return home


def gui_env(home):
    env = dict(os.environ)
    env.update(SESSION_ENV)
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


def stop_running_guis():
    subprocess.run(["pkill", "-x", "appimage-contro"], check=False)
    time.sleep(0.5)


def start_gui(home, args=()):
    stop_running_guis()
    proc = subprocess.Popen([GUI, *args], env=gui_env(home),
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return proc


def stop_gui(proc):
    if proc.poll() is None:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            proc.kill()


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


def mode_details():
    if os.path.exists(LAUNCH_MARKER):
        os.remove(LAUNCH_MARKER)
    home = fixture_home([("keepassxc", "KeePassXC.AppImage", "2.7.9",
                          f"touch {LAUNCH_MARKER}\nsleep 30")])
    proc = start_gui(home)
    try:
        root = app_root(proc.pid)
        dialog = open_details(root, "KeePassXC")
        check(True, "the KeePassXC details dialog opened")
        try:
            wait_for(dialog, lambda n: "#1" in name(n), "instance #1 in the dialog")
        finally:
            if os.environ.get("AIC_DUMP"):
                for n in walk(dialog):
                    log(f"  {role(n)!r} {name(n)!r}")
        check(True, "the dialog lists instance #1")
        wait_for(dialog, lambda n: role(n) == "button" and name(n) == "Launch", "Launch button")
        launch = [n for n in walk(dialog) if role(n) == "button" and name(n) == "Launch"]
        opener = [n for n in walk(dialog) if role(n) == "button" and name(n) == "Open folder"]
        check(len(launch) == 1 and len(opener) == 1, "one Launch and one Open folder button")
        check(sensitive(opener[0]), "Open folder is sensitive")
        check(sensitive(launch[0]), "Launch is sensitive")
        click(launch[0])
        end = time.time() + 5
        while time.time() < end and not os.path.exists(LAUNCH_MARKER):
            time.sleep(0.2)
        check(os.path.exists(LAUNCH_MARKER), "Launch started the instance within 5 s")
        check(find_all(root, lambda n: is_dialog(n, "KeePassXC")) != [],
              "the dialog stays open after Launch")
    finally:
        stop_gui(proc)
        subprocess.run(["pkill", "-f", os.path.join(home, ".local/share/appimages")], check=False)
        shutil.rmtree(home, ignore_errors=True)


def mode_details_all():
    apps = catalog_apps()
    home = fixture_home([])
    proc = start_gui(home)
    try:
        root = app_root(proc.pid)
        wait_for(root, lambda n: role(n) == "button" and name(n).startswith("Details for "),
                 "any details button")
        buttons = find_all(root, lambda n: role(n) == "button" and name(n).startswith("Details for "))
        check(len(buttons) == len(apps),
              f"{len(buttons)} details buttons for {len(apps)} catalog apps")
        for a in apps:
            title = a.get("display_name") or a["id"]
            dialog = open_details(root, title)
            check(any("No instances installed" in name(n) for n in walk(dialog)),
                  f"{title}: the dialog opens and shows the empty state")
            close_dialog(root, dialog, title)
    finally:
        stop_gui(proc)
        shutil.rmtree(home, ignore_errors=True)


MODES = {
    "details": mode_details,
    "details-all": mode_details_all,
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
