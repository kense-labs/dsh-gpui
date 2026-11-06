from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

root = Path(__file__).resolve().parents[1]
out = root / "src-tauri" / "icons"
out.mkdir(parents=True, exist_ok=True)
image = Image.new("RGBA", (512, 512), "#191919")
draw = ImageDraw.Draw(image)
draw.rounded_rectangle((48, 48, 464, 464), radius=96, outline="#353535", width=4)
draw.rounded_rectangle((104, 132, 408, 332), radius=32, fill="#f2f2f2")
draw.ellipse((350, 350, 408, 408), fill="#53d986")
try:
    font = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 112)
except OSError:
    font = ImageFont.load_default(size=112)
draw.text((146, 158), "DS", font=font, fill="#191919")
image.save(out / "icon.png")
