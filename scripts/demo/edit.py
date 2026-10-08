#!/usr/bin/env python3
"""Cut the filmed demo into the README's animated WebP, or its showreel.

Takes what scripts/demo/film-macos.sh leaves (footage.mov, cues.tsv and
timing.tsv), trims it to the storyboard, shows each key chord as key caps
for a moment before its change lands, and rounds the window's corners with a
soft shadow. A .webp loops on a transparent ground, its end crossfaded into
its start so the loop has no seam; an .mp4 plays once on a dark ground.

Usage: scripts/demo/edit.py DEMO_DIR [--out target/demo/demo.webp] [--width N]

The README shows demo.webp from the repository's `media` pre-release and
links it to mux-showreel.mp4 there, so neither is committed. Cut both, then
replace them on the release: gh release upload media demo.webp
mux-showreel.mp4 --clobber.

Needs ffmpeg, Pillow and NumPy, and libwebp's img2webp when ffmpeg was built
without libwebp.
"""

import argparse
import csv
import itertools
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

REPO = Path(__file__).resolve().parents[2]
FONT = REPO / "apps/mux/assets/fonts/JetBrainsMonoNerdFontMono-Bold.ttf"

FPS = 30
# A chord's caps fade in this long before its change lands (the storyboard
# presses the chord a quarter second ahead), stay, then fade out.
CAP_LEAD = 0.05
CAP_HOLD = 1.0
CAP_FADE = 0.15
LOOP_FADE = 0.5
# The editor fills the pane this long after the storyboard's #editor mark.
EDITOR_LATENCY = 0.1


def key_caps(label: str, scale: float) -> Image.Image:
    """Key caps for a chord such as "⌃P R", in the window's own type."""
    font = ImageFont.truetype(str(FONT), round(25 * scale))
    pad_x, pad_y, gap, radius = round(14 * scale), round(9 * scale), round(8 * scale), round(8 * scale)
    keys = label.split()
    boxes = []
    for key in keys:
        left, top, right, bottom = font.getbbox(key)
        boxes.append((right - left, left))
    ascent, descent = font.getmetrics()
    cap_h = ascent + descent + 2 * pad_y
    widths = [max(w + 2 * pad_x, cap_h) for w, _ in boxes]
    shadow = round(10 * scale)
    width = sum(widths) + gap * (len(keys) - 1) + 2 * shadow
    height = cap_h + 2 * shadow
    image = Image.new("RGBA", (width, height), (0, 0, 0, 0))

    under = Image.new("RGBA", image.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(under)
    x = shadow
    for cap_w in widths:
        draw.rounded_rectangle((x, shadow + 2, x + cap_w, shadow + cap_h + 2), radius, fill=(0, 0, 0, 150))
        x += cap_w + gap
    image.alpha_composite(under.filter(ImageFilter.GaussianBlur(shadow / 2)))

    draw = ImageDraw.Draw(image)
    x = shadow
    for key, cap_w, (text_w, text_left) in zip(keys, widths, boxes, strict=True):
        box = (x, shadow, x + cap_w, shadow + cap_h)
        draw.rounded_rectangle(
            box, radius, fill=(30, 26, 23, 255), outline=(255, 255, 255, 46), width=max(1, round(scale))
        )
        draw.text((x + (cap_w - text_w) / 2 - text_left, shadow + pad_y), key, font=font, fill=(240, 233, 224, 255))
        x += cap_w + gap
    return image


def rounded_mask(size: tuple[int, int], radius: int) -> Image.Image:
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size[0] - 1, size[1] - 1), radius, fill=255)
    return mask


def drop_shadow(size: tuple[int, int], margin: int, radius: int) -> Image.Image:
    width, height = size[0] + 2 * margin, size[1] + 2 * margin
    shadow = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    drop = round(margin * 0.3)
    ImageDraw.Draw(shadow).rounded_rectangle(
        (margin, margin + drop, margin + size[0], margin + size[1] + drop), radius, fill=(0, 0, 0, 105)
    )
    return shadow.filter(ImageFilter.GaussianBlur(margin / 2.6))


def find_editor(footage: Path, around: float) -> float | None:
    """When the editor first fills the pane, in footage seconds, near AROUND.

    The opening pane is still but for typing; the editor drawing a screenful
    is the first frame in which much of the pane changes at once.
    """
    begin = max(0.0, around - 3)
    width = 320
    src_w, src_h = probe_size(footage)
    height = round(src_h * width / src_w)
    raw = subprocess.run(
        [
            "ffmpeg",
            "-loglevel",
            "error",
            "-ss",
            f"{begin:.3f}",
            "-t",
            "6",
            "-i",
            str(footage),
            "-vf",
            f"fps={FPS},scale={width}:{height},format=gray",
            "-f",
            "rawvideo",
            "-",
        ],
        check=True,
        capture_output=True,
    ).stdout
    frames = np.frombuffer(raw, np.uint8).reshape(-1, height, width).astype(np.int16)
    # Inside the one pane, clear of the strip and the ground around it.
    inside = frames[:, round(height * 0.15) : round(height * 0.9), round(width * 0.08) : round(width * 0.92)]
    changed = (np.abs(np.diff(inside, axis=0)) > 24).mean(axis=(1, 2))
    hits = np.flatnonzero(changed > 0.04)
    if hits.size == 0:
        return None
    return begin + (hits[0] + 1) / FPS


def probe_size(path: Path) -> tuple[int, int]:
    out = subprocess.run(
        [
            "ffprobe",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
            str(path),
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    width, height = (int(v) for v in out.split(",")[:2])
    return width, height


def has_encoder(name: str) -> bool:
    out = subprocess.run(["ffmpeg", "-hide_banner", "-encoders"], check=True, capture_output=True, text=True)
    return f" {name} " in out.stdout


def frame_durations(count: int) -> list[int]:
    """Whole milliseconds for each of COUNT frames that keep FPS on average."""
    edges = [round(i * 1000 / FPS) for i in range(count + 1)]
    return [b - a for a, b in itertools.pairwise(edges)]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("demo_dir", type=Path)
    parser.add_argument("--out", type=Path, default=REPO / "target/demo/demo.webp", help="a .webp or an .mp4")
    parser.add_argument("--width", type=int, default=1600, help="width of the window in the output")
    parser.add_argument("--radius", type=float, default=12, help="window corner radius, in points")
    parser.add_argument("--points", type=float, default=1120, help="window width in points (Mux opens at 1120)")
    parser.add_argument(
        "--shift", type=float, default=0, help="seconds to add to the storyboard's start, after syncing"
    )
    parser.add_argument("--quality", type=int, default=74)
    args = parser.parse_args()

    footage = args.demo_dir / "footage.mov"
    if not footage.exists():
        footage = args.demo_dir / "footage.mp4"
    timing = {row[0]: float(row[1]) for row in csv.reader(open(args.demo_dir / "timing.tsv"), delimiter="\t")}
    marks = [(float(t), label) for t, label in csv.reader(open(args.demo_dir / "cues.tsv"), delimiter="\t")]
    cues = [(t, label) for t, label in marks if not label.startswith("#")]

    # The clocks say roughly where the storyboard starts in the footage; the
    # editor appearing says exactly, since recording starts a little late.
    start = timing["story"] - timing["record"]
    editor = next((t for t, label in marks if label == "#editor"), None)
    if editor is not None:
        seen = find_editor(footage, start + editor + EDITOR_LATENCY)
        if seen is None:
            print("no sign of the editor opening; trusting the clocks")
        else:
            print(f"the storyboard starts {seen - EDITOR_LATENCY - editor - start:+.2f}s from the clocks")
            start = seen - EDITOR_LATENCY - editor
    start += args.shift
    length = timing["finish"] - timing["story"]
    src_w, src_h = probe_size(footage)
    width = args.width
    height = round(src_h * width / src_w / 2) * 2
    scale = width / args.points
    radius = round(args.radius * scale)
    margin = round(28 * scale)

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        rounded_mask((width, height), radius).save(tmp / "mask.png")
        drop_shadow((width, height), margin, radius).save(tmp / "shadow.png")
        inputs = [
            "-ss",
            f"{start:.3f}",
            "-t",
            f"{length:.3f}",
            "-i",
            str(footage),
            "-loop",
            "1",
            "-framerate",
            str(FPS),
            "-t",
            f"{length:.3f}",
            "-i",
            str(tmp / "mask.png"),
            "-loop",
            "1",
            "-framerate",
            str(FPS),
            "-t",
            f"{length:.3f}",
            "-i",
            str(tmp / "shadow.png"),
        ]
        graph = [f"[0:v]fps={FPS},scale={width}:{height}:flags=lanczos,format=rgba,setpts=PTS-STARTPTS[v0]"]
        caps = {}
        for label in dict.fromkeys(label for _, label in cues):
            path = tmp / f"cap{len(caps)}.png"
            key_caps(label, scale).save(path)
            caps[label] = path
        last = "v0"
        for index, (at, label) in enumerate(cues):
            stream = 3 + index
            inputs += ["-loop", "1", "-framerate", str(FPS), "-t", f"{length:.3f}", "-i", str(caps[label])]
            show, hide = at - CAP_LEAD, at - CAP_LEAD + CAP_HOLD
            graph.append(
                f"[{stream}:v]format=rgba,fade=t=in:st={show:.3f}:d={CAP_FADE}:alpha=1,"
                f"fade=t=out:st={hide:.3f}:d={CAP_FADE}:alpha=1[c{index}]"
            )
            graph.append(
                f"[{last}][c{index}]overlay=x=(W-w)/2:y=H-h-{round(18 * scale)}:"
                f"enable='between(t,{show:.3f},{hide + CAP_FADE:.3f})'[v{index + 1}]"
            )
            last = f"v{index + 1}"
        video = args.out.suffix == ".mp4"
        if video:
            graph.append(f"[{last}]null[looped]")
        else:
            # Crossfade the end into the opening, which is a still pane on a
            # moving ground, so the loop comes round without a cut.
            graph.append(f"[{last}]split[body][head]")
            graph.append(f"[head]trim=0:{LOOP_FADE},setpts=PTS-STARTPTS[headclip]")
            graph.append(
                f"[body][headclip]xfade=transition=fade:duration={LOOP_FADE}:offset={length - LOOP_FADE:.3f}[looped]"
            )
        graph.append("[1:v]format=gray[mask]")
        graph.append("[looped][mask]alphamerge[window]")
        if video:
            # A video has no transparency: the window sits on GitHub's dark.
            canvas = f"{width + 2 * margin}x{height + 2 * margin}"
            graph.append(f"color=c=0x0d1117:s={canvas}:r={FPS}:d={length:.3f}[ground]")
            graph.append("[ground][2:v]overlay=0:0:shortest=1[shadowed]")
            graph.append(f"[shadowed][window]overlay={margin}:{margin}:shortest=1,format=yuv420p[out]")
        else:
            graph.append(f"[2:v][window]overlay={margin}:{margin}:shortest=1,format=yuva420p[out]")
        args.out.parent.mkdir(parents=True, exist_ok=True)
        filtered = ["ffmpeg", "-loglevel", "error", "-y", *inputs, "-filter_complex", ";".join(graph), "-map", "[out]"]
        if video:
            if has_encoder("libx264"):
                codec = ["-c:v", "libx264", "-preset", "slow", "-crf", "18"]
            else:
                codec = ["-c:v", "h264_videotoolbox", "-b:v", "16M"]
            subprocess.run([*filtered, *codec, "-movflags", "+faststart", str(args.out)], check=True)
        elif has_encoder("libwebp_anim"):
            command = [
                *filtered,
                "-c:v",
                "libwebp_anim",
                "-lossless",
                "0",
                "-q:v",
                str(args.quality),
                "-compression_level",
                "6",
                "-loop",
                "0",
                str(args.out),
            ]
            subprocess.run(command, check=True)
        else:
            # Homebrew's ffmpeg has no libwebp: it composes the frames and
            # libwebp's own img2webp encodes them.
            frames = tmp / "frames"
            frames.mkdir()
            subprocess.run([*filtered, "-pix_fmt", "rgba", str(frames / "%05d.png")], check=True)
            command = ["img2webp", "-loop", "0", "-lossy", "-q", str(args.quality), "-m", "6"]
            paths = sorted(frames.glob("*.png"))
            for path, duration in zip(paths, frame_durations(len(paths)), strict=True):
                command += ["-d", str(duration), str(path)]
            subprocess.run([*command, "-o", str(args.out)], check=True, stdout=subprocess.DEVNULL)
    print(f"{args.out}: {args.out.stat().st_size / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
