#!/usr/bin/env python3
"""Gera a logo do Guará: a cabeça do lobo-guará, de frente, recortada das
trilhas de um disco.

As trilhas são arcos concêntricos com centro abaixo do focinho. Na metade
direita estão inteiras (dados recuperados) e na esquerda quebradas em
fragmentos (dados perdidos). Focinho e olhos usam a cor do texto. As cores
são as do app (apps/gui/ui/app.css): --accent e --accent-2 no tema claro e
seus equivalentes no escuro, trocados por prefers-color-scheme.

Uso: python3 scripts/make-logo.py [pasta]   (padrão: apps/gui/ui)
Grava guara-logo.svg e guara.svg (mesma arte; o segundo é o ícone).
O PNG para os ícones do app (src-tauri/icons/source.png) sai desta arte
renderizada em 1024x1024.
"""
import os, sys

HEAD = ("M50 95 C46 88 43 80 40 74 C33 70 25 64 22 56 L14 6 L36 30 "
        "C40 28 45 27 50 27 C55 27 60 28 64 30 L86 6 L78 56 "
        "C75 64 67 70 60 74 C57 80 54 88 50 95 Z")
STYLE = (".a{stroke:#0f766e}.b{stroke:#14b8a6}.t{fill:#1d1d1f}"
         "@media (prefers-color-scheme:dark){.a{stroke:#2dd4bf}.b{stroke:#5eead4}.t{fill:#f5f5f7}}")


def tracks():
    out = []
    for i, r in enumerate(range(7, 104, 7)):
        top, bot = 100 - r, 100 + r
        out.append(f'<path class="a" d="M50 {top}A{r} {r} 0 0 1 50 {bot}"/>')
        dash = f"{3 + i % 3 * 3} {3 + (i * 5) % 4}"
        out.append(f'<path class="b" d="M50 {top}A{r} {r} 0 0 0 50 {bot}" '
                   f'stroke-dasharray="{dash}" stroke-dashoffset="{i * 3}"/>')
    return "".join(out)


def svg():
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" role="img" aria-label="Guará">'
        f'<style>{STYLE}</style>'
        f'<defs><clipPath id="g-head"><path d="{HEAD}"/></clipPath></defs>'
        f'<g clip-path="url(#g-head)" fill="none" stroke-width="4.2">{tracks()}'
        # o focinho é a ponta da própria cabeça (recortada pelo mesmo contorno),
        # então nenhuma trilha escapa pelas laterais
        '<rect class="t" stroke="none" x="0" y="74" width="100" height="30"/></g>'
        '<path class="t" d="M37 50Q42 47 46 52Q41 53 37 50ZM63 50Q58 47 54 52Q59 53 63 50Z"/>'
        '</svg>\n'
    )


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "apps/gui/ui"
    for name in ("guara-logo.svg", "guara.svg"):
        with open(os.path.join(out, name), "w") as f:
            f.write(svg())
