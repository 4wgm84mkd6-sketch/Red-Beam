# Generates assets/redbeam.ico (+ a preview PNG) for Red-Beam.
from PIL import Image, ImageDraw
import sys
out = sys.argv[1]
S = 1024  # draw large, downsample for crisp edges

def tile(size_px):
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # Rounded dark tile with a subtle vertical gradient.
    grad = Image.new("RGBA", (S, S))
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        t = y / S
        c = (int(46 - 18 * t), int(50 - 20 * t), int(56 - 22 * t), 255)
        gd.line([(0, y), (S, y)], fill=c)
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([24, 24, S - 24, S - 24], radius=200, fill=255)
    img.paste(grad, (0, 0), mask)
    # Steel I-beam: extrusion (depth) behind, then the red face.
    fl_l, fl_r = 212, 812        # flange extents
    web_l, web_r = 432, 592      # web extents
    top, top_b = 196, 356        # top flange
    bot_t, bot = 668, 828        # bottom flange
    def ibeam(dx, dy):
        return [
            (fl_l + dx, top + dy), (fl_r + dx, top + dy), (fl_r + dx, top_b + dy), (web_r + dx, top_b + dy),
            (web_r + dx, bot_t + dy), (fl_r + dx, bot_t + dy), (fl_r + dx, bot + dy), (fl_l + dx, bot + dy),
            (fl_l + dx, bot_t + dy), (web_l + dx, bot_t + dy), (web_l + dx, top_b + dy), (fl_l + dx, top_b + dy),
        ]
    depth = 56 if size_px >= 32 else 0
    for k in range(depth, 0, -4):
        d.polygon(ibeam(k, k), fill=(120, 20, 18, 255))
    d.polygon(ibeam(0, 0), fill=(229, 57, 53, 255))
    if size_px >= 32:
        # Highlights along the top faces of the flanges.
        d.rectangle([fl_l, top, fl_r, top + 34], fill=(255, 120, 100, 255))
        d.rectangle([fl_l, bot_t, web_l, bot_t + 28], fill=(255, 110, 92, 255))
        d.rectangle([web_r, bot_t, fl_r, bot_t + 28], fill=(255, 110, 92, 255))
    return img.resize((size_px, size_px), Image.LANCZOS)

sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
frames = [tile(s) for s in sizes]
frames[-1].save(out + "/redbeam.ico", format="ICO", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
tile(256).save(out + "/redbeam-256.png")
# Preview strip for checking small sizes.
strip = Image.new("RGBA", (sum(sizes) + 10 * len(sizes), 256), (240, 240, 240, 255))
x = 0
for f in frames:
    strip.paste(f, (x, 256 - f.height), f)
    x += f.width + 10
strip.save(out + "/preview-strip.png")
print("ok")
