#!/usr/bin/env python3
import sys

from ds_store import DSStore


def fail(message):
    raise SystemExit(f"ERROR: {message}")


if len(sys.argv) != 2:
    fail("usage: verify_dmg_layout.py path/to/.DS_Store")

records = {}
with DSStore.open(sys.argv[1], "r") as store:
    for entry in store:
        records[(entry.filename, entry.code)] = entry.value

if records.get(("Velvt.app", b"Iloc")) != (165, 225):
    fail("Velvt.app icon is not at the approved install-window position")
if records.get(("Applications", b"Iloc")) != (505, 225):
    fail("Applications icon is not at the approved install-window position")

window = records.get((".", b"bwsp"))
if not isinstance(window, dict):
    fail("Finder window settings are missing")
if window.get("WindowBounds") != "{{120, 120}, {660, 420}}":
    fail("Finder window bounds are not 660x420")
for key in ("ShowPathbar", "ShowSidebar", "ShowStatusBar", "ShowTabView", "ShowToolbar"):
    if window.get(key) is not False:
        fail(f"Finder chrome setting {key} must be disabled")

icon_view = records.get((".", b"icvp"))
if not isinstance(icon_view, dict):
    fail("Finder icon-view settings are missing")
expected = {
    "arrangeBy": "none",
    "backgroundType": 2,
    "iconSize": 96.0,
    "labelOnBottom": True,
    "showIconPreview": False,
    "textSize": 14.0,
}
for key, value in expected.items():
    if icon_view.get(key) != value:
        fail(f"Finder icon-view setting {key} is not {value!r}")

print("DMG Finder layout metadata passed")
