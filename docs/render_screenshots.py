#!/usr/bin/env python3
"""Turn portwatch's screen dumps into the PNG screenshots used by the README.

    PORTWATCH_SHOTS=/tmp/shots cargo test dump_screenshots -- --ignored
    python3 docs/render_screenshots.py /tmp/shots docs/screenshots

The dumps come from the real renderer with made-up demo data (see `demo_app`
in src/ui.rs), so no real programs, paths or accounts appear. Needs Pillow and
the DejaVu Sans Mono font.
"""
import getpass
import json
import socket
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

FONT_DIR = Path("/usr/share/fonts/truetype/dejavu")
SCALE = 2  # draw at 2x so text stays sharp on high-DPI screens
SIZE = 13 * SCALE
DEFAULT_FG = (0xD0, 0xCF, 0xCC)
DEFAULT_BG = (0x30, 0x0A, 0x24)
FRAME_BG = (0x2B, 0x08, 0x20)

regular = ImageFont.truetype(str(FONT_DIR / "DejaVuSansMono.ttf"), SIZE)
bold = ImageFont.truetype(str(FONT_DIR / "DejaVuSansMono-Bold.ttf"), SIZE)
CELL_W = round(regular.getlength("M"))
ascent, descent = regular.getmetrics()
CELL_H = ascent + descent + 2 * SCALE


def rgb(css, fallback):
    if css == "default":
        return fallback
    return tuple(int(css[i : i + 2], 16) for i in (1, 3, 5))


def mix(a, b, t):
    return tuple(round(a[i] * t + b[i] * (1 - t)) for i in range(3))


def draw_screen(doc):
    w, h, cells = doc["width"], doc["height"], doc["cells"]
    img = Image.new("RGB", (w * CELL_W, h * CELL_H), DEFAULT_BG)
    d = ImageDraw.Draw(img)
    for i, (symbol, fg_css, bg_css, flags) in enumerate(cells):
        x, y = (i % w) * CELL_W, (i // w) * CELL_H
        fg, bg = rgb(fg_css, DEFAULT_FG), rgb(bg_css, DEFAULT_BG)
        if flags & 8:  # reversed
            fg, bg = bg, fg
        if bg != DEFAULT_BG:
            d.rectangle([x, y, x + CELL_W - 1, y + CELL_H - 1], fill=bg)
        if flags & 2:  # dim
            fg = mix(fg, bg, 0.6)
        if len(symbol) == 1 and 0x2800 <= ord(symbol) <= 0x28FF:
            # DejaVu Sans Mono has no Braille glyphs, so draw the 2x4 dots.
            bits, size = ord(symbol) - 0x2800, max(2, CELL_W // 3)
            for bit, (col, row) in enumerate([(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)]):
                if bits >> bit & 1:
                    cx = x + round(CELL_W * (0.30 + 0.40 * col))
                    cy = y + round(CELL_H * (0.125 + 0.25 * row))
                    d.rectangle([cx - size // 2, cy - size // 2, cx + size // 2 - 1, cy + size // 2 - 1], fill=fg)
        elif symbol.strip():
            d.text((x, y + SCALE), symbol, font=bold if flags & 1 else regular, fill=fg)
        if flags & 4:  # struck through
            d.line([x, y + CELL_H // 2, x + CELL_W, y + CELL_H // 2], fill=fg, width=SCALE)
    return img


def window(screen, title):
    """Put the screen in a simple terminal window frame."""
    pad, bar = 14 * SCALE, 34 * SCALE
    w, h = screen.width + 2 * pad, screen.height + bar + pad
    frame = Image.new("RGB", (w, h), FRAME_BG)
    d = ImageDraw.Draw(frame)
    for n, colour in enumerate([(0xFF, 0x5F, 0x57), (0xFE, 0xBC, 0x2E), (0x28, 0xC8, 0x40)]):
        cx, cy, r = pad + 8 * SCALE + n * 22 * SCALE, bar // 2, 6 * SCALE
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=colour)
    title_font = ImageFont.truetype(str(FONT_DIR / "DejaVuSansMono.ttf"), 11 * SCALE)
    tw = d.textlength(title, font=title_font)
    d.text(((w - tw) / 2, bar // 2 - 7 * SCALE), title, font=title_font, fill=(0xA0, 0x9A, 0x9C))
    frame.paste(screen, (pad, bar))
    # Rounded corners on a dark canvas that works on light and dark pages.
    mask = Image.new("L", frame.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, w - 1, h - 1], radius=10 * SCALE, fill=255)
    out = Image.new("RGBA", frame.size, (0, 0, 0, 0))
    out.paste(frame, (0, 0), mask)
    return out


def swap_real_user_for_demo_name(doc):
    """The Inspector reads the real account name from /proc. Swap it for the
    demo name, padded so the columns stay lined up."""
    real, demo = getpass.getuser(), "alice"
    w, cells = doc["width"], doc["cells"]
    for r in range(doc["height"]):
        start = r * w
        row = "".join(c[0] for c in cells[start : start + w])
        i = row.find(real)
        while i != -1:
            for k, ch in enumerate(demo.ljust(len(real))):
                cells[start + i + k][0] = ch
            row = "".join(c[0] for c in cells[start : start + w])
            i = row.find(real)


def check_no_personal_text(name, doc):
    forbidden = {"abdul", "moiz", socket.gethostname().lower()}
    w = doc["width"]
    rows = ["".join(c[0] for c in doc["cells"][r * w : (r + 1) * w]) for r in range(doc["height"])]
    text = "\n".join(rows).lower()
    for word in forbidden:
        if word and word in text:
            sys.exit(f"{name}: found {word!r} in the screen text; not writing screenshots")


def main(src, dst):
    src, dst = Path(src), Path(dst)
    dst.mkdir(parents=True, exist_ok=True)
    titles = {
        "listening": "portwatch",
        "connections": "portwatch",
        "events": "portwatch",
        "radar": "portwatch",
        "stop": "portwatch",
        "menu": "portwatch",
        "help": "portwatch",
        "tour": "portwatch",
        "technical": "portwatch",
        "inspector-overview": "portwatch",
        "inspector-process": "portwatch",
        "inspector-sockets": "portwatch",
    }
    docs = {p.stem: json.loads(p.read_text()) for p in sorted(src.glob("*.json"))}
    for name, doc in docs.items():
        swap_real_user_for_demo_name(doc)
        check_no_personal_text(name, doc)
    for name, title in titles.items():
        window(draw_screen(docs[name]), title).save(dst / f"{name}.png", optimize=True)
    # The theme gallery: four themes in a 2x2 grid.
    tiles = [window(draw_screen(docs[f"theme-{t}"]), t) for t in ("nord", "dracula", "cyberpunk", "matrix")]
    gap = 16 * SCALE
    tw, th = tiles[0].size
    sheet = Image.new("RGBA", (2 * tw + gap, 2 * th + gap), (0, 0, 0, 0))
    for n, tile in enumerate(tiles):
        sheet.paste(tile, ((n % 2) * (tw + gap), (n // 2) * (th + gap)))
    sheet = sheet.resize((sheet.width // 2, sheet.height // 2), Image.LANCZOS)
    sheet.save(dst / "themes.png", optimize=True)
    print("wrote", len(titles) + 1, "images to", dst)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
