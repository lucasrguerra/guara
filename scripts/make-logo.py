#!/usr/bin/env python3
"""Gera a identidade visual do Guará.

Conceito: pixels de tamanhos distintos, soltos à esquerda, que se juntam numa
grade cada vez mais densa até formar a cabeça de um lobo-guará à direita.

- O lobo é uma silhueta VETORIAL suave (curvas), sutil, sem serrilhado.
- À esquerda a silhueta é recortada por uma máscara de quadrados de tamanhos
  distintos que ficam cada vez mais densos até se fundirem no lobo inteiro.
- Partículas soltas (quadrados) completam a dissolução fora da silhueta.

Uso: python3 scripts/make-logo.py   -> apps/gui/ui/guara-logo.svg e guara.svg
"""
import math
import random
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "apps/gui/ui"

W, H = 160.0, 100.0
CELL = 2.5  # módulo da grade de pixels
COLS, ROWS = int(W / CELL), int(H / CELL)

# --- silhueta -------------------------------------------------------------
# Cabeça de lobo-guará (3/4, olhando para a direita): orelhas enormes, focinho
# longo e fino, e a juba escorrendo para a esquerda (é ela que se desmancha em
# pixels). Coordenadas 0..100 na altura; x pode ser negativo (juba).
WOLF = [
    (100, 57), (97.5, 54), (91, 51), (83, 48), (75, 45), (69, 41), (65, 36), (62, 29), (59, 20), (56, 11),
    (52.5, 4), (49, 0.5), (46, 4), (43, 13), (41, 23), (40, 32), (38, 31), (35, 22), (31, 11), (27.5, 3),
    (24.5, 0.5), (22, 5), (20, 15), (19, 27), (9, 35), (-6, 45), (-20, 58), (-30, 76), (-34, 100), (-10, 100), (20, 100), (44, 100),
    (60, 100), (59.2, 95), (59.6, 89), (62, 82), (65, 78), (72, 74), (82, 71.5), (92, 69), (98.5, 66.5), (100, 63),
]
# Interior das orelhas (escuro, como no animal real).
EAR_IN = [
    [(53, 9), (50, 3), (47, 10), (45, 20), (47, 27), (51, 24), (54, 15)],
    [(27.5, 8), (25, 4), (23, 14), (22, 24), (25, 27), (28, 20)],
]
WX0, WSCALE = 64.0, 88.0  # nariz em x=152 (margem de 8 à direita)
Y0, YS = 6.0, 88.0         # topo das orelhas em y=6, base em y=94  # x do desenho onde wx=0 e largura de 100 unidades do lobo


def to_wolf(x, y):
    return (x - WX0) * 100.0 / WSCALE, y * 100.0 / H


def wolf_at(x, y):
    wx, wy = to_wolf(x, y)
    if not _inside(WOLF, wx, wy):
        return False
    return math.hypot(wx - 71.0, wy - 49.0) >= 1.7  # olho


def ear_in(x, y):
    wx, wy = to_wolf(x, y)
    return any(_inside(p, wx, wy) for p in EAR_IN)


def _inside(poly, x, y):
    c = False
    j = len(poly) - 1
    for i in range(len(poly)):
        xi, yi = poly[i]
        xj, yj = poly[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            c = not c
        j = i
    return c


def smooth(t):
    t = max(0.0, min(1.0, t))
    return t * t * (3 - 2 * t)


def mix(c1, c2, t):
    t = max(0.0, min(1.0, t))
    a = [int(c1[i:i + 2], 16) for i in (1, 3, 5)]
    b = [int(c2[i:i + 2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(p + (q - p) * t):02x}" for p, q in zip(a, b))


def smooth_path(pts, dup=()):
    """Curva fechada (Catmull-Rom -> Bézier); `dup` = índices de quinas vivas."""
    P = []
    for i, p in enumerate(pts):
        P.append(p)
        if i in dup:
            P.append(p)
    n = len(P)
    d = f"M{P[0][0]:.2f},{P[0][1]:.2f}"
    for i in range(n):
        p0, p1, p2, p3 = P[(i - 1) % n], P[i], P[(i + 1) % n], P[(i + 2) % n]
        c1 = (p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6)
        c2 = (p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6)
        d += f"C{c1[0]:.2f},{c1[1]:.2f} {c2[0]:.2f},{c2[1]:.2f} {p2[0]:.2f},{p2[1]:.2f}"
    return d + "Z"


def tx(pts):
    return [(WX0 + x * WSCALE / 100.0, Y0 + y * YS / 100.0) for x, y in pts]


def lerp(a, b, t):
    return a + (b - a) * t


def build(pal, scatter=1.0, seed=5, uid="a", x_min=8.0):
    rnd = random.Random(seed)
    wolf = tx(WOLF)
    corners = {i for i, (_, y) in enumerate(WOLF) if y >= 99.5 or y <= 0.6}  # pontas e cantos vivos
    body = smooth_path(wolf, dup=corners)
    ears = [smooth_path(tx(e)) for e in EAR_IN]
    cols, rows = int(W / CELL), int(H / CELL)

    # gradiente da figura (o mesmo para a silhueta e para cada pixel => fusão sem emenda)
    gx0, gy0, gx1, gy1 = 58.0, 0.0, 160.0, 100.0
    stops = pal["stops"]

    def grad(x, y):
        dx, dy = gx1 - gx0, gy1 - gy0
        t = max(0.0, min(1.0, ((x - gx0) * dx + (y - gy0) * dy) / (dx * dx + dy * dy)))
        for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
            if t <= t1:
                return mix(c0, c1, (t - t0) / (t1 - t0))
        return stops[-1][1]

    X_SOLID = 80.0  # múltiplo do módulo: a partir daqui a figura é inteira e lisa

    def ramp(x):
        return min(1.0, max(0.0, x / X_SOLID)) ** 2.0

    # envelope da "cauda": nasce na largura do pescoço e se afunila para a esquerda
    def envelope(x, y):
        k = smooth((x + 6) / 80.0)
        yc = lerp(70, 62, k)
        hh = lerp(5, 31, k ** 1.15)
        return math.exp(-(abs(y - yc) / hh) ** 3.0)

    occ = set()

    def free(cx, cy, n, m=0):
        return all((cx + i, cy + j) not in occ for i in range(-m, n + m) for j in range(-m, n + m))

    def take(cx, cy, n):
        for i in range(n):
            for j in range(n):
                occ.add((cx + i, cy + j))

    mask_sq, free_sq = [], []

    def emit(cx, cy, n, sz_scale, op, gap):
        x0, y0 = cx * CELL, cy * CELL
        size = n * CELL
        sz = size * sz_scale
        off = (size - sz) / 2
        cxm, cym = x0 + size / 2, y0 + size / 2
        inside = _inside(wolf, cxm, cym)
        if inside and x0 >= 56:
            # faz parte da figura lisa: vira janela da máscara
            e = 0.12
            mask_sq.append(f'<rect x="{x0 - e:.2f}" y="{y0 - e:.2f}" width="{size + 2 * e:.2f}" height="{size + 2 * e:.2f}" fill="#fff"/>')
        else:
            free_sq.append(f'<rect x="{x0 + off:.2f}" y="{y0 + off:.2f}" width="{sz:.2f}" height="{sz:.2f}" fill="{grad(cxm, cym)}" fill-opacity="{op:.2f}"/>')

    # 1) campo de pixels da esquerda até a figura: tamanhos em múltiplos do módulo
    order = [(cx, cy) for cy in range(rows) for cx in range(int(x_min / CELL), int(X_SOLID / CELL))]
    rnd.shuffle(order)
    for cx, cy in order:
        x, y = (cx + 0.5) * CELL, (cy + 0.5) * CELL
        r, env = ramp(x), envelope(x, y)
        p = r * env
        if p < 0.01 or y > 94:
            continue
        # blocos grandes na faixa média; sempre 1×1 colados perto da figura
        n = 1
        if r < 0.7:
            big = rnd.random() < 0.9 * (1 - r) * env ** 0.5 * min(1.0, r * 3.2)
            if big:
                n = rnd.choice([2, 2, 3]) if r < 0.45 else 2
        if cx + n > cols or cy + n > rows or not free(cx, cy, n, 0):
            n = 1
            if not free(cx, cy, 1, 0):
                continue
        acc = p if n == 1 else min(1.0, p * 1.6)
        if r > 0.88:
            acc = 1.0 if env > 0.45 else acc
        if rnd.random() > acc:
            continue
        take(cx, cy, n)
        gap_scale = 1.0 if r > 0.86 else (0.9 - 0.45 * (1 - r)) if n > 1 else rnd.choice([0.5, 0.65, 0.8, 0.9]) * (0.7 + 0.3 * r)
        op = 0.38 + 0.62 * smooth(r * 1.25)
        emit(cx, cy, n, gap_scale if r <= 0.86 else 1.02, op, 0)

    # 2) partículas esparsas bem à esquerda e acima/abaixo da cauda (profundidade)
    placed, tries, target = 0, 0, int(34 * scatter)
    while placed < target and tries < 6000:
        tries += 1
        x = x_min + rnd.random() ** 1.3 * (70 - x_min)
        cy = rnd.randrange(3, rows - 4)
        cx = int(x / CELL)
        y = (cy + 0.5) * CELL
        env = envelope(x, y)
        if env > 0.5 or not free(cx, cy, 1, 1):
            continue
        r = ramp(x)
        # sempre perto da cauda, nunca longe demais
        near = math.exp(-((abs(y - 62) - 26) / 22.0) ** 2)
        if rnd.random() > 0.55 * near * (0.35 + 0.65 * r):
            continue
        take(cx, cy, 1)
        emit(cx, cy, 1, rnd.choice([0.45, 0.6, 0.75]), 0.22 + 0.38 * r, 0)
        placed += 1

    ex, ey = tx([(71.0, 49.0)])[0]
    hx = " ".join(f'<stop offset="{t:g}" stop-color="{c}"/>' for t, c in stops)
    svg = (
        "<defs>"
        f'<linearGradient id="{uid}h" gradientUnits="userSpaceOnUse" x1="{gx0:g}" y1="{gy0:g}" x2="{gx1:g}" y2="{gy1:g}">{hx}</linearGradient>'
        f'<mask id="{uid}m" maskUnits="userSpaceOnUse" x="0" y="0" width="{W:g}" height="{H:g}"><g shape-rendering="crispEdges">'
        f'<rect x="{X_SOLID:g}" y="0" width="{W - X_SOLID + 1:g}" height="{H:g}" fill="#fff"/>'
        + "".join(mask_sq)
        + "</g></mask></defs>"
        f'<g mask="url(#{uid}m)"><path d="{body}" fill="url(#{uid}h)"/>'
        + "".join(f'<path d="{e}" fill="{pal["ear"]}" fill-opacity="{pal["ear_op"]}"/>' for e in ears)
        + "</g>"
        + '<g shape-rendering="crispEdges">' + "".join(free_sq) + "</g>"
        + f'<circle cx="{ex:.2f}" cy="{ey:.2f}" r="1.35" fill="{pal["eye"]}"/>'
    )
    return svg


def main():
    # Marca larga: funciona em fundo claro e escuro. Verde-azulado -> azul petróleo.
    pal = dict(stops=[(0.0, "#3BE3CC"), (0.45, "#12B5A8"), (1.0, "#0B6A86")], ear="#05323a", ear_op=0.5, eye="#ecfeff")
    (OUT / "guara-logo.svg").write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W:g} {H:g}" role="img" aria-label="Guará">' + build(pal, uid="w") + "</svg>\n"
    )
    # Ícone: fundo petróleo profundo, figura clara.
    ipal = dict(stops=[(0.0, "#d9fff7"), (0.5, "#8df1e0"), (1.0, "#3fc8cf")], ear="#0b4a55", ear_op=0.45, eye="#083344")
    (OUT / "guara.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" role="img" aria-label="Guará">'
        '<defs><linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#0f766e"/><stop offset="1" stop-color="#083344"/></linearGradient>'
        '<clipPath id="ic"><rect width="128" height="128" rx="29"/></clipPath></defs>'
        '<rect width="128" height="128" rx="29" fill="url(#bg)"/>'
        '<g clip-path="url(#ic)"><g transform="translate(-13 18) scale(0.86)">' + build(ipal, scatter=0.5, seed=9, uid="i", x_min=30.0) + "</g></g></svg>\n"
    )


if __name__ == "__main__":
    main()
