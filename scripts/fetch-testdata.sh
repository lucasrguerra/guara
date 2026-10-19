#!/usr/bin/env bash
# Baixa imagens de referência (BitLocker e FileVault2) do conjunto de testes do cryptsetup.
# Ficam em target/testdata (não versionado). Sem elas, os testes correspondentes são pulados.
set -euo pipefail
dest="$(cd "$(dirname "$0")/.." && pwd)/target/testdata"
mkdir -p "$dest"
base=https://gitlab.com/cryptsetup/cryptsetup/-/raw/main/tests
for n in bitlk-images fvault2-images; do
  [ -d "$dest/$n" ] && continue
  tmp="$(mktemp -d)"
  curl -fsSL "$base/$n.tar.xz" -o "$tmp/$n.tar.xz"
  tar xJf "$tmp/$n.tar.xz" -C "$dest"
  rm -rf "$tmp"
done
echo "ok: $dest"
