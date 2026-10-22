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
mkdir -p "$dest/dfvfs"
for f in lvm.raw apfs.raw apfs_encrypted.dmg hfsplus.raw hfs.raw xfs.raw xfs-bigtime.raw ext4.raw ext2.raw fvdetest.qcow2 ext2.vhd ext2.vhdx ext2.vmdk windows_volume.qcow2 apm.dmg ntfs-dynamic.vhd ntfs-fixed.vhd ntfs-dynamic.vhdx ntfs-fixed.vhdx ntfs-parent.vhd ntfs-differential.vhd ntfs-parent.vhdx ntfs-differential.vhdx fat-parent.vhd fat-differential.vhd fat-parent.vhdx fat-differential.vhdx luks1.raw luks2.raw bdetogo.raw; do
  [ -f "$dest/dfvfs/$f" ] || curl -fsSL "https://raw.githubusercontent.com/log2timeline/dfvfs/main/test_data/$f" -o "$dest/dfvfs/$f"
done
mkdir -p "$dest/aff4"
for f in Base-Linear.aff4 Base-Allocated.aff4 Base-Linear-AllHashes.aff4; do
  [ -f "$dest/aff4/$f" ] || curl -fsSL "https://raw.githubusercontent.com/aff4/ReferenceImages/master/AFF4Std/$f" -o "$dest/aff4/$f"
done
echo "ok: $dest"
