"""Adds RustVR to Steam as a non-Steam game, flagged as VR so it shows in SteamVR's library.

    python tools/add_steam_shortcut.py                 # add / update "RustVR" (Steam must be closed)
    python tools/add_steam_shortcut.py --restart-steam # close Steam, add it, start Steam again
    python tools/add_steam_shortcut.py --remove        # take it out again

Steam keeps its shortcuts in memory and writes them back when it exits, so the file must be
edited while Steam is closed. The previous file is kept as shortcuts.vdf.rustvr-backup.
"""

import argparse
import binascii
import os
import re
import shutil
import struct
import subprocess
import sys
import time
import winreg

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_EXE = os.path.join(REPO, "target", "release", "rustvr-play.exe")
STEAMID64_BASE = 76561197960265728


# ---- binary VDF ---------------------------------------------------------------------------------
# Each entry is a type byte, a NUL-terminated key, then the value: 0x00 a nested map (ended by 0x08),
# 0x01 a NUL-terminated UTF-8 string, 0x02 a little-endian int32.

def _cstr(data, i):
    end = data.index(b"\x00", i)
    return data[i:end].decode("utf-8", "replace"), end + 1


def parse_map(data, i=0):
    out = {}
    while True:
        kind = data[i]
        i += 1
        if kind == 0x08:
            return out, i
        key, i = _cstr(data, i)
        if kind == 0x00:
            out[key], i = parse_map(data, i)
        elif kind == 0x01:
            out[key], i = _cstr(data, i)
        elif kind == 0x02:
            out[key] = struct.unpack_from("<i", data, i)[0]
            i += 4
        else:
            raise ValueError(f"unsupported VDF type {kind:#x} at {i - 1}")


def write_map(m):
    out = bytearray()
    for key, value in m.items():
        k = key.encode("utf-8") + b"\x00"
        if isinstance(value, dict):
            out += b"\x00" + k + write_map(value)
        elif isinstance(value, int):
            out += b"\x02" + k + struct.pack("<i", value)
        else:
            out += b"\x01" + k + str(value).encode("utf-8") + b"\x00"
    out += b"\x08"
    return bytes(out)


# ---- Steam --------------------------------------------------------------------------------------

def steam_dir():
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Valve\Steam") as k:
        return os.path.normpath(winreg.QueryValueEx(k, "SteamPath")[0])


def active_account(steam):
    """The account that logged in most recently (MostRecent, else the newest Timestamp)."""
    text = open(os.path.join(steam, "config", "loginusers.vdf"), encoding="utf-8").read()
    best = None
    for sid, body in re.findall(r'"(\d{17})"\s*\{(.*?)\}', text, re.S):
        name = re.search(r'"PersonaName"\s+"([^"]*)"', body)
        recent = re.search(r'"MostRecent"\s+"(\d)"', body)
        ts = re.search(r'"Timestamp"\s+"(\d+)"', body)
        key = (recent and recent.group(1) == "1", int(ts.group(1)) if ts else 0)
        if best is None or key > best[0]:
            best = (key, int(sid) - STEAMID64_BASE, name.group(1) if name else sid)
    if best is None:
        sys.exit("no Steam accounts found in loginusers.vdf")
    return best[1], best[2]


def steam_running():
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq steam.exe", "/NH"], capture_output=True, text=True).stdout
    return "steam.exe" in out.lower()


def shutdown_steam(steam):
    subprocess.run([os.path.join(steam, "steam.exe"), "-shutdown"], check=False)
    for _ in range(120):
        if not steam_running():
            return
        time.sleep(0.5)
    sys.exit("Steam didn't close within a minute; close it yourself and run this again")


def shortcut_appid(exe, name):
    # The id other tools use for non-Steam shortcuts (also what grid artwork is keyed on).
    crc = binascii.crc32((exe + name).encode("utf-8")) | 0x80000000
    return struct.unpack("<i", struct.pack("<I", crc))[0]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--name", default="RustVR")
    ap.add_argument("--exe", default=DEFAULT_EXE)
    ap.add_argument("--launch-options", default="")
    ap.add_argument("--remove", action="store_true")
    ap.add_argument("--restart-steam", action="store_true", help="close Steam first and start it again afterwards")
    args = ap.parse_args()

    steam = steam_dir()
    account, persona = active_account(steam)
    path = os.path.join(steam, "userdata", str(account), "config", "shortcuts.vdf")
    exe = os.path.abspath(args.exe)
    if not args.remove and not os.path.exists(exe):
        sys.exit(f"{exe} doesn't exist: build it first (cargo build --release -p rustvr-runtime -p rustvr-range)")

    restarted = False
    if steam_running():
        if not args.restart_steam:
            sys.exit("Steam is running. Close it first, or pass --restart-steam (Steam would overwrite the file on exit).")
        print("closing Steam...")
        shutdown_steam(steam)
        restarted = True

    root = {"shortcuts": {}}
    if os.path.exists(path):
        data = open(path, "rb").read()
        root, _ = parse_map(data)
        shutil.copyfile(path, path + ".rustvr-backup")
    # Steam re-saves the key as "appname", so match either spelling.
    def name_of(entry):
        return next((v for k, v in entry.items() if k.lower() == "appname"), None)

    entries = [v for v in root.get("shortcuts", {}).values() if name_of(v) != args.name]
    if not args.remove:
        quoted = f'"{exe}"'
        entries.append({
            "appid": shortcut_appid(quoted, args.name),
            "AppName": args.name,
            "Exe": quoted,
            "StartDir": f'"{os.path.dirname(exe)}"',
            "icon": "",
            "ShortcutPath": "",
            "LaunchOptions": args.launch_options,
            "IsHidden": 0,
            "AllowDesktopConfig": 1,
            "AllowOverlay": 1,
            "OpenVR": 1,
            "Devkit": 0,
            "DevkitGameID": "",
            "DevkitOverrideAppID": 0,
            "LastPlayTime": 0,
            "FlatpakAppID": "",
            "sortas": "",
            "tags": {},
        })
    root["shortcuts"] = {str(i): e for i, e in enumerate(entries)}
    os.makedirs(os.path.dirname(path), exist_ok=True)
    # The top-level map's own terminator is the file's final 0x08.
    data = write_map(root)
    # Round-trip check before replacing Steam's file.
    assert parse_map(data) == (root, len(data))
    open(path, "wb").write(data)
    print(f"{'removed' if args.remove else 'added'} '{args.name}' for Steam account {persona} ({account}): {path}")

    if restarted:
        subprocess.Popen([os.path.join(steam, "steam.exe")], close_fds=True, creationflags=0x00000008)  # DETACHED_PROCESS
        print("started Steam again")


if __name__ == "__main__":
    main()
