#!/usr/bin/env python3
"""生成桌面应用图标（macOS .icns / 通用 icon.png）。

源:frontend/resources/logo-src/logo-light-mark.png(白标,透明底)
产物:
  - frontend/resources/icon.png   1024 macOS 规范圆角(内容 824 + 圆角 22.37%),黑底白标
  - frontend/build/icon.png       同上(electron-builder 通用回退)
  - frontend/build/icon.icns      iconutil 打包(mac 默认读取 buildResources/icon.icns)

依赖:pillow(pip install pillow);macOS 需 iconutil(系统自带)。
素材更新(logo 换版)后重跑本脚本即可。
"""
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
SRC_MARK = ROOT / "frontend" / "resources" / "logo-src" / "logo-light-mark.png"
OUT_ICON = ROOT / "frontend" / "resources" / "icon.png"
OUT_BUILD = ROOT / "frontend" / "build"

CANVAS = 1024
CONTENT = 824  # macOS 图标内容区(四周各留 100)
RADIUS = round(CONTENT * 0.2237)  # Big Sur 圆角比例
BG = (0, 0, 0, 255)  # 品牌黑底(与既有资源一致)
MARK_RATIO = 0.78  # 白标占内容区宽度比例


def build_master() -> Image.Image:
    canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    d = ImageDraw.Draw(canvas)
    inset = (CANVAS - CONTENT) // 2
    d.rounded_rectangle([inset, inset, inset + CONTENT, inset + CONTENT], RADIUS, fill=BG)

    mark = Image.open(SRC_MARK).convert("RGBA")
    bbox = mark.getbbox()
    if bbox is None:
        sys.exit(f"logo 源无有效像素: {SRC_MARK}")
    mark = mark.crop(bbox)
    target = round(CONTENT * MARK_RATIO)
    scale = target / max(mark.width, mark.height)
    mark = mark.resize((max(1, round(mark.width * scale)), max(1, round(mark.height * scale))), Image.LANCZOS)
    canvas.alpha_composite(mark, ((CANVAS - mark.width) // 2, (CANVAS - mark.height) // 2))
    return canvas


def main() -> None:
    if not SRC_MARK.is_file():
        sys.exit(f"缺少 logo 源: {SRC_MARK}")
    master = build_master()

    OUT_ICON.parent.mkdir(parents=True, exist_ok=True)
    master.save(OUT_ICON)
    print("wrote", OUT_ICON.relative_to(ROOT))

    OUT_BUILD.mkdir(parents=True, exist_ok=True)
    master.save(OUT_BUILD / "icon.png")
    print("wrote", (OUT_BUILD / "icon.png").relative_to(ROOT))

    iconset = OUT_BUILD / "icon.iconset"
    if iconset.exists():
        for f in iconset.iterdir():
            f.unlink()
    iconset.mkdir(exist_ok=True)
    for size in (16, 32, 128, 256, 512):
        master.resize((size, size), Image.LANCZOS).save(iconset / f"icon_{size}x{size}.png")
        master.resize((size * 2, size * 2), Image.LANCZOS).save(iconset / f"icon_{size}x{size}@2x.png")
    subprocess.run(
        ["iconutil", "-c", "icns", str(iconset), "-o", str(OUT_BUILD / "icon.icns")],
        check=True,
    )
    for f in iconset.iterdir():
        f.unlink()
    iconset.rmdir()
    print("wrote", (OUT_BUILD / "icon.icns").relative_to(ROOT))


if __name__ == "__main__":
    main()
