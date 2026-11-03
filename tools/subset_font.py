"""Build the embedded UI font: BIZ UDGothic (SIL OFL 1.1) subset to the
characters the window needs, so the binary stays small and the font costs
little memory at runtime.

Usage:  python tools/subset_font.py
Writes: assets/fonts/BIZUDGothic-Regular-subset.ttf and the OFL licence.
Requires: fonttools (pip install fonttools), network access on first run.
"""

import io
import os
import sys
import urllib.request

from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "assets", "fonts")
SRC_URL = "https://github.com/googlefonts/morisawa-biz-ud-gothic/raw/main/fonts/ttf/BIZUDGothic-Regular.ttf"
LICENSE_URL = "https://github.com/googlefonts/morisawa-biz-ud-gothic/raw/main/OFL.txt"
SRC = os.path.join(OUT_DIR, "BIZUDGothic-Regular.ttf")
OUT = os.path.join(OUT_DIR, "BIZUDGothic-Regular-subset.ttf")


def jis_level1_kanji():
    """Kanji in JIS X 0208 rows 16-47 (level 1), the ones that appear in
    everyday text such as process names."""
    chars = []
    for cp in range(0x4E00, 0xA000):
        c = chr(cp)
        try:
            b = c.encode("iso2022_jp")
        except UnicodeEncodeError:
            continue
        # ESC $ B <row+0x20> <cell+0x20> ESC ( B
        if len(b) == 8 and b[:3] == b"\x1b$B":
            row = b[3] - 0x20
            if 16 <= row <= 47:
                chars.append(c)
    return chars


def unicode_ranges():
    ranges = [
        (0x0020, 0x007E),  # ASCII
        (0x00A0, 0x00FF),  # Latin-1 (°, ×, etc.)
        (0x2010, 0x2027),  # dashes, quotes, ellipsis
        (0x2030, 0x203B),  # per mille, prime, reference mark
        (0x2100, 0x2109),  # ℃ and friends
        (0x2190, 0x2199),  # arrows
        (0x2200, 0x22FF),  # math (≥ ≤ ≒ ∞)
        (0x2460, 0x2473),  # circled numbers
        (0x25A0, 0x25FF),  # geometric shapes
        (0x3000, 0x303F),  # CJK symbols and punctuation
        (0x3040, 0x309F),  # Hiragana
        (0x30A0, 0x30FF),  # Katakana
        (0xFF00, 0xFFEF),  # full-width forms
    ]
    chars = []
    for lo, hi in ranges:
        chars.extend(chr(cp) for cp in range(lo, hi + 1))
    return chars


def fetch(url, path):
    if os.path.exists(path):
        return
    print(f"downloading {url}")
    with urllib.request.urlopen(url, timeout=60) as r:
        data = r.read()
    with open(path, "wb") as f:
        f.write(data)


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    fetch(SRC_URL, SRC)
    fetch(LICENSE_URL, os.path.join(OUT_DIR, "OFL.txt"))

    text = "".join(unicode_ranges() + jis_level1_kanji())
    options = subset.Options()
    options.layout_features = ["*"]
    options.name_IDs = ["*"]
    options.notdef_outline = True
    options.recalc_bounds = True
    options.hinting = False
    font = TTFont(SRC)
    subsetter = subset.Subsetter(options=options)
    subsetter.populate(text=text)
    subsetter.subset(font)
    font.save(OUT)
    print(f"wrote {OUT}: {os.path.getsize(OUT):,} bytes, {len(text):,} characters requested")
    os.remove(SRC)


if __name__ == "__main__":
    sys.exit(main())
