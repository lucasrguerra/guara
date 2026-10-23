#!/usr/bin/env bash
# Instala as bibliotecas do Tauri no Linux do CI (WebKitGTK e afins) sem depender do espelho do
# apt estar rápido: usa os .deb em cache quando existem e, se o apt travar ou falhar, tenta de novo.
#
# O passo "Dependências do Tauri" já levou de 22 a 28 minutos num dia de espelho lento, contra
# uns 35 s no normal. Aqui cada tentativa tem tempo limite e o cache evita o download.
#
# Uso: scripts/install-tauri-deps.sh [pasta-do-cache]   (padrão: ~/apt-cache)
set -euo pipefail
cache="${1:-$HOME/apt-cache}"
pkgs=(libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf)
mkdir -p "$cache/partial"

# 1) tudo em cache: instala direto, sem nem atualizar o índice do apt
if ls "$cache"/*.deb >/dev/null 2>&1 && timeout 300 sudo apt-get -o "Dir::Cache::archives=$cache" -y install "${pkgs[@]}"; then
  echo "instalado a partir do cache"
  exit 0
fi

# 2) sem cache (ou incompleto): atualiza e instala, até 3 tentativas com tempo limite
for tentativa in 1 2 3; do
  if timeout 240 sudo apt-get update && timeout 600 sudo apt-get -o "Dir::Cache::archives=$cache" -y install "${pkgs[@]}"; then
    echo "instalado (tentativa $tentativa)"
    exit 0
  fi
  echo "apt falhou ou passou do tempo (tentativa $tentativa de 3)" >&2
  sleep $((tentativa * 10))
done
echo "não foi possível instalar as dependências do Tauri" >&2
exit 1
