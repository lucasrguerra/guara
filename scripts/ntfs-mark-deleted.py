#!/usr/bin/env python3
"""Marca registros da MFT como apagados (limpa o bit 'em uso') para fixtures de teste.

Uso: ntfs-mark-deleted.py imagem.img caminho/arquivo1 caminho/arquivo2 ...
Só o nome base é usado (ntfscp grava na raiz).
"""
import os
import struct
import sys

img, names = sys.argv[1], {os.path.basename(n) for n in sys.argv[2:]}
f = open(img, "r+b")
boot = f.read(512)
bps = struct.unpack_from("<H", boot, 11)[0]
spc = boot[13]
mft_lcn = struct.unpack_from("<Q", boot, 48)[0]
rs = struct.unpack_from("<b", boot, 64)[0]
rec = rs * bps * spc if rs > 0 else 1 << -rs
f.seek(mft_lcn * bps * spc)
data = f.read(rec * 4096)  # MFT inicial é contígua em imagens pequenas
count = 0
for i in range(len(data) // rec):
    r = bytearray(data[i * rec:(i + 1) * rec])
    if r[:4] != b"FILE" or not (r[22] & 1):
        continue
    p = struct.unpack_from("<H", r, 20)[0]
    while p + 8 <= len(r):
        t, ln = struct.unpack_from("<II", r, p)
        if t == 0xFFFFFFFF or ln == 0:
            break
        if t == 0x30 and r[p + 8] == 0:
            vo = p + struct.unpack_from("<H", r, p + 20)[0]
            nl = r[vo + 0x40]
            name = bytes(r[vo + 0x42:vo + 0x42 + nl * 2]).decode("utf-16le")
            if name in names and r[vo + 0x41] != 2:
                f.seek(mft_lcn * bps * spc + i * rec + 22)
                f.write(bytes([r[22] & ~1 & 0xFF]))
                count += 1
                break
        p += ln
f.flush()
print(f"marcados como apagados: {count}", file=sys.stderr)
