#!/usr/bin/env bash
# Instala pacotes do apt no CI sem depender do espelho estar rápido: usa os .deb em cache quando
# existem e, se o apt travar ou falhar, tenta de novo com tempo limite por tentativa.
#
# Os passos de apt do CI já levaram de 13 a 28 minutos num dia de espelho lento, contra uns 35 s
# no normal. O cache evita o download; o tempo limite evita ficar preso.
#
# Uso: scripts/apt-install.sh <pasta-do-cache> <pacote>...
set -euo pipefail
[ $# -ge 2 ] || { echo "uso: $0 <pasta-do-cache> <pacote>..." >&2; exit 2; }
cache="$1"; shift
pkgs=("$@")
mkdir -p "$cache/partial"
# O apt roda como root e deixa arquivos (lock, .deb) que o usuário do runner não lê; sem devolver a
# posse, o actions/cache falha ao empacotar a pasta ("tar failed with exit code 2") e nunca grava.
trap 'sudo chown -R "$(id -u):$(id -g)" "$cache" 2>/dev/null || true; rm -f "$cache/lock"' EXIT
opts=(-o "Dir::Cache::archives=$cache" -y)

# 1) com .deb em cache: instala direto, sem nem atualizar o índice do apt
if ls "$cache"/*.deb >/dev/null 2>&1 && timeout 300 sudo apt-get "${opts[@]}" install "${pkgs[@]}"; then
  echo "instalado a partir do cache"
  exit 0
fi

# 2) sem cache (ou incompleto): atualiza e instala, até 3 tentativas com tempo limite
for tentativa in 1 2 3; do
  if timeout 240 sudo apt-get update && timeout 600 sudo apt-get "${opts[@]}" install "${pkgs[@]}"; then
    echo "instalado (tentativa $tentativa)"
    exit 0
  fi
  echo "apt falhou ou passou do tempo (tentativa $tentativa de 3)" >&2
  sleep $((tentativa * 10))
done
echo "não foi possível instalar: ${pkgs[*]}" >&2
exit 1
