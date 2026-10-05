"""Generate the test PDFs pdfit is measured on, into fixtures/out/.

Needs: pip install pymupdf pillow numpy

Everything is synthetic and seeded, so a run produces the same files on the
same machine. Fonts come from the system; the script looks in the usual
macOS and Linux places and stops with a message if it can't find any.
"""

import io
import math
import zlib
import os
import random
import sys

import numpy as np
import pymupdf
from PIL import Image, ImageDraw, ImageFilter, ImageFont

OUT = os.path.join(os.path.dirname(__file__), "..", "fixtures", "out")
A4 = (595, 842)
rng = np.random.default_rng(7)
random.seed(7)


def find(*paths):
    for p in paths:
        if os.path.exists(p):
            return p
    return None


SANS = find(
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
)
SERIF = find(
    "/System/Library/Fonts/Supplemental/Georgia.ttf",
    "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
)
BIG_FONTS = [
    f
    for f in [
        "/System/Library/Fonts/Supplemental/AppleMyungjo.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ]
    if os.path.exists(f)
]
if not SANS or not SERIF or not BIG_FONTS:
    sys.exit("Couldn't find the system fonts this script uses; edit the paths at the top.")

LETTER = [
    "Dear Ms Okafor,",
    "",
    "Thank you for your application for the residence permit, reference",
    "RP-2024-118-4471. We have received the documents listed below and",
    "checked them against the requirements of section 16(2).",
    "",
    "1. Passport, valid until 14 March 2031, number X4471 0093.",
    "2. Proof of address: tenancy agreement dated 2 September.",
    "3. Bank statements for the last three months.",
    "4. Employment contract signed on 19 August.",
    "",
    "Before we can make a decision we need a certified copy of your",
    "degree certificate and its translation. Please upload these within",
    "28 days of the date of this letter. Each file must be under 2 MB.",
    "",
    "If you have questions, call 0300 123 4471 between 9am and 4pm,",
    "quoting the reference above.",
    "",
    "Yours sincerely,",
]

PARAGRAPH = (
    "Quarterly output rose in all three plants, with the largest gain at the Leeds site, where the second "
    "line came back into service in May after the bearing replacement. Scrap rates fell from 4.1% to 3.2%, "
    "mostly from the new inspection step on incoming sheet. Energy use per tonne is flat; the gas contract "
    "renewal in October is the main risk for the next quarter. Staffing is at plan except for maintenance, "
    "where two posts are still open after the summer. The figures below are unaudited and may change by a "
    "few tenths of a percent when the month-end stock count is reconciled."
)


def jpeg_bytes(img, quality, **kw):
    buf = io.BytesIO()
    img.save(buf, "JPEG", quality=quality, **kw)
    return buf.getvalue()


def png_bytes(img):
    buf = io.BytesIO()
    img.save(buf, "PNG")
    return buf.getvalue()


def smooth_noise(h, w, scale, amount):
    small = rng.normal(0, 1, (max(2, h // scale), max(2, w // scale))).astype(np.float32)
    img = Image.fromarray(((small - small.min()) / (np.ptp(small) + 1e-6) * 255).astype(np.uint8))
    img = img.resize((w, h), Image.BICUBIC)
    return (np.asarray(img, dtype=np.float32) / 255.0 - 0.5) * amount


def phone_scan(lines, w=3024, h=4032, page_no=1):
    """A sheet of printed paper photographed on a desk: uneven light, paper
    grain, a slight tilt, JPEG at the quality phones use."""
    paper_w, paper_h = int(w * 0.9), int(w * 0.9 * 1.414)
    paper = np.full((paper_h, paper_w, 3), (243, 240, 232), np.float32)
    paper += smooth_noise(paper_h, paper_w, 40, 10)[..., None]
    paper += rng.normal(0, 3.0, (paper_h, paper_w, 1)).astype(np.float32)
    img = Image.fromarray(paper.clip(0, 255).astype(np.uint8))
    d = ImageDraw.Draw(img)
    big = ImageFont.truetype(SERIF, 92)
    body = ImageFont.truetype(SERIF, 58)
    small = ImageFont.truetype(SANS, 40)
    d.text((220, 200), "Department of Residence and Visas", font=big, fill=(30, 30, 35))
    d.text((220, 330), f"Our ref RP-2024-118-4471     Page {page_no} of 4", font=small, fill=(60, 60, 60))
    d.line((220, 400, paper_w - 220, 400), fill=(40, 40, 40), width=4)
    y = 520
    for line in lines:
        d.text((220, y), line, font=body, fill=(25, 25, 30))
        y += 84
    # A table with ruled lines, the kind of thing that breaks up when
    # downsampled badly.
    y += 40
    for r in range(6):
        d.line((220, y + r * 70, paper_w - 220, y + r * 70), fill=(60, 60, 60), width=3)
        d.text((240, y + r * 70 + 14), f"Item {r + 1}", font=small, fill=(30, 30, 30))
        d.text((paper_w // 2, y + r * 70 + 14), f"{(r * 37 + 11) % 97:>3} . {r * 13 % 10}0", font=small, fill=(30, 30, 30))
    # Blue ink signature and a red stamp.
    sx, sy = 260, y + 520
    pts = [(sx + t * 9, sy + 40 * math.sin(t / 4.0) + 25 * math.sin(t / 1.3)) for t in range(90)]
    d.line(pts, fill=(20, 40, 140), width=7, joint="curve")
    stamp = Image.new("RGBA", (520, 520), (0, 0, 0, 0))
    sd = ImageDraw.Draw(stamp)
    sd.ellipse((10, 10, 510, 510), outline=(180, 30, 40, 200), width=14)
    sd.text((95, 225), "RECEIVED", font=ImageFont.truetype(SANS, 76), fill=(180, 30, 40, 200))
    stamp = stamp.rotate(17, resample=Image.BICUBIC)
    img.paste(stamp, (paper_w - 900, y + 380), stamp)

    # Onto a desk, tilted, with light falling off towards one corner.
    desk = np.full((h, w, 3), (92, 74, 60), np.float32)
    desk += smooth_noise(h, w, 30, 30)[..., None]
    scene = Image.fromarray(desk.clip(0, 255).astype(np.uint8))
    tilted = img.rotate(0.8, resample=Image.BICUBIC, expand=True, fillcolor=(92, 74, 60))
    scene.paste(tilted, ((w - tilted.width) // 2, (h - tilted.height) // 2 + 60))
    arr = np.asarray(scene, dtype=np.float32)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    light = 1.0 - 0.18 * ((xx / w) * 0.6 + (yy / h) * 0.4)
    arr = arr * light[..., None] + rng.normal(0, 2.0, (h, w, 1)).astype(np.float32)
    out = Image.fromarray(arr.clip(0, 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(1.1))
    return jpeg_bytes(out, 92)


def photo(w, h, seed):
    """Landscape-ish photo: sky, hills, foliage texture, sensor noise."""
    r = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    sky = np.stack([140 + 60 * yy / h, 175 + 40 * yy / h, 230 - 20 * yy / h], -1)
    horizon = h * (0.55 + 0.08 * np.sin(xx / w * 6.3 + seed) + 0.03 * np.sin(xx / w * 23))
    ground = np.stack([60 + 30 * np.sin(xx / 37), 110 + 25 * np.cos(yy / 53), 50 + 0 * xx], -1)
    img = np.where((yy > horizon)[..., None], ground, sky)
    img += smooth_noise(h, w, 12, 70)[..., None] * (yy > horizon)[..., None]
    img += smooth_noise(h, w, 80, 30)[..., None]
    for _ in range(25):
        cx, cy, rad = r.uniform(0, w), r.uniform(h * 0.5, h), r.uniform(w * 0.01, w * 0.06)
        mask = ((xx - cx) ** 2 + (yy - cy) ** 2) < rad**2
        img[mask] = img[mask] * 0.6 + np.array(r.uniform(30, 200, 3)) * 0.4
    img += r.normal(0, 4, img.shape)
    return Image.fromarray(img.clip(0, 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(0.7))


def make_scan():
    doc = pymupdf.open()
    for p in range(4):
        lines = LETTER if p == 0 else [l.replace("Dear Ms Okafor,", "Continued") for l in LETTER[2:]]
        page = doc.new_page(width=A4[0], height=A4[1])
        page.insert_image(page.rect, stream=phone_scan(lines, page_no=p + 1))
    doc.save(f"{OUT}/scan.pdf")


def make_scan_png():
    """Flatbed scan at 300 dpi saved losslessly, gray: Flate with a PNG
    predictor in the PDF."""
    doc = pymupdf.open()
    for p in range(2):
        jpg = Image.open(io.BytesIO(phone_scan(LETTER, w=2480, h=3508, page_no=p + 1))).convert("L")
        page = doc.new_page(width=A4[0], height=A4[1])
        xref = page.insert_image(page.rect, stream=png_bytes(jpg))
        # pymupdf stores the samples without a predictor; many scanner
        # drivers use PNG "Up" filtering, so rewrite the stream that way.
        a = np.asarray(jpg, dtype=np.uint8)
        up = np.vstack([a[:1], (a[1:].astype(np.int16) - a[:-1]).astype(np.uint8)])
        rows = np.hstack([np.full((a.shape[0], 1), 2, np.uint8), up])
        doc.update_stream(xref, zlib.compress(rows.tobytes(), 9), compress=False)
        doc.xref_set_key(xref, "Filter", "/FlateDecode")
        doc.xref_set_key(xref, "DecodeParms", f"<</Predictor 15 /Colors 1 /BitsPerComponent 8 /Columns {a.shape[1]}>>")
    doc.save(f"{OUT}/scan-gray-png.pdf")


def chart(page, rect, seed):
    r = random.Random(seed)
    page.draw_rect(rect, color=(0.6, 0.6, 0.6), width=0.5)
    n = 8
    bw = rect.width / (n * 1.6)
    for i in range(n):
        v = r.uniform(0.2, 0.95)
        x0 = rect.x0 + 10 + i * bw * 1.5
        page.draw_rect(pymupdf.Rect(x0, rect.y1 - v * (rect.height - 20), x0 + bw, rect.y1 - 1),
                       color=None, fill=(0.15, 0.35, 0.6))
    pts = [pymupdf.Point(rect.x0 + 10 + i * rect.width / 12, rect.y0 + 20 + r.uniform(0, rect.height - 40)) for i in range(12)]
    for a, b in zip(pts, pts[1:]):
        page.draw_line(a, b, color=(0.8, 0.3, 0.1), width=1.2)


def make_report():
    doc = pymupdf.open()
    logo = Image.new("RGB", (600, 200), (255, 255, 255))
    ImageDraw.Draw(logo).text((20, 40), "NORTHFIELD", font=ImageFont.truetype(SANS, 110), fill=(20, 60, 110))
    logo_png = png_bytes(logo)
    for p in range(6):
        page = doc.new_page(width=A4[0], height=A4[1])
        page.insert_font(fontname="body", fontfile=SERIF)
        page.insert_font(fontname="head", fontfile=SANS)
        page.insert_image(pymupdf.Rect(50, 30, 200, 80), stream=logo_png)
        page.insert_text((50, 110), f"Operations report, section {p + 1}", fontname="head", fontsize=18)
        page.insert_textbox(pymupdf.Rect(50, 125, 545, 330), PARAGRAPH + " " + PARAGRAPH, fontname="body", fontsize=10)
        if p % 2 == 0:
            img = photo(3000, 2000, p)
            page.insert_image(pymupdf.Rect(50, 340, 545, 670), stream=jpeg_bytes(img, 90))
        else:
            chart(page, pymupdf.Rect(50, 340, 545, 600), p)
            page.insert_textbox(pymupdf.Rect(50, 610, 545, 800), PARAGRAPH, fontname="body", fontsize=10)
        page.insert_text((280, 820), f"{p + 1}", fontname="body", fontsize=9)
    doc.save(f"{OUT}/report.pdf", deflate=True)
    return doc


def signature_png(w=2400, h=800):
    sig = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(sig)
    pts = []
    for t in range(400):
        x = 120 + t * 5.2
        y = h / 2 + 160 * math.sin(t / 18.0) * math.exp(-t / 500) + 70 * math.sin(t / 5.5)
        pts.append((x, y))
    d.line(pts, fill=(18, 38, 120, 255), width=14, joint="curve")
    d.line([(300, 620), (1900, 600)], fill=(18, 38, 120, 255), width=10)
    return sig.filter(ImageFilter.GaussianBlur(1.5))


def make_form():
    doc = pymupdf.open()
    for p in range(2):
        page = doc.new_page(width=A4[0], height=A4[1])
        page.insert_font(fontname="f", fontfile=SANS)
        page.insert_text((50, 70), "Application for a residence permit" if p == 0 else "Declaration", fontname="f", fontsize=16)
        fields = ["Family name", "Given names", "Date of birth", "Passport number", "Address", "Email"]
        values = ["OKAFOR", "Adaeze Chioma", "1991-04-17", "X4471 0093", "14 Mill Lane, Leeds LS6 2AB", "a.okafor@example.org"]
        for i, (f, v) in enumerate(zip(fields, values)):
            y = 110 + i * 40
            page.insert_text((50, y), f, fontname="f", fontsize=9)
            page.draw_rect(pymupdf.Rect(180, y - 14, 545, y + 8), color=(0.4, 0.4, 0.4), width=0.6)
            page.insert_text((186, y), v, fontname="f", fontsize=11)
        if p == 0:
            portrait = photo(1200, 1600, 11)
            page.insert_image(pymupdf.Rect(420, 380, 520, 513), stream=jpeg_bytes(portrait, 93))
        page.insert_text((50, 620), "Signature", fontname="f", fontsize=9)
        page.insert_image(pymupdf.Rect(180, 590, 400, 663), stream=png_bytes(signature_png()))
    doc.save(f"{OUT}/form-signature.pdf", deflate=True)


def make_cmyk():
    doc = pymupdf.open()
    page = doc.new_page(width=A4[0], height=A4[1])
    img = photo(3000, 2000, 21).convert("CMYK")
    # Pillow writes CMYK JPEGs the Adobe way: inverted, with an APP14 marker.
    page.insert_image(pymupdf.Rect(50, 100, 545, 430), stream=jpeg_bytes(img, 92))
    page.insert_font(fontname="f", fontfile=SANS)
    page.insert_text((50, 80), "Print proof, CMYK", fontname="f", fontsize=14)
    doc.save(f"{OUT}/cmyk.pdf", deflate=True)


def make_smask():
    doc = pymupdf.open()
    page = doc.new_page(width=A4[0], height=A4[1])
    base = photo(2500, 2500, 31).convert("RGBA")
    yy, xx = np.mgrid[0:2500, 0:2500]
    alpha = (255 * np.clip((1100 - np.hypot(xx - 1250, yy - 1250)) / 120, 0, 1)).astype(np.uint8)
    base.putalpha(Image.fromarray(alpha))
    page.draw_rect(pymupdf.Rect(40, 90, 555, 600), color=None, fill=(0.93, 0.88, 0.75))
    page.insert_image(pymupdf.Rect(70, 120, 525, 575), stream=png_bytes(base))
    page.insert_font(fontname="f", fontfile=SANS)
    page.insert_text((50, 70), "Product cut-out with soft edges", fontname="f", fontsize=14)
    doc.save(f"{OUT}/smask.pdf", deflate=True)


def make_fonts():
    """A bilingual notice with a full Korean font embedded: the fonts alone
    are bigger than a 2 MB limit."""
    doc = pymupdf.open()
    page = doc.new_page(width=A4[0], height=A4[1])
    page.insert_font(fontname="cjk", fontfile=BIG_FONTS[0])
    page.insert_text((50, 80), "Notice to applicants", fontname="cjk", fontsize=16)
    page.insert_text((50, 110), "신청자 안내", fontname="cjk", fontsize=16)
    page.insert_textbox(pymupdf.Rect(50, 130, 545, 400), PARAGRAPH, fontname="cjk", fontsize=10)
    page.insert_image(pymupdf.Rect(50, 420, 300, 587), stream=jpeg_bytes(photo(1500, 1000, 41), 90))
    doc.save(f"{OUT}/fonts-heavy.pdf", deflate=True)


def make_encrypted(report):
    report.save(
        f"{OUT}/encrypted.pdf",
        encryption=pymupdf.PDF_ENCRYPT_AES_256,
        owner_pw="owner",
        user_pw="secret",
        deflate=True,
    )


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    make_scan()
    make_scan_png()
    report = make_report()
    make_form()
    make_cmyk()
    make_smask()
    make_fonts()
    make_encrypted(report)
    for f in sorted(os.listdir(OUT)):
        print(f"{f:24} {os.path.getsize(os.path.join(OUT, f)) / 1e6:6.2f} MB")
