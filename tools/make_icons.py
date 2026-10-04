#!/usr/bin/env python3
"""Renders the OmniDrop icon and writes every platform size (Flutter asset, Android mipmaps,
Windows .ico, Linux hicolor PNGs). Run from the repository root: python3 tools/make_icons.py"""
import math
import os
from PIL import Image, ImageDraw, ImageFilter

S = 1024


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))


def render(rounded=True):
    top, bottom = (91, 140, 255), (141, 82, 255)
    grad = Image.new("RGBA", (S, S))
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        gd.line([(0, y), (S, y)], fill=lerp(top, bottom, y / S) + (255,))
    mask = Image.new("L", (S, S), 0)
    md = ImageDraw.Draw(mask)
    if rounded:
        md.rounded_rectangle([0, 0, S - 1, S - 1], radius=int(S * 0.23), fill=255)
    else:
        md.rectangle([0, 0, S - 1, S - 1], fill=255)
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    cx, cy = S / 2, S / 2
    # Radar rings.
    for r, alpha, w in [(S * 0.40, 70, 0.022), (S * 0.32, 130, 0.026)]:
        d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=(255, 255, 255, alpha), width=int(S * w))
    # Target disk with a "drop" arrow cut in the brand colour.
    r = S * 0.235
    d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=(255, 255, 255, 255))
    ink = lerp(top, bottom, 0.5) + (255,)
    shaft_w = S * 0.07
    d.rectangle([cx - shaft_w / 2, cy - S * 0.14, cx + shaft_w / 2, cy + S * 0.02], fill=ink)
    d.ellipse([cx - shaft_w / 2, cy - S * 0.14 - shaft_w / 2, cx + shaft_w / 2, cy - S * 0.14 + shaft_w / 2], fill=ink)
    head = S * 0.115
    tip = cy + S * 0.13
    d.polygon([(cx - head, tip - head), (cx + head, tip - head), (cx, tip)], fill=ink)
    glow = img.filter(ImageFilter.GaussianBlur(2))
    return Image.alpha_composite(glow, img)


def save(img, path, size):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    img.resize((size, size), Image.LANCZOS).save(path)


def main():
    icon = render(rounded=True)
    save(icon, "assets/icon.png", 512)
    for name, size in {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}.items():
        save(icon, f"android/app/src/main/res/mipmap-{name}/ic_launcher.png", size)
    for size in [32, 48, 64, 128, 256, 512]:
        save(icon, f"linux/packaging/icons/hicolor/{size}x{size}/apps/omnidrop.png", size)
    ico = icon.resize((256, 256), Image.LANCZOS)
    ico.save("windows/runner/resources/app_icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])


if __name__ == "__main__":
    main()
