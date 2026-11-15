"""Compare a presentation image with the native image from the same replay.

An anchor is the presentation sample at (s*x, s*y), which samples the same point as native pixel
(x, y). RGB alone cannot distinguish decision changes from value errors. Optional decision
labels are one byte per pixel at each image's resolution (255 means unknown). They must come
from a fixture oracle or diagnostic output, not from an RGB error threshold. Value errors are
counted only where known labels agree. Supply the rule's output-space step with --step or a
native-size RGB8 --steps map; the default of 1 only screens for byte rounding differences.
Tone compares blurred native RGB with blurred s x s block averages, split by native contrast.
"""

import argparse
import json
from pathlib import Path

KERNEL = (1, 4, 6, 4, 1)


def blur(image, width, height):
    def axis(src, dx, dy):
        out = []
        for y in range(height):
            for x in range(width):
                acc = [0.0, 0.0, 0.0]
                for k, w in enumerate(KERNEL):
                    sx = min(max(x + (k - 2) * dx, 0), width - 1)
                    sy = min(max(y + (k - 2) * dy, 0), height - 1)
                    p = src[sy * width + sx]
                    for c in range(3):
                        acc[c] += w * p[c]
                out.append([v / 16 for v in acc])
        return out

    return axis(axis(image, 1, 0), 0, 1)


def percentile(values, fraction):
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(fraction * len(ordered)))]


def compare(native, present, width, height, scale, step=1, decisions=None, steps=None):
    assert width > 0 and height > 0 and scale > 0 and step >= 0
    assert len(native) == width * height * 4
    assert len(present) == width * height * scale * scale * 4
    if decisions is not None:
        assert len(decisions[0]) == width * height
        assert len(decisions[1]) == width * height * scale * scale
    if steps is not None:
        assert len(steps) == width * height * 3
    pw = width * scale
    pixel = lambda data, i: data[i * 4:i * 4 + 3]
    bands = {"identical": 0, "within_step": 0, "beyond_step": 0}
    decision_mismatches = value_beyond_step = value_tested = unclassified = 0
    beyond = []
    anchor_max = 0
    boxes = []
    for y in range(height):
        for x in range(width):
            ni = y * width + x
            ai = y * scale * pw + x * scale
            n = pixel(native, ni)
            a = pixel(present, ai)
            differences = [abs(n[c] - a[c]) for c in range(3)]
            limits = steps[ni * 3:ni * 3 + 3] if steps is not None else [step] * 3
            exceeds = any(d > limit for d, limit in zip(differences, limits))
            d = max(differences)
            anchor_max = max(anchor_max, d)
            band = "identical" if d == 0 else "beyond_step" if exceeds else "within_step"
            bands[band] += 1
            beyond.append(exceeds)
            if decisions is None or decisions[0][ni] == 255 or decisions[1][ai] == 255:
                unclassified += 1
            elif decisions[0][ni] != decisions[1][ai]:
                decision_mismatches += 1
            else:
                value_tested += 1
                value_beyond_step += exceeds
            sums = [0, 0, 0]
            for j in range(scale):
                for i in range(scale):
                    p = pixel(present, (y * scale + j) * pw + x * scale + i)
                    for c in range(3):
                        sums[c] += p[c]
            boxes.append([v / (scale * scale) for v in sums])
    natives = [list(pixel(native, i)) for i in range(width * height)]
    blurred_box = blur(boxes, width, height)
    blurred_native = blur(natives, width, height)
    tone = {"flat": [], "detail": []}
    for y in range(height):
        for x in range(width):
            i = y * width + x
            n = natives[i]
            contrast = max(
                abs(n[c] - natives[min(max(y + dy, 0), height - 1) * width + min(max(x + dx, 0), width - 1)][c])
                for dx in (-1, 0, 1) for dy in (-1, 0, 1) for c in range(3)
            )
            error = max(abs(blurred_box[i][c] - blurred_native[i][c]) for c in range(3))
            tone["flat" if contrast <= 8 else "detail"].append(error)
    report = {
        "scale": scale,
        "pixels": width * height,
        "anchors": bands,
        "anchor_max": anchor_max,
        "step_rgb8": "map" if steps is not None else step,
        "decision_mismatches": decision_mismatches if decisions is not None else None,
        "value_beyond_step": value_beyond_step if decisions is not None else None,
        "value_tested": value_tested,
        "unclassified": unclassified,
    }
    for region, errors in tone.items():
        report[f"tone_{region}"] = {
            "pixels": len(errors),
            "mean": round(sum(errors) / len(errors), 3) if errors else 0.0,
            "p99": round(percentile(errors, 0.99), 3),
        }
    return report, beyond


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("native", type=Path)
    parser.add_argument("presentation", type=Path)
    parser.add_argument("--width", type=int, required=True)
    parser.add_argument("--height", type=int, required=True)
    parser.add_argument("--scale", type=int, required=True)
    parser.add_argument("--step", type=int, default=1, help="rule's output-space step in RGB8 units")
    parser.add_argument("--steps", type=Path, help="per-anchor, per-channel step bounds as native-size RGB8")
    parser.add_argument("--decisions", type=Path, nargs=2, metavar=("NATIVE", "PRESENTATION"),
                        help="decision labels as raw u8, one per pixel; 255 means unknown")
    parser.add_argument("--mask", type=Path, help="write beyond-step anchors as a native RGBA8 mask")
    args = parser.parse_args()
    report, beyond = compare(args.native.read_bytes(), args.presentation.read_bytes(),
                             args.width, args.height, args.scale, args.step,
                             [p.read_bytes() for p in args.decisions] if args.decisions else None,
                             args.steps.read_bytes() if args.steps else None)
    if args.mask:
        args.mask.write_bytes(b"".join(b"\xff\xff\xff\xff" if b else b"\0\0\0\xff" for b in beyond))
    print(json.dumps(report))
