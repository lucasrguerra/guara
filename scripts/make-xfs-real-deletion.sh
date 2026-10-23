#!/usr/bin/env bash
# Cria uma imagem XFS com exclusão REAL (feita pelo kernel), para validar a recuperação de XFS
# sem simulação. Precisa de root, porque monta a imagem por loop: sudo scripts/make-xfs-real-deletion.sh
#
# Saída em target/testdata/xfs-real-delete/: disk.img e expected.tsv (nome, tamanho, sha256 e se
# o arquivo foi apagado). O teste em crates/guara-core/tests/xfs.rs usa esses dois arquivos e é
# pulado quando eles não existem.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "rode com sudo: sudo $0" >&2; exit 1; }
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/target/testdata/xfs-real-delete}"
mkdir -p "$out"
img="$out/disk.img"
rm -f "$img"
truncate -s 320M "$img"   # o mkfs.xfs recente recusa menos de 300 MiB
mkfs.xfs -q -f "$img"
mnt="$(mktemp -d)"
cleanup() { mountpoint -q "$mnt" && umount "$mnt"; rmdir "$mnt" 2>/dev/null || true; }
trap cleanup EXIT
mount -o loop "$img" "$mnt"
python3 - "$mnt" "$out/expected.tsv" "$root/crates/guara-core/tests/data/exemplo.sqlite" <<'PY'
import hashlib, os, sys
mnt, tsv, sqlite = sys.argv[1], sys.argv[2], sys.argv[3]
def data(seed, n):
    x, out = seed | 1, bytearray()
    for _ in range(n):
        x ^= (x << 13) & 0xFFFFFFFFFFFFFFFF; x ^= x >> 7; x ^= (x << 17) & 0xFFFFFFFFFFFFFFFF
        out.append((x >> 24) & 0xFF)
    return bytes(out)
files = [  # (caminho, tamanho, semente, apagar?)
    ("docs/relatorio.dat", 150000, 5, True),
    ("docs/mantido.dat", 90000, 6, False),
    ("fotos/foto1.bin", 300000, 7, True),
    ("fotos/foto2.bin", 40000, 8, False),
    ("raiz-apagada.txt", 20000, 9, True),
]
# um arquivo com estrutura reconhecível (SQLite), para a recuperação por carving
real = open(sqlite, "rb").read()
rows = []
files.append(("dados/exemplo.sqlite", len(real), 0, True))
for rel, n, seed, gone in files:
    p = os.path.join(mnt, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    d = real if rel.endswith('.sqlite') else data(seed, n)
    with open(p, "wb") as f:
        f.write(d)
    rows.append((rel, n, hashlib.sha256(d).hexdigest(), "deleted" if gone else "live"))
os.sync()
for rel, n, seed, gone in files:
    if gone:
        os.remove(os.path.join(mnt, rel))
os.sync()
with open(tsv, "w") as f:
    for r in rows:
        f.write("\t".join(map(str, r)) + "\n")
PY
sync
umount "$mnt"
[ -n "${SUDO_UID:-}" ] && chown -R "$SUDO_UID:${SUDO_GID:-$SUDO_UID}" "$out"
echo "ok: $img"
