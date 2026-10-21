"""Saves a screenshot of a process's main window (no input, no focus change):

    python tools/window_shot.py <exe name> <out.png>

Grabs the screen region the window covers, so the window has to be visible on screen.
"""
import ctypes
import ctypes.wintypes as w
import subprocess
import sys

from PIL import ImageGrab


def pids(name):
    out = subprocess.check_output(["tasklist", "/FI", f"IMAGENAME eq {name}", "/FO", "CSV", "/NH"]).decode(errors="replace")
    return {int(line.split(",")[1].strip('"')) for line in out.splitlines() if line.startswith('"')}


def main_window(pid_set):
    user32 = ctypes.windll.user32
    found = []

    @ctypes.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    def cb(hwnd, _):
        pid = w.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if pid.value in pid_set and user32.IsWindowVisible(hwnd):
            r = w.RECT()
            user32.GetWindowRect(hwnd, ctypes.byref(r))
            area = (r.right - r.left) * (r.bottom - r.top)
            if area > 0:
                found.append((area, (r.left, r.top, r.right, r.bottom)))
        return True

    user32.EnumWindows(cb, 0)
    return max(found)[1] if found else None


def main():
    ctypes.windll.user32.SetProcessDPIAware()
    name, out = sys.argv[1], sys.argv[2]
    rect = main_window(pids(name))
    if rect is None:
        sys.exit(f"no visible window for {name}")
    ImageGrab.grab(bbox=rect, all_screens=True).save(out)
    print(out, rect)


if __name__ == "__main__":
    main()
