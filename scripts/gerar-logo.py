#!/usr/bin/env python3
"""Gera, a partir da logo oficial, os arquivos derivados usados no app e nos ícones.

A logo oficial é apps/gui/ui/guara.png (RGBA, fundo transparente, serve aos temas claro e escuro).
Ela não é alterada aqui. Saída:
  apps/gui/ui/guara-s.png                  128 px, recortada e quadrada (barra lateral e favicon)
  apps/gui/src-tauri/icons/source.png      1024 px, a arte sobre o fundo escuro com cantos arredondados;
                                           o CI e `npx @tauri-apps/cli icon` geram o resto dos ícones.

Uso: python3 scripts/gerar-logo.py
"""
import os
from PIL import Image, ImageDraw

RAIZ = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
UI = os.path.join(RAIZ, "apps/gui/ui")
ICONES = os.path.join(RAIZ, "apps/gui/src-tauri/icons")
FUNDO_ICONE = (10, 16, 28)  # o azul-noite do fundo da arte original


def recortar_quadrado(im, margem=0.04):
    """Recorta no que é visível (opacidade > 16, ignora resquícios) e centraliza num quadrado."""
    box = im.getchannel("A").point(lambda v: 255 if v > 16 else 0).getbbox()
    im = im.crop(box)
    w, h = im.size
    lado = round(max(w, h) * (1 + 2 * margem))
    quadro = Image.new("RGBA", (lado, lado), (0, 0, 0, 0))
    quadro.paste(im, ((lado - w) // 2, (lado - h) // 2), im)
    return quadro


def main():
    logo = recortar_quadrado(Image.open(os.path.join(UI, "guara.png")).convert("RGBA"))
    logo.resize((128, 128), Image.LANCZOS).save(os.path.join(UI, "guara-s.png"), optimize=True)
    print("guara-s.png")

    lado = 1024
    fundo = Image.new("RGBA", (lado, lado), FUNDO_ICONE + (255,))
    arte = logo.resize((round(lado * 0.82),) * 2, Image.LANCZOS)
    fundo.paste(arte, ((lado - arte.width) // 2, (lado - arte.height) // 2), arte)
    mascara = Image.new("L", (lado, lado), 0)
    ImageDraw.Draw(mascara).rounded_rectangle((0, 0, lado - 1, lado - 1), radius=round(lado * 0.2237), fill=255)
    icone = Image.new("RGBA", (lado, lado), (0, 0, 0, 0))
    icone.paste(fundo, (0, 0), mascara)
    icone.save(os.path.join(ICONES, "source.png"), optimize=True)
    print("icons/source.png")


if __name__ == "__main__":
    main()
