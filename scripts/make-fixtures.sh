#!/usr/bin/env bash
# Gera imagens de disco de teste (FAT32, exFAT, NTFS, ext4) com arquivos
# conhecidos, apaga alguns e grava os SHA-256 esperados em <saida>/expected-<fs>.txt.
#
# Uso: scripts/make-fixtures.sh <diretório-de-saída> [fat32 exfat ntfs ext4]
# Requer (Linux): mtools, dosfstools, exfatprogs/exfat-fuse, ntfs-3g, e2fsprogs,
# imagemagick, zip, python3 e FUSE para exFAT/NTFS.
set -euo pipefail

OUT="$(realpath -m "${1:?uso: $0 <saida> [fs...]}")"
shift || true
FSES=("${@:-fat32 exfat ntfs ext4}")
[ $# -eq 0 ] && FSES=(fat32 exfat ntfs ext4)
mkdir -p "$OUT"
SIZE_MB=96

CONTENT="$OUT/content"
rm -rf "$CONTENT"; mkdir -p "$CONTENT/Documentos/Trabalho" "$CONTENT/Fotos"

# --- conteúdo de teste (mistura de formatos com e sem assinatura) ---
convert -size 1400x900 plasma:fractal -quality 92 "$CONTENT/Fotos/praia.jpg"
convert -size 900x700 plasma:red-blue "$CONTENT/Fotos/montanha.png"
convert -size 600x400 plasma:green-yellow "$CONTENT/Fotos/mantida.jpg"
head -c 700000 /dev/urandom | base64 > "$CONTENT/Documentos/Trabalho/notas.txt"
head -c 3000000 /dev/urandom > "$CONTENT/Documentos/dados.bin"
python3 - "$CONTENT" <<'PY'
import sys, sqlite3, os
d = sys.argv[1]
pdf = b"%PDF-1.4\n" + b"".join(b"%% linha %d preenchimento %s\n" % (i, os.urandom(40).hex().encode()) for i in range(6000)) + b"trailer\n<< /Root 1 0 R >>\n%%EOF\n"
open(d + "/Documentos/Trabalho/relatorio.pdf", "wb").write(pdf)
db = d + "/Documentos/contatos.db"
c = sqlite3.connect(db)
c.execute("create table t(id integer primary key, v text)")
c.executemany("insert into t(v) values (?)", [(os.urandom(64).hex(),) for _ in range(4000)])
c.commit(); c.close()
PY
mkdir -p "$OUT/zip" && ( cd "$OUT/zip" && rm -rf * && mkdir -p word _rels && echo '<?xml version="1.0"?><Types/>' > '[Content_Types].xml' \
  && head -c 400000 /dev/urandom | base64 > word/document.xml && zip -q -r "$CONTENT/Documentos/Trabalho/tese.docx" . )
rm -rf "$OUT/zip"

# Arquivos que serão APAGADOS (caminhos relativos a $CONTENT)
DELETE=(Fotos/praia.jpg Fotos/montanha.png Documentos/Trabalho/notas.txt Documentos/Trabalho/relatorio.pdf Documentos/Trabalho/tese.docx Documentos/contatos.db Documentos/dados.bin)

write_expected() { # $1=fs
  : > "$OUT/expected-$1.txt"
  for f in "${DELETE[@]}"; do
    echo "$(sha256sum "$CONTENT/$f" | cut -d' ' -f1) $f" >> "$OUT/expected-$1.txt"
  done
}

mnt_cleanup() { fusermount -u "$1" 2>/dev/null || true; rmdir "$1" 2>/dev/null || true; }

for fs in "${FSES[@]}"; do
  img="$OUT/$fs.img"
  rm -f "$img"; truncate -s ${SIZE_MB}M "$img"
  case $fs in
    fat32)
      mkfs.vfat -F 32 -n GUARATEST "$img" >/dev/null
      mcopy -i "$img" -s -m "$CONTENT"/* ::
      for f in "${DELETE[@]}"; do mdel -i "$img" "::/$f"; done
      ;;
    exfat)
      # Sem FUSE sem privilégio, só o sistema vazio é criado aqui; o teste em Rust
      # injeta as entradas (veja crates/guara-core/tests/exfat.rs).
      mkfs.exfat -L GUARATEST "$img" >/dev/null
      ;;
    ntfs)
      # ntfscp só grava na raiz; "apagar" = limpar o bit "em uso" do registro da MFT,
      # exatamente o que o NTFS faz (clusters e nome permanecem no disco).
      mkntfs -F -Q -L GUARATEST "$img" >/dev/null 2>&1
      for f in $(cd "$CONTENT" && find . -type f | sed 's|^\./||'); do
        ntfscp -f "$img" "$CONTENT/$f" "/$(basename "$f")" >/dev/null 2>&1
      done
      python3 "$(dirname "$0")/ntfs-mark-deleted.py" "$img" "${DELETE[@]}"
      ;;
    ext4)
      mkfs.ext4 -q -F -L GUARATEST -d "$CONTENT" "$img"
      for f in "${DELETE[@]}"; do debugfs -w -R "rm /$f" "$img" >/dev/null 2>&1; done
      ;;
  esac
  write_expected "$fs"
  echo "ok: $img"
done
