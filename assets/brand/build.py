#!/usr/bin/env python3
"""Generates every Valka brand asset in this directory and installs the web copies.

    python3 assets/brand/build.py

Needs fontTools and Pillow. The OFL fonts are fetched from the google/fonts repository into
`.cache/` on first run. All text in the SVGs is outlined, so no file depends on installed fonts.
"""

from __future__ import annotations

import random
import struct
import urllib.request
from io import BytesIO
from pathlib import Path

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from PIL import Image, ImageDraw, ImageFont

OUT = Path(__file__).resolve().parent
ROOT = OUT.parent.parent
CACHE = OUT / ".cache"

INSTALL = {
    "web/public/favicon.svg": "favicon.svg",
    "web/public/favicon.ico": "favicon.ico",
    "web/public/apple-touch-icon.png": "apple-touch-icon.png",
    "web/public/valka.svg": "mark-on-dark.svg",
    "www/app/icon.svg": "favicon.svg",
    "www/app/apple-icon.png": "apple-touch-icon.png",
    "www/public/logo.svg": "mark.svg",
}

INK = "#111816"
SLATE = "#1B2422"
FROST = "#EEF2EF"
PATINA = "#3E9F88"
PATINA_LIGHT = "#67C2AA"
PATINA_DEEP = "#1F5B4F"
COPPER = "#C27A48"
COPPER_DEEP = "#9E5A2E"
MIST = "#B8C4BF"
SHADE = "#3F4B47"

DIM_OPACITY = 0.22
V_CELLS = {(0, 0), (0, 4), (1, 0), (1, 4), (2, 1), (2, 3), (3, 1), (3, 3), (4, 2)}

FONTS = {
    "display": ("schibstedgrotesk/SchibstedGrotesk%5Bwght%5D.ttf", {"wght": 800}),
    "body": ("instrumentsans/InstrumentSans%5Bwdth%2Cwght%5D.ttf", {"wght": 400, "wdth": 100}),
    "mono": ("jetbrainsmono/JetBrainsMono%5Bwght%5D.ttf", {"wght": 500}),
}

TAGLINE = "A distributed task queue whose only dependency is a bucket."
FACTS = "acked = durable  ·  4096 shards  ·  rust  ·  four SDKs"


def fetch(rel: str) -> Path:
    CACHE.mkdir(exist_ok=True)
    path = CACHE / rel.split("/")[-1].replace("%5B", "[").replace("%5D", "]").replace("%2C", ",")
    if not path.exists():
        url = f"https://github.com/google/fonts/raw/main/ofl/{rel}"
        with urllib.request.urlopen(url, timeout=60) as r:
            path.write_bytes(r.read())
    return path


class Face:
    def __init__(self, role: str):
        rel, axes = FONTS[role]
        self.path = fetch(rel)
        self.axes = axes
        self.font = instantiateVariableFont(TTFont(self.path), axes)
        self.upm = self.font["head"].unitsPerEm
        self.cmap = self.font.getBestCmap()
        self.glyphs = self.font.getGlyphSet()
        self.hmtx = self.font["hmtx"]
        self._pair_lookups = self._kern_lookups()

    def _kern_lookups(self):
        if "GPOS" not in self.font:
            return []
        table = self.font["GPOS"].table
        indices = sorted(
            {
                i
                for rec in table.FeatureList.FeatureRecord
                if rec.FeatureTag == "kern"
                for i in rec.Feature.LookupListIndex
            }
        )
        lookups = []
        for i in indices:
            lookup = table.LookupList.Lookup[i]
            subtables = []
            for st in lookup.SubTable:
                if lookup.LookupType == 9:
                    if st.ExtensionLookupType != 2:
                        continue
                    st = st.ExtSubTable
                elif lookup.LookupType != 2:
                    continue
                subtables.append(st)
            lookups.append(subtables)
        return lookups

    def kern(self, left: str, right: str) -> float:
        total = 0.0
        for subtables in self._pair_lookups:
            for st in subtables:
                if left not in st.Coverage.glyphs:
                    continue
                if st.Format == 1:
                    pairs = st.PairSet[st.Coverage.glyphs.index(left)].PairValueRecord
                    rec = next((p for p in pairs if p.SecondGlyph == right), None)
                    if rec is None:
                        continue
                    total += getattr(rec.Value1, "XAdvance", 0) or 0
                else:
                    c1 = st.ClassDef1.classDefs.get(left, 0)
                    c2 = st.ClassDef2.classDefs.get(right, 0)
                    value = st.Class1Record[c1].Class2Record[c2].Value1
                    total += getattr(value, "XAdvance", 0) or 0
                break
        return total

    def outline(self, text: str, size: float, x: float, baseline: float, tracking: float = 0.0):
        """SVG path data for `text` with its left edge at `x`, and the advance width."""
        scale = size / self.upm
        names = [self.cmap[ord(ch)] for ch in text]
        pen = SVGPathPen(self.glyphs, ntos=lambda v: f"{v:.2f}".rstrip("0").rstrip("."))
        cursor = 0.0
        for i, name in enumerate(names):
            origin = x + cursor * scale
            self.glyphs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, origin, baseline)))
            cursor += self.hmtx[name][0] + tracking * self.upm
            if i + 1 < len(names):
                cursor += self.kern(name, names[i + 1])
        cursor -= tracking * self.upm
        return pen.getCommands(), cursor * scale

    def ascender_of(self, ch: str) -> float:
        """Height above the baseline of the glyph for `ch`, in em."""
        bp = BoundsPen(self.glyphs)
        self.glyphs[self.cmap[ord(ch)]].draw(bp)
        return bp.bounds[3] / self.upm

    def pil(self, size: int) -> ImageFont.FreeTypeFont:
        font = ImageFont.truetype(str(self.path), size, layout_engine=ImageFont.Layout.RAQM)
        font.set_variation_by_axes([self._axis_value(a) for a in font.get_variation_axes()])
        return font

    def _axis_value(self, axis: dict) -> float:
        name = axis["name"]
        name = name.decode() if isinstance(name, bytes) else name
        tag = {"Weight": "wght", "Width": "wdth"}.get(name, name)
        return self.axes.get(tag, axis["default"])


def cells(x: float, y: float, cell: float, gap: float):
    """(rect x, rect y, lit) for the 5x5 shard grid, top-left at (x, y)."""
    pitch = cell + gap
    for r in range(5):
        for c in range(5):
            yield x + c * pitch, y + r * pitch, (r, c) in V_CELLS


def mark_rects(x, y, size, lit, dim=None, dim_opacity=DIM_OPACITY, rounded=True) -> str:
    cell = size * 9 / 58
    gap = (size - 5 * cell) / 4
    rx = f' rx="{fmt(cell / 6)}"' if rounded else ""
    dim = dim or lit
    out = []
    for cx, cy, on in cells(x, y, cell, gap):
        fill = f'fill="{lit}"' if on else f'fill="{dim}" fill-opacity="{dim_opacity}"'
        out.append(f'<rect x="{fmt(cx)}" y="{fmt(cy)}" width="{fmt(cell)}" height="{fmt(cell)}"{rx} {fill}/>')
    return "".join(out)


def fmt(v: float) -> str:
    return f"{v:.2f}".rstrip("0").rstrip(".")


def svg(width, height, body, title="Valka") -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {fmt(width)} {fmt(height)}" '
        f'width="{fmt(width)}" height="{fmt(height)}" role="img" aria-label="{title}">'
        f"<title>{title}</title>{body}</svg>\n"
    )


def field_cells(cols: int, rows: int, seed: int):
    """(col, row, accent, alpha) for a shard field that brightens toward the right edge."""
    rng = random.Random(seed)
    for r in range(rows):
        for c in range(cols):
            t = (c + 1) / cols
            roll = rng.random()
            if t > 0.5 and roll < 0.015 * t:
                yield c, r, True, 0.9 * t
            elif roll < 0.12 * t:
                yield c, r, False, 0.45 * t
            else:
                yield c, r, False, 0.08 * t


def shard_field(x0, y0, width, height, color, accent, seed) -> str:
    cell, pitch = 10, 15
    return "".join(
        f'<rect x="{fmt(x0 + c * pitch)}" y="{fmt(y0 + r * pitch)}" width="{cell}" '
        f'height="{cell}" rx="1.5" fill="{accent if lit else color}" fill-opacity="{fmt(alpha)}"/>'
        for c, r, lit, alpha in field_cells(int(width // pitch), int(height // pitch), seed)
    )


def lockup(display: Face, mark_color: str, word_color: str) -> str:
    mark = 64
    size = mark / display.ascender_of("l")
    d, width = display.outline("valka", size, mark + 26, mark, tracking=-0.02)
    body = mark_rects(0, 0, mark, mark_color) + f'<path fill="{word_color}" d="{d}"/>'
    return svg(mark + 26 + width, mark, body)


def banner(display: Face, body_face: Face, mono: Face, dark: bool) -> str:
    w, h = 900, 300
    bg, mark_c, word_c, tag_c, fact_c = (
        (INK, PATINA_LIGHT, FROST, MIST, COPPER)
        if dark
        else (FROST, PATINA_DEEP, INK, SHADE, COPPER_DEEP)
    )
    field = shard_field(560, 30, 330, 240, mark_c, fact_c, seed=4096)
    mark, left, top = 104, 64, 64
    size = 84
    word_d, _ = display.outline("valka", size, left + mark + 30, top + mark, tracking=-0.02)
    tag_d, _ = body_face.outline(TAGLINE, 19, left, 214)
    fact_d, _ = mono.outline(FACTS, 13, left, 246)
    body = (
        f'<rect width="{w}" height="{h}" rx="16" fill="{bg}"/>'
        + field
        + mark_rects(left, top, mark, mark_c)
        + f'<path fill="{word_c}" d="{word_d}"/>'
        + f'<path fill="{tag_c}" d="{tag_d}"/>'
        + f'<path fill="{fact_c}" d="{fact_d}"/>'
    )
    return svg(w, h, body, "Valka, a distributed task queue whose only dependency is a bucket")


def favicon_svg() -> str:
    rects = []
    for cx, cy, on in cells(1, 1, 2, 1):
        cls = "v" if on else "d"
        rects.append(f'<rect class="{cls}" x="{cx}" y="{cy}" width="2" height="2"/>')
    style = (
        f".v,.d{{fill:{PATINA}}}.d{{fill-opacity:.3}}"
        f"@media (prefers-color-scheme:dark){{.v,.d{{fill:{PATINA_LIGHT}}}}}"
    )
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">'
        f"<style>{style}</style>{''.join(rects)}</svg>\n"
    )


def rgba(hex_color: str, alpha: float = 1.0):
    h = hex_color.lstrip("#")
    return tuple(int(h[i : i + 2], 16) for i in (0, 2, 4)) + (round(alpha * 255),)


def pixel_icon(n: int) -> Image.Image:
    """Transparent icon snapped to whole pixels: 5 cells + 4 gaps + 2 margins = n."""
    cell, gap, margin = {16: (2, 1, 1), 32: (4, 2, 2), 48: (6, 3, 3)}[n]
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    for cx, cy, on in cells(margin, margin, cell, gap):
        color = rgba(PATINA) if on else rgba(PATINA, 0.3)
        draw.rectangle([cx, cy, cx + cell - 1, cy + cell - 1], fill=color)
    return img


def app_icon(n: int) -> Image.Image:
    """Opaque icon on Ink for home screens and app launchers, drawn at 4x then downsampled."""
    k = 4
    big = n * k
    img = Image.new("RGBA", (big, big), rgba(INK))
    draw = ImageDraw.Draw(img)
    size = round(big * 0.58)
    cell = size * 9 / 58
    gap = (size - 5 * cell) / 4
    origin = (big - size) / 2
    radius = cell / 6
    for cx, cy, on in cells(origin, origin, cell, gap):
        color = rgba(PATINA_LIGHT) if on else blend(PATINA_LIGHT, INK, DIM_OPACITY)
        draw.rounded_rectangle([cx, cy, cx + cell, cy + cell], radius=radius, fill=color)
    return img.resize((n, n), Image.LANCZOS)


def blend(fg: str, bg: str, alpha: float):
    f, b = rgba(fg), rgba(bg)
    return tuple(round(f[i] * alpha + b[i] * (1 - alpha)) for i in range(3)) + (255,)


def write_ico(path: Path, images: list[Image.Image]) -> None:
    """ICO with embedded PNGs, one entry per size."""
    blobs = []
    for im in images:
        buf = BytesIO()
        im.save(buf, format="PNG")
        blobs.append((im.size[0], buf.getvalue()))
    header = struct.pack("<HHH", 0, 1, len(blobs))
    offset = 6 + 16 * len(blobs)
    entries, data = b"", b""
    for size, blob in blobs:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
        data += blob
    path.write_bytes(header + entries + data)


def social_preview(display: Face, body_face: Face, mono: Face) -> Image.Image:
    w, h = 1280, 640
    img = Image.new("RGB", (w, h), rgba(INK)[:3])
    draw = ImageDraw.Draw(img, "RGBA")
    pitch, cell, x0, y0 = 22, 14, 744, 44
    for c, r, lit, alpha in field_cells(23, 15, seed=1280):
        x, y = x0 + c * pitch, y0 + r * pitch
        color = rgba(COPPER if lit else PATINA_LIGHT, alpha)
        draw.rounded_rectangle([x, y, x + cell, y + cell], radius=2, fill=color)
    left, top, mark = 96, 150, 176
    cell = mark * 9 / 58
    gap = (mark - 5 * cell) / 4
    for cx, cy, on in cells(left, top, cell, gap):
        color = rgba(PATINA_LIGHT) if on else rgba(PATINA_LIGHT, DIM_OPACITY)
        draw.rounded_rectangle([cx, cy, cx + cell, cy + cell], radius=cell / 6, fill=color)
    word = display.pil(150)
    draw.text((left + mark + 44, top + mark), "valka", font=word, fill=rgba(FROST), anchor="ls")
    draw.text((left, 440), TAGLINE, font=body_face.pil(38), fill=rgba(MIST), anchor="ls")
    draw.text((left, 500), FACTS, font=mono.pil(24), fill=rgba(COPPER), anchor="ls")
    return img


def main() -> None:
    display, body_face, mono = Face("display"), Face("body"), Face("mono")
    files = {
        "mark.svg": svg(64, 64, mark_rects(3, 3, 58, PATINA)),
        "mark-on-dark.svg": svg(64, 64, mark_rects(3, 3, 58, PATINA_LIGHT)),
        "mark-on-light.svg": svg(64, 64, mark_rects(3, 3, 58, PATINA_DEEP)),
        "mark-mono-black.svg": svg(64, 64, mark_rects(3, 3, 58, "#000000")),
        "mark-mono-white.svg": svg(64, 64, mark_rects(3, 3, 58, "#FFFFFF")),
        "logo-on-dark.svg": lockup(display, PATINA_LIGHT, FROST),
        "logo-on-light.svg": lockup(display, PATINA_DEEP, INK),
        "banner-dark.svg": banner(display, body_face, mono, dark=True),
        "banner-light.svg": banner(display, body_face, mono, dark=False),
        "favicon.svg": favicon_svg(),
    }
    for name, text in files.items():
        (OUT / name).write_text(text)

    icons = {n: pixel_icon(n) for n in (16, 32, 48)}
    icons[16].save(OUT / "favicon-16.png")
    icons[32].save(OUT / "favicon-32.png")
    write_ico(OUT / "favicon.ico", [icons[16], icons[32], icons[48]])
    app_icon(180).save(OUT / "apple-touch-icon.png")
    app_icon(192).save(OUT / "icon-192.png")
    app_icon(512).save(OUT / "icon-512.png")
    social_preview(display, body_face, mono).save(OUT / "social-preview.png")

    for dest, name in INSTALL.items():
        (ROOT / dest).write_bytes((OUT / name).read_bytes())

    written = sorted(p.name for p in OUT.iterdir() if p.suffix in {".svg", ".png", ".ico"})
    print("\n".join(written + [f"installed {dest}" for dest in INSTALL]))


if __name__ == "__main__":
    main()
