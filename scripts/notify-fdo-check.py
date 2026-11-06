#!/usr/bin/env python3
"""Check `--check-updates --notify` on a desktop whose notification server speaks only
org.freedesktop.Notifications (LXQt, KDE, Xfce).

Run it on a private session bus, from the repo root, after building the binary:

    dbus-run-session -- python3 scripts/notify-fdo-check.py

The bus has no org.gtk.Notifications, so the binary takes its freedesktop route. This script
plays the notification server and the running app: it records Notify, then "clicks" the
notice (ActionInvoked "default") and expects the binary to call
org.freedesktop.Application.ActivateAction("show-updates") on the app's bus name and exit 0.
A second run with a server that has no "actions" capability must get a plain notice and exit
at once. Exit 0 means every check passed.
"""

import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import warnings

from gi.repository import Gio, GLib

# register_object's plain-callback form is deprecated in PyGObject but still works.
warnings.filterwarnings("ignore", category=DeprecationWarning)

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GUI = os.path.join(REPO, "target", "debug", "appimage-control")
APP_ID = "io.github.sureserverman.AppImageControl"
APP_PATH = "/" + APP_ID.replace(".", "/")

NOTIFY_XML = """<node><interface name="org.freedesktop.Notifications">
  <method name="GetCapabilities"><arg type="as" direction="out"/></method>
  <method name="Notify">
    <arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/>
    <arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/>
    <arg type="a{sv}" direction="in"/><arg type="i" direction="in"/>
    <arg type="u" direction="out"/></method>
  <method name="CloseNotification"><arg type="u" direction="in"/></method>
  <method name="GetServerInformation">
    <arg type="s" direction="out"/><arg type="s" direction="out"/>
    <arg type="s" direction="out"/><arg type="s" direction="out"/></method>
  <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
  <signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
</interface></node>"""

APP_XML = """<node><interface name="org.freedesktop.Application">
  <method name="Activate"><arg type="a{sv}" direction="in"/></method>
  <method name="Open"><arg type="as" direction="in"/><arg type="a{sv}" direction="in"/></method>
  <method name="ActivateAction"><arg type="s" direction="in"/><arg type="av" direction="in"/>
    <arg type="a{sv}" direction="in"/></method>
</interface></node>"""

FIXTURE_CATALOG = {"apps": [{
    "id": "fixture", "display_name": "Fixture App",
    "download_url": "https://example.invalid/releases/fixture-2.0.0.AppImage",
    "appimage_filename": "fixture.AppImage", "instance_strategy": "electron_class",
}]}


class Fail(Exception):
    pass


def check(cond, msg):
    if not cond:
        raise Fail(msg)
    print(f"ok: {msg}", flush=True)


class FakeDesktop:
    """A notification server plus the running app, on one connection."""

    def __init__(self, conn, actions):
        self.conn, self.actions = conn, actions
        self.notifies, self.activated = [], []
        self.ids = []
        for xml, path, handler in ((NOTIFY_XML, "/org/freedesktop/Notifications", self.on_notify),
                                   (APP_XML, APP_PATH, self.on_app)):
            info = Gio.DBusNodeInfo.new_for_xml(xml).interfaces[0]
            self.ids.append(conn.register_object(path, info, handler, None, None))

    def on_notify(self, conn, sender, path, iface, method, params, inv):
        if method == "GetCapabilities":
            caps = ["body", "actions"] if self.actions else ["body"]
            inv.return_value(GLib.Variant("(as)", (caps,)))
        elif method == "Notify":
            self.notifies.append(params.unpack())
            nid = len(self.notifies) + 41
            inv.return_value(GLib.Variant("(u)", (nid,)))
            if self.actions:
                # Another notice's click first: it must be ignored.
                GLib.timeout_add(300, self.emit, "ActionInvoked", GLib.Variant("(us)", (nid + 100, "default")))
                GLib.timeout_add(600, self.emit, "ActionInvoked", GLib.Variant("(us)", (nid, "default")))
                GLib.timeout_add(700, self.emit, "NotificationClosed", GLib.Variant("(uu)", (nid, 2)))
        elif method == "GetServerInformation":
            inv.return_value(GLib.Variant("(ssss)", ("fake", "aic", "1", "1.2")))
        else:
            inv.return_value(None)

    def on_app(self, conn, sender, path, iface, method, params, inv):
        if method == "ActivateAction":
            self.activated.append(params.unpack()[0])
        inv.return_value(None)

    def emit(self, name, params):
        self.conn.emit_signal(None, "/org/freedesktop/Notifications",
                              "org.freedesktop.Notifications", name, params)
        return False


def own(conn, name):
    r = conn.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                       "RequestName", GLib.Variant("(su)", (name, 4)), GLib.VariantType("(u)"),
                       Gio.DBusCallFlags.NONE, -1, None)
    if r.unpack()[0] != 1:
        raise Fail(f"could not own {name} on the private bus")


def run_binary(home, catalog, loop_ctx):
    """Runs the check in a thread, so this thread's main loop can answer it."""
    result = {}

    def target():
        env = dict(os.environ, HOME=home)
        for k in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "GNOTIFICATION_BACKEND",
                  "APPIMAGE_CONTROL_CATALOG"):
            env.pop(k, None)
        start = time.time()
        p = subprocess.run([GUI, "--check-updates", "--notify", "--catalog", catalog], env=env,
                           capture_output=True, text=True, timeout=60)
        result.update(rc=p.returncode, out=p.stdout, err=p.stderr, secs=time.time() - start)
        GLib.idle_add(loop_ctx.quit)

    threading.Thread(target=target, daemon=True).start()
    loop_ctx.run()
    return result


def scenario(actions):
    home = tempfile.mkdtemp(prefix="aic-fdo-home-")
    inst = os.path.join(home, ".local/share/appimages/apps/fixture/1")
    os.makedirs(inst)
    with open(os.path.join(inst, "fixture.AppImage"), "w") as f:
        f.write("#!/bin/sh\n")
    with open(os.path.join(inst, ".appimage-control-version"), "w") as f:
        f.write("1.0.0\n")
    catalog = os.path.join(home, "catalog.json")
    with open(catalog, "w") as f:
        json.dump(FIXTURE_CATALOG, f)
    conn = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    fake = FakeDesktop(conn, actions)
    loop = GLib.MainLoop()
    try:
        r = run_binary(home, catalog, loop)
    finally:
        for i in fake.ids:
            conn.unregister_object(i)
        subprocess.run(["rm", "-rf", home], check=False)
    return fake, r


def main():
    if not os.path.exists(GUI):
        print(f"build first: {GUI} is missing", file=sys.stderr)
        return 2
    conn = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    try:
        own(conn, "org.freedesktop.Notifications")
        own(conn, APP_ID)
        has_gtk = conn.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus",
                                 "org.freedesktop.DBus", "NameHasOwner",
                                 GLib.Variant("(s)", ("org.gtk.Notifications",)),
                                 GLib.VariantType("(b)"), Gio.DBusCallFlags.NONE, -1, None)
        check(not has_gtk.unpack()[0], "the private bus has no org.gtk.Notifications")

        fake, r = scenario(actions=True)
        check(r["rc"] == 0, f"exit 0 (stderr: {r['err'].strip()!r})")
        check(r["out"] == "fixture: 1.0.0 -> 2.0.0\n", "it lists the outdated app")
        check(len(fake.notifies) == 1, "one Notify call")
        _app, _rid, _icon, summary, body, actions, hints, _t = fake.notifies[0]
        check(summary == "Updates available" and body == "Fixture App",
              f"title and body name the app ({summary!r}, {body!r})")
        check(actions[:1] == ["default"], f"it offers the default action ({actions})")
        check(hints.get("desktop-entry") == APP_ID, "desktop-entry hint names the app")
        check(fake.activated == ["show-updates"],
              f"the click asked the app to run show-updates, once ({fake.activated})")
        check(r["secs"] < 10, f"it left soon after the click ({r['secs']:.1f} s)")

        fake, r = scenario(actions=False)
        check(r["rc"] == 0, "exit 0 with a server that has no actions")
        check(len(fake.notifies) == 1 and fake.notifies[0][5] == [],
              "a plain notice, with no actions")
        check(fake.activated == [], "nothing activated")
        check(r["secs"] < 5, f"it did not wait ({r['secs']:.1f} s)")
    except Fail as e:
        print(f"FAIL: {e}", file=sys.stderr)
        return 1
    print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
