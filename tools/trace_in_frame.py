"""Reads the runtime's in-frame trace (`RUSTVR_TRACE_IN_FRAME=<file>`): per headset frame, the right
controller, the gun the rig has now and where the game drew it, and whether it was moved.

    python tools/trace_in_frame.py captures/trace_in_frame.csv [from_s] [to_s]

Prints, over the frames given (display time, seconds; default all): how often the game's player was
moved and its gun paired, how far the rig's gun stays from the controller (it should hold still
relative to it), and how far the game's drawing of the gun was from the gun now (what moving it
corrects).
"""

import math
import sys


def main():
    rows = []
    for line in open(sys.argv[1]):
        f = line.strip().split(",")
        if len(f) < 16:
            continue
        num = lambda s: float(s) if s else None
        rows.append(
            {
                "t": float(f[0]),
                "ready": int(f[4]),
                "pairs": int(f[5]),
                "held": int(f[6]),
                "c": (float(f[7]), float(f[8]), float(f[9])),
                "now": tuple(num(x) for x in f[10:13]),
                "then": tuple(num(x) for x in f[13:16]),
            }
        )
    if len(sys.argv) > 3:
        lo, hi = float(sys.argv[2]), float(sys.argv[3])
        t0 = rows[0]["t"]
        rows = [r for r in rows if lo <= r["t"] - t0 <= hi]
    n = len(rows)
    print(f"{n} frames")
    if not n:
        return
    print(f"  moved: {sum(r['ready'] for r in rows)}; the gun paired: {sum(1 for r in rows if r['held'] > 0)}")
    offs = [tuple(r["now"][k] - r["c"][k] for k in range(3)) for r in rows if r["now"][0] is not None]
    if offs:
        m = [sum(o[k] for o in offs) / len(offs) for k in range(3)]
        spread = math.sqrt(sum(sum((o[k] - m[k]) ** 2 for k in range(3)) for o in offs) / len(offs))
        worst = max(math.sqrt(sum((o[k] - m[k]) ** 2 for k in range(3))) for o in offs)
        print(f"  the rig's gun from the controller: {m[0]:.3f},{m[1]:.3f},{m[2]:.3f} m on average; it wanders {spread * 100:.1f} cm RMS (most {worst * 100:.1f} cm)")
    corr = [math.dist(r["now"], r["then"]) for r in rows if r["now"][0] is not None and r["then"][0] is not None]
    if corr:
        corr.sort()
        print(f"  the game drew the gun {sum(corr) / len(corr) * 100:.1f} cm from where it is now on average (95% under {corr[int(len(corr) * 0.95)] * 100:.1f}, most {corr[-1] * 100:.1f})")


if __name__ == "__main__":
    main()
