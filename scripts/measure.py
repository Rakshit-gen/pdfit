"""Run pdfit on every fixture, validate each result and measure it.

Needs: pip install pymupdf pikepdf numpy, a release build of pdfit
(cargo build --release), and the fixtures from make_fixtures.py.

For each fixture and limit it records the size before and after, the
median time of three runs, whether qpdf (through pikepdf) and mupdf accept
the result, whether the extracted text is identical, and PSNR and SSIM of
every recompressed image's area on the page, rendered at 150 dpi, against
the same area of the original.
"""

import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time

import numpy as np
import pikepdf
import pymupdf

ROOT = os.path.join(os.path.dirname(__file__), "..")
FIX = os.path.join(ROOT, "fixtures", "out")
BIN = os.path.join(ROOT, "target", "release", "pdfit")
DPI = 150
CASES = [
    ("scan.pdf", ["2MB", "1MB", "500KB"]),
    ("scan-gray-png.pdf", ["2MB", "1MB"]),
    ("report.pdf", ["2MB", "1MB", "500KB"]),
    ("form-signature.pdf", ["500KB", "300KB"]),
    ("cmyk.pdf", ["2MB", "1MB"]),
    ("smask.pdf", ["2MB", "1MB"]),
    ("fonts-heavy.pdf", ["2MB"]),
    ("encrypted.pdf", ["2MB"]),
]

pymupdf.TOOLS.set_icc(True)


def render(page):
    pix = page.get_pixmap(dpi=DPI, colorspace=pymupdf.csRGB, alpha=False)
    return np.frombuffer(pix.samples, np.uint8).reshape(pix.height, pix.width, 3).astype(np.float64)


def gray(a):
    return a @ np.array([0.299, 0.587, 0.114])


def psnr(a, b):
    mse = np.mean((a - b) ** 2)
    return float("inf") if mse == 0 else 10 * np.log10(255.0**2 / mse)


def box(a, k=7):
    """Mean over k x k windows, valid region only, via an integral image."""
    c = np.pad(a, ((1, 0), (1, 0))).cumsum(0).cumsum(1)
    return (c[k:, k:] - c[:-k, k:] - c[k:, :-k] + c[:-k, :-k]) / (k * k)


def ssim(a, b):
    """SSIM on luma with a 7 x 7 uniform window (Wang et al. 2004 constants)."""
    x, y = gray(a), gray(b)
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    mx, my = box(x), box(y)
    vx, vy = box(x * x) - mx * mx, box(y * y) - my * my
    cov = box(x * y) - mx * my
    s = ((2 * mx * my + c1) * (2 * cov + c2)) / ((mx * mx + my * my + c1) * (vx + vy + c2))
    return float(s.mean())


def image_areas(doc):
    """(page index, clip rect) for every image placement in the document."""
    out = []
    for i, page in enumerate(doc):
        for img in page.get_images(full=True):
            for r in page.get_image_rects(img[0]):
                r = r & page.rect
                if r.width > 20 and r.height > 20:
                    out.append((i, r))
    return out


def crop(arr, r):
    s = DPI / 72
    return arr[int(r.y0 * s) : int(r.y1 * s), int(r.x0 * s) : int(r.x1 * s)]


def compare(orig_path, out_path):
    a, b = pymupdf.open(orig_path), pymupdf.open(out_path)
    assert len(a) == len(b), "page count changed"
    text_same = all(a[i].get_text() == b[i].get_text() for i in range(len(a)))
    scores = []
    renders = {}
    for i, r in image_areas(a):
        if i not in renders:
            renders[i] = (render(a[i]), render(b[i]))
        x, y = crop(renders[i][0], r), crop(renders[i][1], r)
        h, w = min(x.shape[0], y.shape[0]), min(x.shape[1], y.shape[1])
        if h > 10 and w > 10:
            scores.append((psnr(x[:h, :w], y[:h, :w]), ssim(x[:h, :w], y[:h, :w])))
    return text_same, scores


def validate(path):
    problems = []
    with pikepdf.open(path) as pdf:
        problems += [str(p) for p in pdf.check_pdf_syntax()]
    pymupdf.TOOLS.mupdf_warnings(reset=True)
    doc = pymupdf.open(path)
    for page in doc:
        page.get_pixmap(dpi=36)
    warnings = pymupdf.TOOLS.mupdf_warnings()
    if warnings:
        problems.append("mupdf: " + warnings.replace("\n", "; "))
    return problems


def run(src, limit, out):
    times, proc = [], None
    for _ in range(3):
        t = time.perf_counter()
        proc = subprocess.run([BIN, src, "--under", limit, "-o", out, "--json"], capture_output=True, text=True)
        times.append(time.perf_counter() - t)
    return proc, statistics.median(times)


def main():
    if not os.path.exists(BIN):
        sys.exit("Build first: cargo build --release")
    rows = []
    tmp = tempfile.mkdtemp(prefix="pdfit-measure-")
    for name, limits in CASES:
        src = os.path.join(FIX, name)
        for limit in limits:
            out = os.path.join(tmp, f"{name[:-4]}-{limit}.pdf")
            proc, secs = run(src, limit, out)
            row = {"file": name, "limit": limit, "before": os.path.getsize(src), "seconds": round(secs, 2)}
            if proc.returncode == 1:
                row["error"] = proc.stderr.strip()
                rows.append(row)
                print(name, limit, "refused:", row["error"][:80])
                continue
            report = json.loads(proc.stdout)
            row.update(
                after=report["output_bytes"],
                fits=report["fits"],
                setting=report["setting"],
                reason=report["reason"],
                problems=validate(out),
            )
            text_same, scores = compare(src, out)
            row["text_same"] = text_same
            # Only images pdfit changed can differ; skip the rest rather than report infinity.
            if scores and any(i["after"] for i in report["images"]):
                row["psnr_min"] = round(min(s[0] for s in scores), 1)
                row["ssim_min"] = round(min(s[1] for s in scores), 3)
            rows.append(row)
            print(
                f"{name:20} {limit:>6} {row['before'] / 1e6:6.2f} -> {row['after'] / 1e6:6.2f} MB "
                f"fits={row['fits']} {secs:5.2f}s text_same={text_same} "
                f"psnr_min={row.get('psnr_min')} ssim_min={row.get('ssim_min')} problems={row['problems']}"
            )
    machine = f"{platform.machine()} {platform.mac_ver()[0] or platform.release()}"
    with open(os.path.join(ROOT, "fixtures", "results.json"), "w") as f:
        json.dump({"machine": machine, "dpi": DPI, "rows": rows}, f, indent=1)
    print(f"wrote fixtures/results.json ({machine})")


if __name__ == "__main__":
    main()
