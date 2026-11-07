"""Where is the side-panel thumb?

Crops the "文件 / 大纲" strip out of a window screenshot and reports the raised
slab's extent, so a slide can be measured frame by frame instead of eyeballed.

The slab is painted in the page colour and the track behind it in a slightly
darker tint, so the trick is to find the long run of pixels that is *not* the
track tint. Label glyphs are darker still and read as holes, so the run is
allowed to be interrupted — what matters is its two ends.
"""
import subprocess, sys, os

FF = r"C:\Users\22357\AppData\Local\Programs\recordly\resources\app.asar.unpacked\node_modules\ffmpeg-static\ffmpeg.exe"
X, Y, W, H = 0, 60, 240, 40
LO, HI = 16, 219          # track interior, clear of its rounded corners
CELL = 105.0              # half of the 210px track interior


def rows(img):
    raw = subprocess.run([FF, "-v", "quiet", "-i", img, "-vf",
                          f"crop={W}:{H}:{X}:{Y}", "-f", "rawvideo",
                          "-pix_fmt", "rgb24", "-"], capture_output=True).stdout
    return {y: raw[(y - Y) * W * 3:(y - Y) * W * 3 + W * 3] for y in range(Y, Y + H)}


def slab(img):
    best = None
    for y, band in rows(img).items():
        vals = [band[i * 3] for i in range(LO, HI)]
        grey = sum(1 for v in vals if 243 <= v <= 247)
        if not 40 < grey < 180:
            continue                      # a row that misses the track band
        mask = [not (243 <= v <= 247) for v in vals]
        run, start = None, 0
        runs = []
        for i, on in enumerate(mask + [False]):
            if on and run is None:
                run = i
            elif not on and run is not None:
                runs.append((run, i - 1))
                run = None
        runs = [(a, b) for a, b in runs if 80 <= b - a <= 130]
        if not runs:
            continue
        a, b = max(runs, key=lambda r: r[1] - r[0])
        if best is None or b - a > best[1] - best[0]:
            best = (LO + a, LO + b)
    return best


for name in sys.argv[1:]:
    span = slab(os.path.abspath(name))
    if not span:
        print(f"{os.path.basename(name):>10}  no slab found")
        continue
    lo, hi = span
    print(f"{os.path.basename(name):>10}  slab {lo:>3}..{hi:>3}  "
          f"width {hi - lo:>3}  pos {(lo - 11) / CELL:5.2f}")
