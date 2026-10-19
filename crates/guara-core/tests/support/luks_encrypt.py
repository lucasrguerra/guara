#!/usr/bin/env python3
"""Cifra um volume em claro dentro de uma imagem LUKS, com cryptography (referência independente).
uso: luks_encrypt.py IMG PLAIN VOLUME_KEY_FILE PAYLOAD_OFFSET CIPHER SECTOR_SIZE"""
import sys, hashlib
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

img, plain, vkf, off, cipher, ss = sys.argv[1:7]
off = int(off); ss = int(ss)
vk = open(vkf, 'rb').read()
data = open(plain, 'rb').read()
data += bytes((-len(data)) % ss)
out = bytearray()
for i in range(0, len(data), ss):
    sector512 = i // 512
    blk = data[i:i+ss]
    if 'xts' in cipher:
        tweak = sector512.to_bytes(16, 'little')
        e = Cipher(algorithms.AES(vk), modes.XTS(tweak)).encryptor()
    else:
        if 'essiv:sha256' in cipher:
            salt = hashlib.sha256(vk).digest()
            iv = Cipher(algorithms.AES(salt), modes.ECB()).encryptor().update(sector512.to_bytes(16, 'little'))
        else:
            iv = sector512.to_bytes(16, 'little')
        e = Cipher(algorithms.AES(vk), modes.CBC(iv)).encryptor()
    out += e.update(blk) + e.finalize()
with open(img, 'r+b') as f:
    f.seek(off)
    f.write(out)
