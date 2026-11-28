"""Generate tests/sample-set.pdf: a small 4-sheet drawing set (G, A, S, M)
with bookmarks, a title block, grids, walls and hatching, plus
tests/sample-set-rev1.pdf, an earlier revision with a few differences for
the compare screens. Used by the CI smoke test.
Run: python3 tests/make_sample.py"""
import math
import zlib

W, H = 2592, 1728  # 36" x 24" sheets, in points

SHEETS = [
    ("G-001", "COVER SHEET"),
    ("A-101", "FIRST FLOOR PLAN"),
    ("S-201", "FOUNDATION PLAN"),
    ("M-301", "MECHANICAL PLAN"),
]


def esc(t):
    return t.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")


def text(x, y, size, t, bold=False):
    font = "/F2" if bold else "/F1"
    return f"BT {font} {size} Tf {x:.1f} {y:.1f} Td ({esc(t)}) Tj ET\n"


def rect(x, y, w, h, lw=1.0):
    return f"{lw} w {x:.1f} {y:.1f} {w:.1f} {h:.1f} re S\n"


def line(x1, y1, x2, y2, lw=0.5):
    return f"{lw} w {x1:.1f} {y1:.1f} m {x2:.1f} {y2:.1f} l S\n"


def circle(cx, cy, r, lw=0.8):
    k = 0.5523 * r
    return (f"{lw} w {cx + r:.1f} {cy:.1f} m "
            f"{cx + r:.1f} {cy + k:.1f} {cx + k:.1f} {cy + r:.1f} {cx:.1f} {cy + r:.1f} c "
            f"{cx - k:.1f} {cy + r:.1f} {cx - r:.1f} {cy + k:.1f} {cx - r:.1f} {cy:.1f} c "
            f"{cx - r:.1f} {cy - k:.1f} {cx - k:.1f} {cy - r:.1f} {cx:.1f} {cy - r:.1f} c "
            f"{cx + k:.1f} {cy - r:.1f} {cx + r:.1f} {cy - k:.1f} {cx + r:.1f} {cy:.1f} c S\n")


def sheet(num, title, idx, rev):
    c = "0 0 0 RG 0 0 0 rg 1 J 1 j\n"
    c += rect(36, 36, W - 72, H - 72, 2.5)
    # Title block on the right.
    tb = W - 36 - 360
    c += rect(tb, 36, 360, H - 72, 1.5)
    c += line(tb, 300, W - 36, 300, 1.0)
    c += line(tb, 520, W - 36, 520, 1.0)
    c += text(tb + 20, 1500, 22, "RED-BEAM SAMPLE PROJECT", True)
    c += text(tb + 20, 1460, 14, "123 EXAMPLE STREET")
    c += text(tb + 20, 470, 16, title, True)
    c += text(tb + 20, 440, 12, "SCALE: 1/8\" = 1'-0\"")
    c += text(tb + 20, 120, 64, num, True)
    c += text(tb + 20, 250, 11, "REV 2   ISSUED FOR PERMIT" if rev == 2 else "REV 1   DESIGN DEVELOPMENT")
    if idx == 0:
        c += text(300, 1100, 72, "SAMPLE PROJECT", True)
        c += text(300, 1000, 28, "DRAWING INDEX")
        for i, (n, t) in enumerate(SHEETS):
            c += text(300, 940 - i * 40, 20, f"{n}    {t}")
        return c
    # Grids: 6 numbered columns, 4 lettered rows; bubbles at the ends.
    x0, x1, y0, y1 = 200, tb - 160, 180, H - 200
    xs = [x0 + i * (x1 - x0) / 5 for i in range(6)]
    ys = [y0 + i * (y1 - y0) / 3 for i in range(4)]
    c += "[12 4 2 4] 0 d\n"
    for i, x in enumerate(xs):
        c += line(x, y0 - 40, x, y1 + 40, 0.4)
    for y in ys:
        c += line(x0 - 40, y, x1 + 40, y, 0.4)
    c += "[] 0 d\n"
    for i, x in enumerate(xs):
        c += circle(x, y1 + 70, 26)
        c += text(x - 7, y1 + 62, 22, str(i + 1))
    for i, y in enumerate(ys):
        c += circle(x0 - 70, y, 26)
        c += text(x0 - 78, y - 8, 22, "ABCD"[i])
    if num.startswith("A"):
        # Exterior walls (double lines) and some rooms.
        c += rect(xs[0], ys[0], xs[5] - xs[0], ys[3] - ys[0], 3.0)
        c += rect(xs[0] + 10, ys[0] + 10, xs[5] - xs[0] - 20, ys[3] - ys[0] - 20, 1.5)
        for i, x in enumerate(xs[1:5]):
            # Rev 1 had the third partition one bay-quarter further east.
            if rev == 1 and i == 2:
                x += (xs[1] - xs[0]) / 4
            c += line(x, ys[0], x, ys[2], 2.0)
        c += line(xs[0], ys[2], xs[5], ys[2], 2.0)
        for i in range(4):
            cx = (xs[i] + xs[i + 1]) / 2
            c += text(cx - 50, (ys[0] + ys[2]) / 2, 16, f"UNIT {i + 101}", True)
            c += text(cx - 30, (ys[0] + ys[2]) / 2 - 22, 12, "850 SF")
        c += text(xs[2], ys[2] + 120, 16, "CORRIDOR" if rev == 2 else "HALL", True)
        c += circle(xs[4] + 120, ys[3] - 120, 34)
        c += line(xs[4] + 86, ys[3] - 120, xs[4] + 154, ys[3] - 120, 0.8)
        c += text(xs[4] + 112, ys[3] - 110, 14, "5")
        c += text(xs[4] + 96, ys[3] - 142, 12, "A-501")
    elif num.startswith("S"):
        for x in xs:
            for y in ys:
                c += f"0.6 g {x - 14:.1f} {y - 14:.1f} 28 28 re f 0 g\n"
        c += rect(xs[0], ys[0], xs[5] - xs[0], ys[3] - ys[0], 4.0)
        # Hatched slab edge.
        for i in range(0, 600 if rev == 2 else 300, 12):
            c += line(xs[0] + i, ys[0], xs[0] + i + 40, ys[0] + 40, 0.3)
        c += text(xs[1], ys[1] + 30, 14, "F1  4'-0\" x 4'-0\" x 1'-6\" FOOTING", True)
    else:
        for i in range(5):
            y = ys[1] + 30 + i * 6
            c += line(xs[0] + 40, y, xs[5] - 40, y, 0.6 if i % 2 else 1.2)
        for x in xs[1:5] if rev == 2 else xs[1:4]:
            c += rect(x - 30, ys[1] + 20, 60, 50, 1.0)
            c += text(x - 22, ys[1] + 80, 11, "SD-1")
        c += text(xs[0] + 40, ys[1] + 110, 14, "24x12 SUPPLY DUCT", True)
    return c


def build(rev):
    objs = []

    def add(body):
        objs.append(body)
        return len(objs)


    font1 = add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    font2 = add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>")
    pages_id = len(objs) + 1 + 2 * len(SHEETS)  # placeholder index, fixed below
    page_ids = []
    content_ids = []
    for i, (num, title) in enumerate(SHEETS):
        data = zlib.compress(sheet(num, title, i, rev).encode("latin-1"))
        content_ids.append(add(b"<< /Length %d /Filter /FlateDecode >>\nstream\n" % len(data) + data + b"\nendstream"))
        page_ids.append(add(b""))  # filled later
    pages_id = add(b"")
    outline_root = add(b"")
    item_ids = [add(b"") for _ in SHEETS]
    catalog = add(b"")

    for i, pid in enumerate(page_ids):
        objs[pid - 1] = (f"<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {W} {H}] "
                         f"/Resources << /Font << /F1 {font1} 0 R /F2 {font2} 0 R >> >> "
                         f"/Contents {content_ids[i]} 0 R >>").encode()
    kids = " ".join(f"{p} 0 R" for p in page_ids)
    objs[pages_id - 1] = f"<< /Type /Pages /Kids [{kids}] /Count {len(page_ids)} >>".encode()
    objs[outline_root - 1] = f"<< /Type /Outlines /First {item_ids[0]} 0 R /Last {item_ids[-1]} 0 R /Count {len(item_ids)} >>".encode()
    for i, iid in enumerate(item_ids):
        num, title = SHEETS[i]
        links = ""
        if i > 0:
            links += f" /Prev {item_ids[i - 1]} 0 R"
        if i + 1 < len(item_ids):
            links += f" /Next {item_ids[i + 1]} 0 R"
        objs[iid - 1] = (f"<< /Title ({num} - {title}) /Parent {outline_root} 0 R{links} "
                         f"/Dest [{page_ids[i]} 0 R /Fit] >>").encode()
    objs[catalog - 1] = f"<< /Type /Catalog /Pages {pages_id} 0 R /Outlines {outline_root} 0 R /PageMode /UseOutlines >>".encode()

    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for i, body in enumerate(objs):
        offsets.append(len(out))
        out += f"{i + 1} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
    for o in offsets:
        out += f"{o:010d} 00000 n \n".encode()
    out += f"trailer\n<< /Size {len(objs) + 1} /Root {catalog} 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    name = "sample-set.pdf" if rev == 2 else "sample-set-rev1.pdf"
    open(__file__.rsplit("/", 1)[0] + "/" + name, "wb").write(out)
    print(f"wrote {name}: {len(SHEETS)} sheets, {len(out)} bytes")


for r in (2, 1):
    build(r)
