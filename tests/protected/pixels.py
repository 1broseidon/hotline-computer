"""Pixels without OCR: decode a PNG a tool returned (standard library only)
and compare or count its pixels. Enough for what the computer's screenshots
are: 8-bit RGB or RGBA, not interlaced."""
import base64
import struct
import zlib


def decode(png):
    """(width, height, rows): rows is a list of bytes, 3 or 4 bytes a pixel."""
    if isinstance(png, str):
        png = base64.b64decode(png)
    if png[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    pos, idat, head = 8, b"", None
    while pos < len(png):
        n, kind = struct.unpack(">I4s", png[pos:pos + 8])
        data = png[pos + 8:pos + 8 + n]
        pos += 12 + n
        if kind == b"IHDR":
            head = struct.unpack(">IIBBBBB", data)
        elif kind == b"IDAT":
            idat += data
        elif kind == b"IEND":
            break
    width, height, depth, colour, _, _, interlace = head
    if depth != 8 or colour not in (2, 6) or interlace:
        raise ValueError(f"unsupported PNG: depth {depth}, colour type {colour}, interlace {interlace}")
    bpp = 3 if colour == 2 else 4
    raw = zlib.decompress(idat)
    stride = width * bpp
    rows, prev, i = [], bytearray(stride), 0
    for _ in range(height):
        f, line = raw[i], bytearray(raw[i + 1:i + 1 + stride])
        i += 1 + stride
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + ((a + b) >> 1)) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(bytes(line))
        prev = line
    return width, height, [bytes(_rgb(r, bpp)) for r in rows]


def _rgb(row, bpp):
    if bpp == 3:
        return row
    out = bytearray()
    for x in range(0, len(row), 4):
        out += row[x:x + 3]
    return out


def crop(image, x, y, w, h):
    """The rows of one rectangle, comparable with ==."""
    _, _, rows = image
    return [r[x * 3:(x + w) * 3] for r in rows[y:y + h]]


def bbox(image, test):
    """(x, y, w, h) around every pixel test(r, g, b) holds for, or None."""
    _, _, rows = image
    xs, ys = [], []
    for y, r in enumerate(rows):
        hit = [x // 3 for x in range(0, len(r), 3) if test(r[x], r[x + 1], r[x + 2])]
        if hit:
            xs += [hit[0], hit[-1]]
            ys.append(y)
    if not ys:
        return None
    return min(xs), ys[0], max(xs) - min(xs) + 1, ys[-1] - ys[0] + 1


def count(image, test):
    """Pixels for which test(r, g, b) is true."""
    _, _, rows = image
    n = 0
    for r in rows:
        for x in range(0, len(r), 3):
            if test(r[x], r[x + 1], r[x + 2]):
                n += 1
    return n


def magenta(r, g, b):
    return r > 230 and g < 40 and b > 230


def cyan(r, g, b):
    return r < 40 and g > 230 and b > 230


def dark(r, g, b):
    return r < 100 and g < 100 and b < 100
