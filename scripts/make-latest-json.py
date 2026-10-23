#!/usr/bin/env python3
"""Monta o latest.json que o atualizador do app lê (Tauri updater), a partir dos pacotes e das
assinaturas `.sig` já enviados à release.

Roda no job de publicação, depois que todos os jobs de empacotamento terminaram. Montar o arquivo
aqui, em um passo só, evita a corrida de quem tenta atualizá-lo em paralelo por plataforma.

Uso: make-latest-json.py <tag> <notas.md> <saida.json>   (usa `gh` e GH_REPO/GITHUB_REPOSITORY)
"""
import json
import os
import subprocess
import sys
import tempfile
from datetime import datetime, timezone

# (chaves de plataforma, padrão do nome do pacote). {v} = versão sem o "v".
PLATFORMS = [
    (["linux-x86_64", "linux-x86_64-appimage"], "Guara_{v}_amd64.AppImage"),
    (["windows-x86_64", "windows-x86_64-nsis"], "Guara_{v}_x64-setup.exe"),
    (["windows-x86_64-msi"], "Guara_{v}_x64_en-US.msi"),
    (["darwin-aarch64", "darwin-aarch64-app"], "Guara_{v}_aarch64.app.tar.gz"),
    (["darwin-x86_64", "darwin-x86_64-app"], "Guara_{v}_x64.app.tar.gz"),
]


def build(version, notes, assets, pub_date):
    """`assets` = {nome_do_pacote: (url, assinatura)}. Devolve o dicionário do latest.json.

    Plataforma sem pacote ou sem assinatura sai da lista; o chamador decide se isso é erro."""
    platforms = {}
    for keys, pattern in PLATFORMS:
        name = pattern.format(v=version)
        if name not in assets:
            continue
        url, sig = assets[name]
        for k in keys:
            platforms.setdefault(k, {"signature": sig.strip(), "url": url})
    return {"version": version, "notes": notes.strip(), "pub_date": pub_date, "platforms": platforms}


def main():
    tag, notes_path, out = sys.argv[1:4]
    version = tag.lstrip("v")
    repo = os.environ.get("GH_REPO") or os.environ["GITHUB_REPOSITORY"]
    rel = json.loads(subprocess.check_output(["gh", "release", "view", tag, "--repo", repo, "--json", "assets"]))
    urls = {a["name"]: a["url"] for a in rel["assets"]}
    assets = {}
    with tempfile.TemporaryDirectory() as d:
        for _, pattern in PLATFORMS:
            name = pattern.format(v=version)
            if name in urls and name + ".sig" in urls:
                subprocess.check_call(["gh", "release", "download", tag, "--repo", repo, "-p", name + ".sig", "-D", d])
                assets[name] = (urls[name], open(os.path.join(d, name + ".sig")).read())
    data = build(version, open(notes_path, encoding="utf-8").read(), assets,
                 datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"))
    # Sem o Linux (AppImage) o atualizador do dono do projeto não funciona: não publica pela metade.
    for must in ("linux-x86_64", "windows-x86_64", "darwin-aarch64", "darwin-x86_64"):
        if must not in data["platforms"]:
            sys.exit(f"faltam o pacote ou a assinatura de {must}; confira se TAURI_SIGNING_PRIVATE_KEY está nos segredos")
    with open(out, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(f"{out}: {sorted(data['platforms'])}")


if __name__ == "__main__":
    main()
