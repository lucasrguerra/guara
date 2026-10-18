#!/usr/bin/env python3
"""Gera uma imagem FAT32 com muitas fotos JPEG (EXIF de câmeras/datas/GPS) já apagadas.

Uso: make-photo-fixture.py <saida.img> [quantidade=300] [copias=1]
(`copias` repete cada foto N vezes com outro nome: é um jeito rápido de gerar milhares de arquivos.)
Serve para testar a navegação por data/câmera/pasta e as miniaturas.
"""
import os, random, shutil, struct, subprocess, sys, tempfile

out = sys.argv[1]
count = int(sys.argv[2]) if len(sys.argv) > 2 else 300
copies = int(sys.argv[3]) if len(sys.argv) > 3 else 1
rnd = random.Random(42)

def ifd_bytes(entries, start):
    n = len(entries)
    table = struct.pack('<H', n)
    data = b''
    data_off = start + 2 + 12 * n + 4
    for tag, typ, cnt, val in entries:
        if isinstance(val, int):
            vb = struct.pack('<I', val) if typ == 4 else struct.pack('<H', val) + b'\0\0'
        else:
            vb = val
        if len(vb) <= 4:
            table += struct.pack('<HHI', tag, typ, cnt) + vb.ljust(4, b'\0')
        else:
            if len(data) % 2:
                data += b'\0'
            table += struct.pack('<HHII', tag, typ, cnt, data_off + len(data))
            data += vb
    return table + struct.pack('<I', 0) + data

def rat(*pairs):
    return b''.join(struct.pack('<II', a, b) for a, b in pairs)

def make_exif(make, model, dt, gps, orient):
    def build(exif_off, gps_off):
        e0 = [(0x010F, 2, len(make) + 1, make.encode() + b'\0'), (0x0110, 2, len(model) + 1, model.encode() + b'\0'),
              (0x0112, 3, 1, orient), (0x8769, 4, 1, exif_off)]
        if gps:
            e0.append((0x8825, 4, 1, gps_off))
        return ifd_bytes(e0, 8)
    ex = [(0x9003, 2, 20, dt.encode() + b'\0')]
    first = build(0, 0)
    exif_off = 8 + len(first)
    exif_ifd = ifd_bytes(ex, exif_off)
    gps_off = exif_off + len(exif_ifd)
    gps_ifd = b''
    if gps:
        lat, lon = gps
        def dms(v):
            v = abs(v); d = int(v); m = int((v - d) * 60); s = round(((v - d) * 60 - m) * 60 * 100)
            return rat((d, 1), (m, 1), (s, 100))
        g = [(1, 2, 2, b'S\0' if lat < 0 else b'N\0'), (2, 5, 3, dms(lat)), (3, 2, 2, b'W\0' if lon < 0 else b'E\0'), (4, 5, 3, dms(lon))]
        gps_ifd = ifd_bytes(g, gps_off)
    body = build(exif_off, gps_off) + exif_ifd + gps_ifd
    tiff = b'II*\0' + struct.pack('<I', 8) + body
    return b'Exif\0\0' + tiff

def add_exif(jpg_bytes, exif):
    assert jpg_bytes[:2] == b'\xff\xd8'
    seg = b'\xff\xe1' + struct.pack('>H', len(exif) + 2) + exif
    return jpg_bytes[:2] + seg + jpg_bytes[2:]

cams = [('Canon', 'Canon EOS 80D'), ('Apple', 'iPhone 13'), ('samsung', 'SM-G991B'), ('NIKON CORPORATION', 'NIKON D3500')]
places = [(-23.5505, -46.6333), (-8.0476, -34.8770), (-22.9068, -43.1729), (38.7223, -9.1393)]
folders = ['DCIM/100CANON', 'DCIM/Camera', 'WhatsApp/Images', 'Viagens/Recife', 'Viagens/Lisboa', 'Familia']

tmp = tempfile.mkdtemp()
files = []
for i in range(count):
    cam = cams[i % 4]
    y = rnd.choice([2021, 2022, 2023, 2024]); mo = rnd.randint(1, 12); d = rnd.randint(1, 28)
    dt = f'{y}:{mo:02d}:{d:02d} {rnd.randint(7, 22):02d}:{rnd.randint(0,59):02d}:{rnd.randint(0,59):02d}'
    folder = folders[i % len(folders)]
    name = f'IMG_{i:04d}.jpg'
    p = os.path.join(tmp, name)
    w, h = rnd.choice([(480, 320), (320, 480), (640, 360), (400, 400)])
    subprocess.run(['convert', '-seed', str(i), '-size', f'{w}x{h}', 'plasma:fractal', '-gravity', 'center', '-pointsize', '34', '-fill', 'white',
                    '-stroke', 'black', '-strokewidth', '1', '-annotate', '0', f'{y}-{mo:02d}-{d:02d}\n#{i}', '-quality', '82', p], check=True, stderr=subprocess.DEVNULL)
    raw = open(p, 'rb').read()
    if i % 9 != 0:  # alguns sem EXIF
        gps = places[i % 4] if i % 3 == 0 else None
        raw = add_exif(raw, make_exif(cam[0], cam[1], dt, gps, rnd.choice([1, 1, 1, 6, 8])))
    open(p, 'wb').write(raw)
    files.append((folder, name, p))

if copies > 1:
    base = list(files)
    for c in range(1, copies):
        for folder, name, p in base:
            files.append((folder, name.replace('IMG_', f'IMG{c:03d}_'), p))
mb = max(64, len(files) * 40 // 1024 + 48)
if os.path.exists(out):
    os.remove(out)
subprocess.run(['truncate', '-s', f'{mb}M', out], check=True)
subprocess.run(['mkfs.vfat', '-F', '32', out], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
made = set()
for folder, name, p in files:
    cur = ''
    for part in folder.split('/'):
        cur += '/' + part
        if cur not in made:
            subprocess.run(['mmd', '-i', out, '::' + cur], check=True, stderr=subprocess.DEVNULL)
            made.add(cur)
for folder, name, p in files:
    subprocess.run(['mcopy', '-i', out, p, f'::/{folder}/{name}'], check=True)
for folder, name, p in files:
    subprocess.run(['mdel', '-i', out, f'::/{folder}/{name}'], check=True)
shutil.rmtree(tmp)
print(f'ok: {out} ({len(files)} fotos apagadas)')
