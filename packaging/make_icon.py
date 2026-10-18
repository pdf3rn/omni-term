#!/usr/bin/env python3
"""Genera los iconos de omni-term (PNG en varios tamaños) con Pillow."""
import pathlib
from PIL import Image, ImageDraw

BASE, ACCENT, CURSOR = (0x1E, 0x1E, 0x2E), (0x89, 0xB4, 0xFA), (0xF5, 0xE0, 0xDC)
SIZE = 1024
here = pathlib.Path(__file__).parent

img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rounded_rectangle((48, 48, SIZE - 48, SIZE - 48), radius=220, fill=BASE + (255,))
d.rounded_rectangle((48, 48, SIZE - 48, SIZE - 48), radius=220, outline=(0x45, 0x47, 0x5A, 255), width=14)

# Símbolo de prompt: un chevron y un cursor de bloque.
stroke = 96
points = [(290, 330), (540, 512), (290, 694)]
d.line(points, fill=ACCENT + (255,), width=stroke, joint="curve")
for x, y in (points[0], points[-1]):
    d.ellipse((x - stroke // 2, y - stroke // 2, x + stroke // 2, y + stroke // 2), fill=ACCENT + (255,))
d.rounded_rectangle((600, 618, 800, 700), radius=18, fill=CURSOR + (255,))

for size in (512, 256, 128, 64, 48, 32, 16):
    out = here / "icons" / f"{size}x{size}"
    out.mkdir(parents=True, exist_ok=True)
    img.resize((size, size), Image.LANCZOS).save(out / "omni-term.png")
img.resize((256, 256), Image.LANCZOS).save(here.parent / "assets" / "icon.png")
print("iconos generados")
