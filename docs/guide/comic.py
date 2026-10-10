"""Comic-style sample pictures for the guide: a girl (Mia) and a cat
(Whiskers), drawn with Pillow, so no real photos appear in the screenshots.
Every function returns the picture and the boxes the mock recognizer reports."""
import math
from PIL import Image, ImageDraw

W, H, S = 1600, 1067, 2          # picture size, supersampling factor
INK = (38, 28, 30)
SKIN = (247, 205, 164)
HAIR = (92, 52, 28)
SHIRT = (226, 70, 76)
JEANS = (62, 96, 168)
ORANGE = (240, 158, 62)
ORANGE_D = (214, 120, 36)


class Canvas:
    def __init__(self):
        self.im = Image.new("RGB", (W * S, H * S), "white")
        self.d = ImageDraw.Draw(self.im)

    def p(self, v):
        return round(v * S)

    def ellipse(self, cx, cy, rx, ry, fill, ow=6):
        s = self.p
        self.d.ellipse([s(cx - rx), s(cy - ry), s(cx + rx), s(cy + ry)], fill=fill, outline=INK if ow else None, width=s(ow))

    def rect(self, x0, y0, x1, y1, fill, ow=6, r=0):
        s = self.p
        box = [s(x0), s(y0), s(x1), s(y1)]
        if r:
            self.d.rounded_rectangle(box, radius=s(r), fill=fill, outline=INK if ow else None, width=s(ow))
        else:
            self.d.rectangle(box, fill=fill, outline=INK if ow else None, width=s(ow))

    def poly(self, pts, fill, ow=6):
        s = self.p
        self.d.polygon([(s(x), s(y)) for x, y in pts], fill=fill, outline=INK if ow else None, width=s(ow))

    def line(self, pts, w, fill=INK):
        s = self.p
        self.d.line([(s(x), s(y)) for x, y in pts], fill=fill, width=s(w), joint="curve")
        for x, y in (pts[0], pts[-1]):
            self.d.ellipse([s(x - w / 2), s(y - w / 2), s(x + w / 2), s(y + w / 2)], fill=fill)

    def arc(self, cx, cy, rx, ry, a0, a1, w, fill=INK):
        s = self.p
        self.d.arc([s(cx - rx), s(cy - ry), s(cx + rx), s(cy + ry)], a0, a1, fill=fill, width=s(w))

    def gradient(self, y0, y1, top, bottom):
        s = self.p
        for y in range(s(y0), s(y1)):
            t = (y - s(y0)) / max(1, s(y1) - s(y0))
            c = tuple(round(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
            self.d.line([(0, y), (W * S, y)], fill=c)

    def done(self):
        return self.im.resize((W, H), Image.LANCZOS)


# ---------------------------------------------------------------- scenery ----
def cloud(c, x, y, k=1.0):
    for dx, dy, r in [(-50, 8, 36), (0, -10, 48), (52, 6, 38), (14, 22, 36)]:
        c.ellipse(x + dx * k, y + dy * k, r * k, r * k, (255, 255, 255), ow=0)


def sun(c, x, y, r=62):
    for i in range(12):
        a = i * math.pi / 6
        c.line([(x + math.cos(a) * (r + 14), y + math.sin(a) * (r + 14)), (x + math.cos(a) * (r + 40), y + math.sin(a) * (r + 40))], 8, (255, 196, 64))
    c.ellipse(x, y, r, r, (255, 218, 90), ow=6)


def tree(c, x, y, k=1.0):
    c.rect(x - 22 * k, y - 190 * k, x + 22 * k, y, (150, 98, 58), ow=6)
    for dx, dy, r in [(-60, -240, 80), (60, -250, 85), (0, -310, 95)]:
        c.ellipse(x + dx * k, y + dy * k, r * k, r * k, (92, 170, 92))


def park(c):
    c.gradient(0, H, (128, 196, 244), (214, 238, 252))
    sun(c, 1320, 170)
    cloud(c, 380, 150); cloud(c, 820, 250, .8)
    c.ellipse(400, 1010, 1100, 380, (118, 190, 92))
    c.ellipse(1250, 1060, 760, 300, (96, 170, 78))
    tree(c, 1360, 800, 1.2)
    tree(c, 170, 760, .9)


def beach(c):
    c.gradient(0, 520, (120, 190, 240), (208, 236, 252))
    sun(c, 250, 160)
    cloud(c, 900, 140); cloud(c, 1250, 230, .8)
    c.rect(-10, 480, W + 10, 700, (70, 160, 214), ow=0)
    for i in range(7):
        for j in range(2):
            c.arc(120 + i * 240 + j * 110, 540 + j * 70, 60, 20, 200, 340, 6, (255, 255, 255))
    c.rect(-10, 700, W + 10, H + 10, (246, 222, 160), ow=0)
    c.line([(0, 700), (W, 700)], 6)
    # umbrella
    c.line([(1330, 480), (1330, 800)], 12, (150, 98, 58))
    c.poly([(1180, 560), (1330, 400), (1480, 560)], (226, 70, 76))
    c.poly([(1255, 480), (1330, 400), (1405, 480)], (255, 255, 255), ow=0)
    c.line([(1180, 560), (1330, 400), (1480, 560), (1180, 560)], 6)


def garden(c):
    c.gradient(0, 600, (150, 206, 244), (226, 244, 252))
    cloud(c, 1180, 150); cloud(c, 300, 200, .9)
    c.rect(-10, 600, W + 10, H + 10, (126, 196, 96), ow=0)
    for x in range(-20, W + 80, 110):
        c.rect(x, 470, x + 70, 760, (252, 250, 244), ow=5)
        c.poly([(x, 470), (x + 35, 430), (x + 70, 470)], (252, 250, 244), ow=5)
    c.rect(-10, 560, W + 10, 590, (252, 250, 244), ow=5)
    for i, col in enumerate([(240, 96, 140), (255, 206, 66), (160, 110, 220), (255, 130, 80)] * 4):
        x = 80 + i * 96 + (i % 3) * 14
        y = 880 + (i % 4) * 36
        c.line([(x, y), (x, y + 60)], 7, (60, 140, 60))
        for k in range(5):
            a = k * 2 * math.pi / 5
            c.ellipse(x + math.cos(a) * 17, y + math.sin(a) * 17, 13, 13, col, ow=4)
        c.ellipse(x, y, 10, 10, (255, 230, 120), ow=4)


def indoor(c):
    c.rect(-10, -10, W + 10, 780, (250, 226, 190), ow=0)
    for x in range(0, W, 160):
        c.line([(x, 0), (x, 780)], 3, (240, 212, 174))
    c.rect(-10, 780, W + 10, H + 10, (190, 138, 90), ow=0)
    c.line([(0, 780), (W, 780)], 7)
    c.rect(150, 150, 520, 520, (178, 224, 250), ow=8)
    c.line([(335, 150), (335, 520)], 8); c.line([(150, 335), (520, 335)], 8)
    c.ellipse(240, 250, 50, 28, (255, 255, 255), ow=0)
    c.rect(900, 200, 1240, 470, (252, 246, 236), ow=7, r=8)
    c.ellipse(1070, 335, 70, 70, (255, 150, 150))
    c.rect(1020, 700, 1060, 780, (150, 98, 58))   # table leg stub
    c.rect(700, 620, 1500, 700, (214, 96, 90), ow=7, r=30)     # sofa seat
    c.rect(700, 480, 1500, 640, (226, 110, 104), ow=7, r=40)   # sofa back
    c.rect(1380, 560, 1520, 780, (206, 84, 80), ow=7, r=30)    # arm
    c.rect(700, 560, 790, 780, (206, 84, 80), ow=7, r=30)
    c.rect(1060, 560, 1200, 640, (255, 214, 102), ow=6, r=22)   # cushion


# ------------------------------------------------------------- characters ----
def mia(c, x, y, k=1.0, mood="smile", wave=False, look=0):
    """x, y: between the feet. Returns the face box [x, y, w, h] in picture pixels."""
    r = 92 * k
    hy = y - 560 * k
    # legs and shoes
    for sx in (-1, 1):
        c.rect(x + sx * 62 * k - 34 * k, y - 250 * k, x + sx * 62 * k + 34 * k, y - 40 * k, JEANS, r=14)
        c.ellipse(x + sx * 70 * k, y - 22 * k, 52 * k, 24 * k, (250, 250, 250))
    # arms (behind the shirt)
    arm = lambda sx, hand: (c.line([(x + sx * 78 * k, y - 440 * k), hand], 44 * k + 12), c.line([(x + sx * 78 * k, y - 440 * k), hand], 44 * k, SHIRT),
                            c.ellipse(hand[0], hand[1], 28 * k, 28 * k, SKIN))
    arm(1, (x + 140 * k, y - 270 * k))
    arm(-1, (x - 150 * k, y - 640 * k) if wave else (x - 140 * k, y - 270 * k))
    # shirt, neck
    c.rect(x - 88 * k, y - 470 * k, x + 88 * k, y - 250 * k, SHIRT, r=36)
    c.rect(x - 22 * k, y - 490 * k, x + 22 * k, y - 450 * k, SKIN)
    c.ellipse(x, y - 360 * k, 24 * k, 24 * k, (255, 255, 255), ow=0)  # star badge
    # hair back, ears, head
    c.ellipse(x, hy - 6 * k, r * 1.18, r * 1.22, HAIR)
    for sx in (-1, 1):
        c.ellipse(x + sx * r * 0.98, hy + 10 * k, 17 * k, 22 * k, SKIN)
    c.ellipse(x, hy, r, r * 1.04, SKIN)
    # bangs
    c.d.pieslice([c.p(x - r * 1.06), c.p(hy - r * 1.14), c.p(x + r * 1.06), c.p(hy + r * 0.7)], 188, 352, fill=HAIR, outline=INK, width=c.p(6))
    c.poly([(x - r * 0.2, hy - r * 0.96), (x + r * 0.35, hy - r * 0.3), (x + r * 0.7, hy - r * 0.78)], HAIR, ow=0)
    # face
    for sx in (-1, 1):
        ex = x + sx * 38 * k + look * 6 * k
        c.ellipse(ex, hy + 6 * k, 19 * k, 25 * k, (255, 255, 255), ow=4)
        c.ellipse(ex + look * 5 * k, hy + 9 * k, 10 * k, 13 * k, INK, ow=0)
        c.ellipse(ex + look * 5 * k + 3 * k, hy + 3 * k, 4 * k, 4 * k, (255, 255, 255), ow=0)
        c.arc(ex, hy - 26 * k, 24 * k, 14 * k, 200, 340, 6)
        c.ellipse(x + sx * 62 * k, hy + 44 * k, 15 * k, 10 * k, (250, 150, 150), ow=0)
    c.arc(x, hy + 28 * k, 8 * k, 6 * k, 20, 160, 5)
    if mood == "grin":
        c.d.chord([c.p(x - 40 * k), c.p(hy + 30 * k), c.p(x + 40 * k), c.p(hy + 82 * k)], 0, 180, fill=(190, 54, 64), outline=INK, width=c.p(5))
        c.rect(x - 28 * k, hy + 56 * k, x + 28 * k, hy + 64 * k, (255, 255, 255), ow=0)
    else:
        c.arc(x, hy + 44 * k, 36 * k, 26 * k, 20, 160, 7)
    return [x - r, hy - r * 0.95, 2 * r, r * 1.95]


def whiskers(c, x, y, k=1.0, mood="calm", tail=1):
    """A sitting cat. Returns the pet's box [x, y, w, h]."""
    tx = x + tail * 130 * k
    c.line([(x + tail * 70 * k, y - 40 * k), (tx + tail * 60 * k, y - 60 * k), (tx + tail * 40 * k, y - 190 * k)], 38 * k + 12)
    c.line([(x + tail * 70 * k, y - 40 * k), (tx + tail * 60 * k, y - 60 * k), (tx + tail * 40 * k, y - 190 * k)], 38 * k, ORANGE)
    c.ellipse(x, y - 105 * k, 100 * k, 112 * k, ORANGE)
    c.ellipse(x, y - 80 * k, 56 * k, 80 * k, (255, 236, 206), ow=0)
    for sx in (-1, 1):
        c.ellipse(x + sx * 50 * k, y - 18 * k, 34 * k, 20 * k, (255, 236, 206))
    hy = y - 235 * k
    for sx in (-1, 1):
        c.poly([(x + sx * 20 * k, hy - 50 * k), (x + sx * 92 * k, hy - 124 * k), (x + sx * 88 * k, hy - 20 * k)], ORANGE)
        c.poly([(x + sx * 38 * k, hy - 58 * k), (x + sx * 76 * k, hy - 100 * k), (x + sx * 76 * k, hy - 40 * k)], (250, 170, 170), ow=0)
    c.ellipse(x, hy, 90 * k, 76 * k, ORANGE)
    for dx in (-24, 0, 24):
        c.line([(x + dx * k, hy - 70 * k), (x + dx * k * 1.1, hy - 44 * k)], 7 * k, ORANGE_D)
    for sx in (-1, 1):
        c.ellipse(x + sx * 36 * k, hy - 4 * k, 20 * k, 22 * k, (150, 220, 120), ow=4)
        c.ellipse(x + sx * 36 * k, hy - 4 * k, 6 * k, 18 * k, INK, ow=0)
        c.line([(x + sx * 52 * k, hy + 26 * k), (x + sx * 118 * k, hy + 14 * k)], 4)
        c.line([(x + sx * 52 * k, hy + 36 * k), (x + sx * 116 * k, hy + 44 * k)], 4)
    c.poly([(x - 11 * k, hy + 22 * k), (x + 11 * k, hy + 22 * k), (x, hy + 36 * k)], (250, 140, 150), ow=4)
    c.arc(x - 12 * k, hy + 36 * k, 12 * k, 10 * k, 20, 170, 4)
    c.arc(x + 12 * k, hy + 36 * k, 12 * k, 10 * k, 10, 160, 4)
    if mood == "yawn":
        c.ellipse(x, hy + 52 * k, 16 * k, 14 * k, (190, 54, 64), ow=4)
    left = min(x - 110 * k, x + tail * 70 * k)
    right = max(x + 110 * k, tx + tail * 60 * k + 20 * k)
    top = hy - 124 * k
    return [left, top, right - left, y - top + 4 * k]


def person(c, x, y, k=1.0, who="lena", mood="smile", wave=False, look=0):
    """Other comic people for the people screenshots. Same size and pose as Mia,
    but their own hair, skin, clothes and glasses or beard. Returns the face box."""
    st = PEOPLE[who]
    skin, hair, shirt, pants = st["skin"], st["hair"], st["shirt"], st.get("pants", JEANS)
    r = 92 * k
    hy = y - 560 * k
    for sx in (-1, 1):
        c.rect(x + sx * 62 * k - 34 * k, y - 250 * k, x + sx * 62 * k + 34 * k, y - 40 * k, pants, r=14)
        c.ellipse(x + sx * 70 * k, y - 22 * k, 52 * k, 24 * k, (250, 250, 250))
    arm = lambda sx, hand: (c.line([(x + sx * 78 * k, y - 440 * k), hand], 44 * k + 12), c.line([(x + sx * 78 * k, y - 440 * k), hand], 44 * k, shirt),
                            c.ellipse(hand[0], hand[1], 28 * k, 28 * k, skin))
    arm(1, (x + 140 * k, y - 270 * k))
    arm(-1, (x - 150 * k, y - 640 * k) if wave else (x - 140 * k, y - 270 * k))
    c.rect(x - 88 * k, y - 470 * k, x + 88 * k, y - 250 * k, shirt, r=36)
    c.rect(x - 22 * k, y - 490 * k, x + 22 * k, y - 450 * k, skin)
    if st.get("tie"):
        c.poly([(x - 14 * k, y - 470 * k), (x + 14 * k, y - 470 * k), (x + 10 * k, y - 330 * k), (x, y - 300 * k), (x - 10 * k, y - 330 * k)], st["tie"], ow=4)
    style = st["style"]
    # hair behind the head
    if style == "long":
        c.rect(x - r * 1.12, hy - r * 0.2, x + r * 1.12, hy + r * 1.5, hair, r=40)
    elif style == "bun":
        c.ellipse(x, hy - r * 1.18, r * 0.42, r * 0.42, hair)
    elif style == "curly":
        for i in range(9):
            a = math.pi * (1 + i / 8)
            c.ellipse(x + math.cos(a) * r * 1.02, hy + math.sin(a) * r * 1.02, r * 0.34, r * 0.34, hair)
    for sx in (-1, 1):
        c.ellipse(x + sx * r * 0.98, hy + 10 * k, 17 * k, 22 * k, skin)
    c.ellipse(x, hy, r, r * 1.04, skin)
    # hair on top
    if style in ("short", "bun", "long", "grey"):
        c.d.pieslice([c.p(x - r * 1.05), c.p(hy - r * 1.12), c.p(x + r * 1.05), c.p(hy + r * 0.62)], 190, 350, fill=hair, outline=INK, width=c.p(6))
    elif style == "curly":
        c.d.pieslice([c.p(x - r * 1.0), c.p(hy - r * 1.06), c.p(x + r * 1.0), c.p(hy + r * 0.5)], 195, 345, fill=hair, outline=INK, width=c.p(6))
    elif style == "bald":
        for sx in (-1, 1):
            c.ellipse(x + sx * r * 0.96, hy - 8 * k, 14 * k, 30 * k, hair)
    # beard / moustache
    if st.get("beard"):
        c.d.pieslice([c.p(x - r * 0.98), c.p(hy - r * 0.2), c.p(x + r * 0.98), c.p(hy + r * 1.0)], 10, 170, fill=hair, outline=INK, width=c.p(5))
        c.ellipse(x, hy + 40 * k, 34 * k, 18 * k, skin, ow=0)
    # eyes, brows, cheeks, nose
    for sx in (-1, 1):
        ex = x + sx * 38 * k + look * 6 * k
        c.ellipse(ex, hy + 6 * k, 19 * k, 25 * k, (255, 255, 255), ow=4)
        c.ellipse(ex + look * 5 * k, hy + 9 * k, 10 * k, 13 * k, INK, ow=0)
        c.ellipse(ex + look * 5 * k + 3 * k, hy + 3 * k, 4 * k, 4 * k, (255, 255, 255), ow=0)
        c.arc(ex, hy - 26 * k, 24 * k, 14 * k, 200, 340, 6, hair if style != "bald" else INK)
        if not st.get("beard"):
            c.ellipse(x + sx * 62 * k, hy + 44 * k, 15 * k, 10 * k, (250, 150, 150), ow=0)
    if st.get("glasses"):
        for sx in (-1, 1):
            c.ellipse(x + sx * 38 * k + look * 6 * k, hy + 6 * k, 30 * k, 28 * k, None, ow=6)
        c.line([(x - 8 * k, hy + 4 * k), (x + 8 * k, hy + 4 * k)], 5)
    c.arc(x, hy + 28 * k, 8 * k, 6 * k, 20, 160, 5)
    if mood == "grin":
        c.d.chord([c.p(x - 40 * k), c.p(hy + 30 * k), c.p(x + 40 * k), c.p(hy + 82 * k)], 0, 180, fill=(190, 54, 64), outline=INK, width=c.p(5))
        c.rect(x - 28 * k, hy + 56 * k, x + 28 * k, hy + 64 * k, (255, 255, 255), ow=0)
    else:
        c.arc(x, hy + 44 * k, 36 * k, 26 * k, 20, 160, 7)
    return [x - r, hy - r * 0.95, 2 * r, r * 1.95]


PEOPLE = {
    "rosa":   dict(skin=(244, 200, 160), hair=(200, 200, 205), shirt=(150, 96, 170), style="bun", glasses=True),
    "ben":    dict(skin=(232, 184, 140), hair=(70, 44, 30), shirt=(70, 150, 90), style="short", beard=True),
    "lena":   dict(skin=(250, 214, 178), hair=(236, 190, 80), shirt=(60, 170, 170), style="long"),
    "jonas":  dict(skin=(214, 160, 112), hair=(46, 30, 24), shirt=(240, 140, 50), style="curly"),
    "sam":    dict(skin=(150, 100, 70), hair=(30, 26, 28), shirt=(80, 110, 200), style="short", glasses=True),
    "priya":  dict(skin=(190, 130, 90), hair=(28, 24, 30), shirt=(40, 56, 110), style="long", pants=(60, 60, 70)),
    "marco":  dict(skin=(238, 192, 150), hair=(120, 90, 70), shirt=(130, 135, 145), style="bald", beard=True, pants=(60, 60, 70)),
    "chen":   dict(skin=(246, 208, 160), hair=(24, 22, 26), shirt=(250, 250, 250), style="short", glasses=True, tie=(200, 60, 70), pants=(60, 60, 70)),
    "stranger1": dict(skin=(222, 172, 130), hair=(90, 60, 40), shirt=(180, 180, 90), style="short"),
    "stranger2": dict(skin=(246, 204, 168), hair=(160, 110, 60), shirt=(120, 90, 160), style="long"),
}

BROWN = (170, 112, 66)
BROWN_D = (120, 76, 44)


def buddy(c, x, y, k=1.0, mood="calm", tail=1):
    """A sitting dog. Returns the pet's box."""
    tx = x + tail * 120 * k
    c.line([(x + tail * 80 * k, y - 50 * k), (tx + tail * 30 * k, y - 120 * k), (tx + tail * 50 * k, y - 190 * k)], 40 * k + 12)
    c.line([(x + tail * 80 * k, y - 50 * k), (tx + tail * 30 * k, y - 120 * k), (tx + tail * 50 * k, y - 190 * k)], 40 * k, BROWN)
    c.ellipse(x, y - 110 * k, 105 * k, 118 * k, BROWN)
    c.ellipse(x, y - 80 * k, 58 * k, 82 * k, (244, 222, 188), ow=0)
    for sx in (-1, 1):
        c.ellipse(x + sx * 52 * k, y - 20 * k, 36 * k, 21 * k, (244, 222, 188))
    hy = y - 245 * k
    for sx in (-1, 1):  # floppy ears
        c.ellipse(x + sx * 92 * k, hy + 6 * k, 38 * k, 74 * k, BROWN_D)
    c.ellipse(x, hy, 88 * k, 80 * k, BROWN)
    c.ellipse(x, hy + 34 * k, 52 * k, 40 * k, (244, 222, 188))
    c.ellipse(x, hy + 18 * k, 18 * k, 12 * k, INK, ow=0)
    for sx in (-1, 1):
        c.ellipse(x + sx * 34 * k, hy - 14 * k, 14 * k, 17 * k, (255, 255, 255), ow=4)
        c.ellipse(x + sx * 34 * k, hy - 12 * k, 7 * k, 9 * k, INK, ow=0)
        c.arc(x + sx * 34 * k, hy - 38 * k, 20 * k, 10 * k, 200, 340, 5)
    c.line([(x, hy + 28 * k), (x, hy + 44 * k)], 5)
    if mood == "pant":
        c.d.chord([c.p(x - 26 * k), c.p(hy + 36 * k), c.p(x + 26 * k), c.p(hy + 78 * k)], 0, 180, fill=(190, 54, 64), outline=INK, width=c.p(5))
        c.ellipse(x, hy + 66 * k, 12 * k, 16 * k, (250, 140, 150), ow=4)
    else:
        c.arc(x - 14 * k, hy + 44 * k, 14 * k, 10 * k, 20, 170, 5)
        c.arc(x + 14 * k, hy + 44 * k, 14 * k, 10 * k, 10, 160, 5)
    left = min(x - 130 * k, x + tail * 80 * k)
    right = max(x + 130 * k, tx + tail * 50 * k + 20 * k)
    top = hy - 82 * k
    return [left, top, right - left, y - top + 4 * k]


# ------------------------------------------------------------------ scenes ----
def scene(name):
    """Returns (picture, faces, pets). faces: [(who, box, mood)], pets likewise."""
    c = Canvas()
    faces, pets = [], []
    if name == "park":
        park(c)
        faces.append(("mia", mia(c, 520, 980, 1.0, "smile", wave=True), "smile"))
        pets.append(("whiskers", whiskers(c, 1010, 990, 0.95), "calm"))
    elif name == "beach":
        beach(c)
        pets.append(("whiskers", whiskers(c, 380, 1010, 0.85, tail=-1), "calm"))
        faces.append(("mia", mia(c, 880, 1020, 1.05, "grin", look=1), "grin"))
    elif name == "garden":
        garden(c)
        faces.append(("mia", mia(c, 760, 1040, 1.12, "grin", wave=True), "grin"))
    elif name == "sofa":
        indoor(c)
        pets.append(("whiskers", whiskers(c, 1130, 640, 0.95, "yawn", tail=-1), "yawn"))
    elif name == "close":
        c.gradient(0, H, (255, 226, 160), (250, 160, 140))
        for i in range(14):
            c.ellipse(80 + i * 118, 120 + (i % 3) * 70, 14, 14, [(255, 255, 255), (255, 120, 140), (120, 200, 255)][i % 3], ow=0)
        faces.append(("mia", mia(c, 800, 1560, 1.9, "smile", look=-1), "smile"))
    elif name == "cat2":
        garden(c)
        pets.append(("whiskers", whiskers(c, 800, 1020, 1.25, "calm"), "calm"))
    elif name.startswith("grp:"):
        # "grp:<background>:<who>,<who>,<who>[:<variant>]"
        _, bg, whos, *rest = name.split(":")
        v = int(rest[0]) if rest else 0
        {"park": park, "garden": garden, "beach": beach, "indoor": indoor}[bg](c)
        xs = [420, 800, 1180]
        moods = [("smile", "grin", "smile"), ("grin", "smile", "grin"), ("smile", "smile", "grin")][v % 3]
        for i, who in enumerate(whos.split(",")):
            m = moods[i]
            if who == "mia":
                faces.append(("mia", mia(c, xs[i], 1020 - 10 * (i % 2), 0.9, m, wave=(i == v % 3)), m))
            else:
                faces.append((who, person(c, xs[i], 1020 - 10 * (i % 2), 0.9, who, m, wave=(i == v % 3)), m))
    elif name.startswith("pets:"):
        # "pets:<background>:<variant>": the dog and the cat together
        _, bg, v = name.split(":")
        {"park": park, "garden": garden, "beach": beach, "indoor": indoor}[bg](c)
        pets.append(("buddy", buddy(c, 560, 1000, 1.0, "pant" if v != "2" else "calm"), "pant"))
        pets.append(("whiskers", whiskers(c, 1060, 1000, 0.95, "calm", tail=-1), "calm"))
    elif name.startswith("strangers"):
        # two passers-by far from the camera
        {"park": park, "beach": beach}[name.partition(":")[2] or "park"](c)
        faces.append(("stranger1", person(c, 300, 760, 0.5, "stranger1"), "smile"))
        faces.append(("stranger2", person(c, 780, 800, 0.5, "stranger2"), "smile"))
    elif name == "poster":
        indoor(c)
        faces.append(("poster", [1000, 265, 140, 140], "smile"))
    else:
        raise ValueError(name)
    return c.done(), faces, pets
