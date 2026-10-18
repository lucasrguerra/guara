#!/usr/bin/env python3
"""Gera a identidade visual do Guará: pixels de tamanhos distintos, soltos à esquerda,
que se juntam até formar a cabeça de um lobo-guará à direita.

Uso: python3 scripts/make-logo.py   (escreve apps/gui/ui/guara.svg e guara-logo.svg)
Determinístico (semente fixa): a mesma execução gera o mesmo desenho.
"""
import math
import random
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "apps/gui/ui"

# Cabeça de lobo-guará de perfil, olhando para a direita (coordenadas 0..100).
WOLF = [
    (-18, 100), (-2, 76), (10, 62), (16, 54), (21, 2), (30, 24), (41, 40), (47, 38), (57, 0), (62, 34), (72, 46),
    (88, 55), (100, 60), (100, 69), (88, 73), (74, 77), (68, 84), (62, 100), (40, 100),
]
EYE = (63, 54, 2.4)  # x, y, raio (em coords do lobo)


def inside(poly, x, y):
    c = False
    j = len(poly) - 1
    for i in range(len(poly)):
        xi, yi = poly[i]
        xj, yj = poly[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            c = not c
        j = i
    return c


def lerp(a, b, t):
    return a + (b - a) * t


def hexmix(c1, c2, t):
    a = [int(c1[i:i + 2], 16) for i in (1, 3, 5)]
    b = [int(c2[i:i + 2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(lerp(x, y, t)):02x}" for x, y in zip(a, b))


def smooth(t):
    t = max(0.0, min(1.0, t))
    return t * t * (3 - 2 * t)


def pixels(x_from, x_to, c_left, c_right, seed=7):
    rnd = random.Random(seed)
    cell = 4.0
    out = []
    wolf_x0, wolf_w = 62, 98  # onde o lobo começa e quanto mede (em unidades do desenho)
    for cy in range(25):
        for cx in range(40):
            x, y = cx * cell + cell / 2, cy * cell + cell / 2
            if not (x_from <= x <= x_to):
                continue
            t = smooth((x - 14) / 96)
            wx, wy = (x - wolf_x0) * 100 / wolf_w, y
            in_wolf = inside(WOLF, wx, wy) and math.hypot(wx - EYE[0], wy - EYE[1]) > EYE[2]
            if in_wolf:
                keep = 0.06 + 0.94 * t ** 1.15
                if t > 0.9:
                    keep = 1.0
                if rnd.random() > keep:
                    continue
                size = cell * (0.28 + 0.72 * t ** 0.9) * (1 if t > 0.9 else rnd.uniform(0.65, 1.15))
                jit = (1 - t) ** 1.4 * cell * 0.9
            else:
                if rnd.random() > 0.2 * (1 - t) ** 1.2 or x > 96:
                    continue
                size = cell * rnd.uniform(0.22, 0.85)
                jit = cell * 0.9
            size = min(size, cell * 1.05)
            px = x - size / 2 + rnd.uniform(-jit, jit)
            py = y - size / 2 + rnd.uniform(-jit, jit)
            col = hexmix(c_left, c_right, max(0.0, min(1.0, t * 1.05 + rnd.uniform(-0.12, 0.12))))
            op = 0.5 + 0.5 * t if not in_wolf or t < 0.9 else 1
            out.append((px, py, size, col, op))
    return out


def rects(px, dx=0.0, dy=0.0):
    return "".join(
        f'<rect x="{x + dx:.2f}" y="{y + dy:.2f}" width="{s:.2f}" height="{s:.2f}" rx="{s * 0.12:.2f}" fill="{c}" opacity="{o:.2f}"/>'
        for x, y, s, c, o in px
    )


def main():
    # Marca larga (fundo transparente): ciano claro -> verde-azulado profundo.
    wide = pixels(0, 160, "#67e8f9", "#0f766e")
    (OUT / "guara-logo.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 100" role="img" aria-label="Guará">' + rects(wide) + "</svg>\n"
    )
    # Ícone quadrado: fundo verde-azulado escuro, pixels claros.
    icon_px = pixels(30, 160, "#99f6e4", "#f0fdfa", seed=11)
    (OUT / "guara.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" role="img" aria-label="Guará">'
        '<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#14b8a6"/><stop offset="1" stop-color="#0c4a6e"/></linearGradient></defs>'
        '<rect width="128" height="128" rx="29" fill="url(#g)"/>'
        '<g transform="translate(-19 20) scale(0.88)">' + rects(icon_px) + "</g></svg>\n"
    )


if __name__ == "__main__":
    main()
