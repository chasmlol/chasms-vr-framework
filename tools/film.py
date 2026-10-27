"""Judges motion in a film from the emulated headset (crates/xrsim, `[[film]]` in a script): where
the player's gun is drawn in each frame against where the controller holding it is.

Run Metro with `debug_player_tint = true` in rustvr-metro.toml (the body drawn flat green, what it
holds flat magenta), film it moving (tools/xrsim/metro_motion.toml), then:

    python tools/film.py captures/xrsim_motion/films/sweep [--hand right] [--eye left] [--frames]

For each frame: the gun's centre on the screen (its magenta pixels) and the controller's grip
projected through the very eye pose and field of view that frame was drawn with. Then:

* lag: the time shift that best lines the gun's track up with the controller's (ms; the gun drawn
  where the hand was this long ago);
* judder: how far the gun's frame-to-frame screen motion strays from the controller's (pixels, RMS
  and worst), and how many frames it didn't move while the controller did (a repeated frame);
* `--frames`: every frame's numbers;
* a strip of crops round the controller (`strip.png`) to look at.
"""

import csv
import math
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def qrot(q, v):
    """Rotates v by the unit quaternion q (x, y, z, w)."""
    x, y, z, w = q
    vx, vy, vz = v
    tx, ty, tz = 2 * (y * vz - z * vy), 2 * (z * vx - x * vz), 2 * (x * vy - y * vx)
    return (vx + w * tx + (y * tz - z * ty), vy + w * ty + (z * tx - x * tz), vz + w * tz + (x * ty - y * tx))


def project(p, eye_pos, eye_q, fov, w, h):
    """OpenXR stage point -> pixel in an eye's image drawn from eye_pos/eye_q with fov (radians)."""
    inv = (-eye_q[0], -eye_q[1], -eye_q[2], eye_q[3])
    v = qrot(inv, (p[0] - eye_pos[0], p[1] - eye_pos[1], p[2] - eye_pos[2]))
    if v[2] >= -1e-6:
        return None
    tx, ty = v[0] / -v[2], v[1] / -v[2]
    l, r, u, d = (math.tan(a) for a in fov)
    return ((tx - l) / (r - l) * w, (u - ty) / (u - d) * h)


def main():
    args = sys.argv[1:]
    film = Path(args[0])
    hand = "rh" if "--hand" not in args or args[args.index("--hand") + 1] == "right" else "lh"
    eye = "l" if "--eye" not in args or args[args.index("--eye") + 1] == "left" else "r"
    rows = list(csv.DictReader(open(film / "frames.csv")))
    out = []
    crops = []
    for k, row in enumerate(rows):
        f = lambda n: float(row[n])
        # The eye's whole image (in the film's pixels), and where the film's part of it starts.
        w, h = int(row["img_w"]), int(row["img_h"])
        cx, cy = int(row.get("crop_x") or 0), int(row.get("crop_y") or 0)
        img = Image.open(film / f"{k:03d}.png").convert("RGB")
        a = np.asarray(img).astype(np.int32)
        iw = img.width // 2
        x0 = 0 if eye == "l" else iw
        e_img = a[:, x0 : x0 + iw]
        r, g, b = e_img[..., 0], e_img[..., 1], e_img[..., 2]
        m = (r > 90) & (b > 90) & (g < 60) & (np.abs(r - b) < 70)
        ys, xs = np.nonzero(m)
        grip = (f(f"{hand}_px"), f(f"{hand}_py"), f(f"{hand}_pz"))
        e = project(grip, (f(f"{eye}_px"), f(f"{eye}_py"), f(f"{eye}_pz")), (f(f"{eye}_qx"), f(f"{eye}_qy"), f(f"{eye}_qz"), f(f"{eye}_qw")), (f(f"{eye}_fl"), f(f"{eye}_fr"), f(f"{eye}_fu"), f(f"{eye}_fd")), w, h)
        e = (e[0] - cx, e[1] - cy) if e else None
        c = (xs.mean(), ys.mean()) if len(xs) > 40 else None
        out.append((f("t"), len(xs), c, e))
        if e:
            cx, cy = int(e[0]), int(e[1])  # (in the film's part now)
            box = (x0 + max(0, cx - 110), max(0, cy - 150), x0 + min(iw, cx + 110), min(img.height, cy + 70))
            if box[2] > box[0] and box[3] > box[1]:
                crops.append(img.crop(box).resize((110, 110)))
    good = [(t, c, e) for t, n, c, e in out if c and e]
    print(f"{film.name}: {len(rows)} frames over {out[-1][0] - out[0][0]:.3f} s, the gun found in {len(good)}")
    if "--frames" in args:
        prev = None
        for t, n, c, e in out:
            if c and e:
                d = (c[0] - e[0], c[1] - e[1])
                step = (c[0] - prev[0], c[1] - prev[1]) if prev else (0, 0)
                print(f"  t {t:.4f}  gun {c[0]:7.1f},{c[1]:7.1f}  hand {e[0]:7.1f},{e[1]:7.1f}  gun-hand {d[0]:6.1f},{d[1]:6.1f}  step {step[0]:5.1f},{step[1]:5.1f}  ({n} px)")
                prev = c
    if len(good) < 10:
        return
    ts = [g[0] for g in good]
    cs = [g[1] for g in good]
    es = [g[2] for g in good]

    def hand_at(t):
        if t <= ts[0]:
            return es[0]
        for i in range(1, len(ts)):
            if ts[i] >= t:
                a = (t - ts[i - 1]) / max(ts[i] - ts[i - 1], 1e-9)
                return (es[i - 1][0] + a * (es[i][0] - es[i - 1][0]), es[i - 1][1] + a * (es[i][1] - es[i - 1][1]))
        return es[-1]

    best = None
    for lag_ms in range(-40, 121, 2):
        lag = lag_ms / 1000.0
        d = [(c[0] - hand_at(t - lag)[0], c[1] - hand_at(t - lag)[1]) for t, c in zip(ts, cs) if t - lag >= ts[0]]
        if len(d) < 10:
            continue
        mx, my = sum(x for x, _ in d) / len(d), sum(y for _, y in d) / len(d)
        rms = math.sqrt(sum((x - mx) ** 2 + (y - my) ** 2 for x, y in d) / len(d))
        if best is None or rms < best[1]:
            best = (lag_ms, rms)
    steps = []
    still = 0
    for i in range(1, len(good)):
        gc = (cs[i][0] - cs[i - 1][0], cs[i][1] - cs[i - 1][1])
        he = (es[i][0] - es[i - 1][0], es[i][1] - es[i - 1][1])
        steps.append(math.hypot(gc[0] - he[0], gc[1] - he[1]))
        if math.hypot(*gc) < 0.3 and math.hypot(*he) > 1.5:
            still += 1
    steps.sort()
    # Smoothness: how much the gun's own screen motion changes from one frame to the next (its
    # second difference), against the hand's: a smooth sweep changes little; judder jumps.
    def accel(track):
        return [math.hypot(track[i + 1][0] - 2 * track[i][0] + track[i - 1][0], track[i + 1][1] - 2 * track[i][1] + track[i - 1][1]) for i in range(1, len(track) - 1)]
    ga, ha = accel(cs), accel(es)
    rms = lambda v: math.sqrt(sum(x * x for x in v) / max(len(v), 1))
    print(f"  smoothness: the gun's frame-to-frame change of motion {rms(ga):.2f} px RMS (worst {max(ga):.1f}); the hand's {rms(ha):.2f} px RMS (worst {max(ha):.1f})")
    hand_speed = sum(math.hypot(es[i][0] - es[i - 1][0], es[i][1] - es[i - 1][1]) for i in range(1, len(es))) / (len(es) - 1)
    print(f"  the hand moves {hand_speed:.1f} px a frame on the screen ({w} px across)")
    print(f"  lag: the gun is drawn where the hand was {best[0]} ms ago (left over {best[1]:.1f} px RMS)")
    print(f"  judder: the gun's step from frame to frame strays {math.sqrt(sum(s * s for s in steps) / len(steps)):.2f} px RMS from the hand's (worst {steps[-1]:.1f}, 95% under {steps[int(len(steps) * 0.95)]:.1f}); {still} frames the gun stood still while the hand moved")
    if crops:
        cols = 20
        rows_n = (len(crops) + cols - 1) // cols
        sheet = Image.new("RGB", (cols * 110, rows_n * 110))
        for i, c in enumerate(crops):
            sheet.paste(c, ((i % cols) * 110, (i // cols) * 110))
        sheet.save(film / "strip.png")
        print(f"  {film / 'strip.png'}")


if __name__ == "__main__":
    main()
