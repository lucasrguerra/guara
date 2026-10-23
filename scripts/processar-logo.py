#!/usr/bin/env python3
"""Tira o fundo da logo do Guará e gera os arquivos usados no app, no README e nos ícones.

Entrada (arte original, fundo sólido escuro): assets/logo/guara-fonte.png.
Saída: apps/gui/ui/guara.png (768 px, fundo transparente, recortada e quadrada), guara-s.png
(128 px, para a barra lateral e o favicon) e apps/gui/src-tauri/icons/source.png (ícone do app,
sobre o fundo escuro da arte; o CI gera o resto com `tauri icon`). Uma logo só serve aos temas
claro e escuro: o laranja lê bem nos dois.

O fundo é uma cor sólida: sai o que é parecido com ela e está ligado à borda da imagem (sombras
escuras dentro do desenho ficam). Na faixa de 3 px ao redor, a cor do desenho é estimada pelos
vizinhos internos e a transparência vem de onde o pixel cai entre o fundo e essa cor, o que evita
halo escuro ou claro.

Uso: python3 scripts/processar-logo.py
"""
import os
from PIL import Image

RAIZ = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
UI = os.path.join(RAIZ, "apps/gui/ui")
ICONES = os.path.join(RAIZ, "apps/gui/src-tauri/icons")
FONTE = "assets/logo/guara-fonte.png"
FUNDO = (10, 16, 28)  # cor do fundo sólido da arte original
LIMIAR = 9.0  # distância que ainda conta como fundo certo


def tirar_fundo(path, bg, d0, _d1=None):
    """Remove o fundo conectado à borda e refina a faixa de 3 px ao redor do desenho.

    1. Fundo certo: pixels a menos de `d0` da cor do fundo, ligados à borda da imagem ou formando
       um bolsão grande (as frestas entre as pernas). Sombras escuras do desenho, que passam de
       `d0`, não são tocadas.
    2. Faixa de 3 px ao redor do fundo (a borda suavizada da arte): a cor verdadeira do desenho é
       estimada pelos vizinhos internos mais próximos, e a opacidade sai da projeção do pixel
       entre a cor do fundo e essa cor. Sem halo escuro ou claro.
    3. O resto fica como está, opaco.
    """
    from collections import deque

    im = Image.open(path).convert("RGB")
    w, h = im.size
    px = im.load()
    br, bgc, bb = bg
    d0sq = d0 * d0

    def perto(x, y):
        r, g, b = px[x, y]
        return (r - br) ** 2 + (g - bgc) ** 2 + (b - bb) ** 2 < d0sq

    # 1) fundo certo: flood fill a partir da borda
    fundo = bytearray(w * h)
    visto = bytearray(w * h)
    fila = deque()
    for x in range(w):
        for y in (0, h - 1):
            if perto(x, y) and not fundo[y * w + x]:
                fundo[y * w + x] = 1
                fila.append((x, y))
    for y in range(h):
        for x in (0, w - 1):
            if perto(x, y) and not fundo[y * w + x]:
                fundo[y * w + x] = 1
                fila.append((x, y))
    while fila:
        x, y = fila.popleft()
        for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
            if 0 <= nx < w and 0 <= ny < h and not fundo[ny * w + nx] and perto(nx, ny):
                fundo[ny * w + nx] = 1
                fila.append((nx, ny))

    # 1b) bolsões de fundo dentro do desenho (frestas entre as pernas, por exemplo): regiões grandes
    # com a cor do fundo que não tocam a borda. Manchas pequenas (brilho do olho) ficam.
    for i0 in range(w * h):
        if fundo[i0] or visto[i0]:
            continue
        x0, y0 = i0 % w, i0 // w
        if not perto(x0, y0):
            continue
        comp = [(x0, y0)]
        visto[i0] = 1
        k = 0
        while k < len(comp):
            x, y = comp[k]
            k += 1
            for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
                if 0 <= nx < w and 0 <= ny < h and not visto[ny * w + nx] and not fundo[ny * w + nx] and perto(nx, ny):
                    visto[ny * w + nx] = 1
                    comp.append((nx, ny))
        if len(comp) >= 60:
            for x, y in comp:
                fundo[y * w + x] = 1

    # 2) distância (em passos de 8 vizinhos) até o fundo, só até 3
    dist = bytearray([255]) * (w * h)
    fila = deque()
    for i in range(w * h):
        if fundo[i]:
            dist[i] = 0
            fila.append((i % w, i // w))
    while fila:
        x, y = fila.popleft()
        d = dist[y * w + x]
        if d >= 3:
            continue
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                nx, ny = x + dx, y + dy
                if 0 <= nx < w and 0 <= ny < h and dist[ny * w + nx] == 255:
                    dist[ny * w + nx] = d + 1
                    fila.append((nx, ny))

    out = Image.new("RGBA", (w, h))
    po = out.load()
    R = 6
    for y in range(h):
        for x in range(w):
            d = dist[y * w + x]
            if d == 0:
                po[x, y] = (0, 0, 0, 0)
            elif d == 255:
                r, g, b = px[x, y]
                po[x, y] = (r, g, b, 255)
            else:  # faixa da borda
                soma = [0, 0, 0]
                n = 0
                melhor = 99
                for yy in range(max(0, y - R), min(h, y + R + 1)):
                    for xx in range(max(0, x - R), min(w, x + R + 1)):
                        if dist[yy * w + xx] == 255:
                            q = (xx - x) ** 2 + (yy - y) ** 2
                            if q < melhor:
                                melhor, soma, n = q, list(px[xx, yy]), 1
                            elif q == melhor:
                                c = px[xx, yy]
                                soma = [soma[0] + c[0], soma[1] + c[1], soma[2] + c[2]]
                                n += 1
                r, g, b = px[x, y]
                if n == 0:
                    po[x, y] = (r, g, b, 255)
                    continue
                f = (soma[0] / n, soma[1] / n, soma[2] / n)
                vx, vy, vz = f[0] - br, f[1] - bgc, f[2] - bb
                nn = vx * vx + vy * vy + vz * vz
                a = ((r - br) * vx + (g - bgc) * vy + (b - bb) * vz) / nn if nn else 1.0
                a = 0.0 if a < 0.02 else 1.0 if a > 0.98 else a
                po[x, y] = (round(f[0]), round(f[1]), round(f[2]), round(a * 255))
    return out


def recortar_quadrado(im, margem=0.04, lado=1024):
    box = im.getchannel("A").point(lambda v: 255 if v > 8 else 0).getbbox()
    im = im.crop(box)
    w, h = im.size
    s = round(max(w, h) * (1 + 2 * margem))
    quadro = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    quadro.paste(im, ((s - w) // 2, (s - h) // 2), im)
    return quadro.resize((lado, lado), Image.LANCZOS)


def main():
    logo = recortar_quadrado(tirar_fundo(os.path.join(RAIZ, FONTE), FUNDO, LIMIAR))
    for sufixo, lado in (("", 768), ("-s", 128)):
        logo.resize((lado, lado), Image.LANCZOS).save(os.path.join(UI, f"guara{sufixo}.png"), optimize=True)
    print("guara.png e guara-s.png")
    # ícone do app: a arte sobre o fundo escuro da própria arte, com cantos arredondados
    from PIL import ImageDraw

    lado = 1024
    fundo = Image.new("RGBA", (lado, lado), FUNDO + (255,))
    arte = logo.resize((round(lado * 0.82),) * 2, Image.LANCZOS)
    fundo.paste(arte, ((lado - arte.width) // 2, (lado - arte.height) // 2), arte)
    mascara = Image.new("L", (lado, lado), 0)
    ImageDraw.Draw(mascara).rounded_rectangle((0, 0, lado - 1, lado - 1), radius=round(lado * 0.2237), fill=255)
    icone = Image.new("RGBA", (lado, lado), (0, 0, 0, 0))
    icone.paste(fundo, (0, 0), mascara)
    icone.save(os.path.join(ICONES, "source.png"), optimize=True)
    print("icons/source.png", icone.size)


if __name__ == "__main__":
    main()
