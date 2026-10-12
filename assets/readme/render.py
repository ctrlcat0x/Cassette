"""Render the README artwork from captured Cassette screens. Requires Pillow."""

from pathlib import Path
import math

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent
ASSETS = ROOT.parent
WIDTH, HEIGHT = 1800, 1120
CREAM = "#fff7e6"
MUTED = "#b8d8d7"


def font(size, weight=500):
    face = ImageFont.truetype(str(ASSETS / "fonts/Manrope[wght].ttf"), size)
    face.set_variation_by_axes([weight])
    return face


canvas = Image.new("RGB", (WIDTH, HEIGHT))
pixels = canvas.load()
for y in range(HEIGHT):
    for x in range(WIDTH):
        glow = math.exp(-(((x - 1510) / 770) ** 2 + ((y - 260) / 690) ** 2))
        pixels[x, y] = (int(6 + 7 * glow), int(36 + 76 * glow), int(43 + 73 * glow))
canvas = canvas.convert("RGBA")
draw = ImageDraw.Draw(canvas)

icon = Image.open(ASSETS / "cassette-256.png").convert("RGBA")
icon.thumbnail((92, 92), Image.Resampling.LANCZOS)
canvas.alpha_composite(icon, (76, 58))
draw.text((188, 62), "Cassette", font=font(62, 750), fill=CREAM)
draw.rounded_rectangle((1530, 77, 1724, 124), radius=24, outline="#73acac", width=1)
draw.text((1563, 85), "v0.5 beta", font=font(23, 600), fill=CREAM)

draw.multiline_text((78, 242), "Find your\nfrequency.", font=font(94, 750), fill=CREAM, spacing=4)
draw.multiline_text(
    (83, 500), "A familiar favourite.\nSomething unexpected.",
    font=font(30, 500), fill=MUTED, spacing=12,
)
draw.line((83, 655, 155, 655), fill="#f6aa49", width=5)
draw.multiline_text(
    (83, 698), "YouTube Music + local files\nSynced lyrics. Playlists. Themes.",
    font=font(23, 500), fill=CREAM, spacing=17,
)
draw.text((83, 1025), "Windows  /  macOS  /  Linux", font=font(22, 600), fill=MUTED)

screen = Image.open(ROOT / "home.png").convert("RGBA")
screen.thumbnail((1120, 760), Image.Resampling.LANCZOS)
x, y = 620, 272
mask = Image.new("L", screen.size)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, screen.width - 1, screen.height - 1), radius=22, fill=255)
shadow = Image.new("RGBA", canvas.size)
ImageDraw.Draw(shadow).rounded_rectangle(
    (x - 7, y + 20, x + screen.width + 7, y + screen.height + 20), radius=28, fill=(0, 9, 14, 190)
)
canvas = Image.alpha_composite(canvas, shadow.filter(ImageFilter.GaussianBlur(32)))
canvas.paste(screen, (x, y), mask)
draw = ImageDraw.Draw(canvas)
draw.rounded_rectangle((x, y, x + screen.width, y + screen.height), radius=22, outline="#477b7d", width=2)
draw.text((x + 3, y + screen.height + 38), "Captured from Cassette v0.5 beta on Windows.", font=font(18), fill=MUTED)
canvas.convert("RGB").save(ROOT / "hero.png", optimize=True)
