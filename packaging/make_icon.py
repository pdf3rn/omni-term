#!/usr/bin/env python3
"""Genera el logo de omni-term (una «O» seguida de un cursor de subrayado) en PNG, ICO e ICNS con Pillow.

Uso: python3 packaging/make_icon.py   (el ICNS necesita `png2icns`, de icnsutils)
"""
import pathlib
import shutil
import subprocess
from PIL import Image, ImageDraw

BASE, EDGE = (0x1E, 0x1E, 0x2E, 255), (0x45, 0x47, 0x5A, 255)
ACCENT, CURSOR = (0x89, 0xB4, 0xFA, 255), (0xF5, 0xE0, 0xDC, 255)
SIZE = 1024
SCALE = 2  # se dibuja al doble y se reduce para suavizar los bordes
here = pathlib.Path(__file__).parent


def logo() -> Image.Image:
    s = SIZE * SCALE
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    k = SCALE
    d.rounded_rectangle((48 * k, 48 * k, s - 48 * k, s - 48 * k), radius=220 * k, fill=BASE, outline=EDGE, width=14 * k)

    # «O» cerrada seguida de un cursor de subrayado, como en el prompt: O_
    thickness = 96 * k
    outer = 215 * k
    gap, cursor_w, cursor_h = 60 * k, 190 * k, 76 * k
    left = (s - (2 * outer + gap + cursor_w)) // 2
    cx, cy = left + outer, s // 2
    d.ellipse((cx - outer, cy - outer, cx + outer, cy + outer), fill=ACCENT)
    inner = outer - thickness
    d.ellipse((cx - inner, cy - inner, cx + inner, cy + inner), fill=BASE)
    x0 = cx + outer + gap
    d.rounded_rectangle((x0, cy + outer - cursor_h, x0 + cursor_w, cy + outer), radius=18 * k, fill=CURSOR)
    return img.resize((SIZE, SIZE), Image.LANCZOS)


img = logo()
sizes = (512, 256, 128, 64, 48, 32, 16)
for size in sizes:
    out = here / "icons" / f"{size}x{size}"
    out.mkdir(parents=True, exist_ok=True)
    img.resize((size, size), Image.LANCZOS).save(out / "omni-term.png")
img.resize((256, 256), Image.LANCZOS).save(here.parent / "assets" / "icon.png")

# Windows: un .ico con varias resoluciones.
img.save(here / "omni-term.ico", sizes=[(n, n) for n in (256, 128, 64, 48, 32, 16)])

# macOS: .icns a partir de los PNG.
if shutil.which("png2icns"):
    pngs = [str(here / "icons" / f"{n}x{n}" / "omni-term.png") for n in (512, 256, 128, 32, 16)]
    subprocess.run(["png2icns", str(here / "omni-term.icns"), *pngs], check=True)
print("iconos generados")
