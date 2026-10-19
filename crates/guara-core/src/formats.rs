//! Mais formatos reconhecidos pela estrutura (cabeçalhos, blocos e tamanhos internos), para o
//! carving devolver o arquivo inteiro e não só o começo. Cada detector devolve o tamanho exato
//! quando a estrutura permite e uma confiança proporcional ao que foi conferido.

use crate::signatures::Hit;
use crate::source::{read_vec, Source};
use crate::types::Confidence;

fn u16le(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]) as u64)
        .unwrap_or(0)
}
fn u32le(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()) as u64)
        .unwrap_or(0)
}
fn u64le(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}
fn u32be(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 4)
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()) as u64)
        .unwrap_or(0)
}
fn u16be(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]) as u64)
        .unwrap_or(0)
}

fn hit(len: u64, ext: &'static str, confidence: Confidence) -> Option<Hit> {
    Some(Hit {
        len,
        ext,
        confidence,
    })
}

/// Primeiros bytes que disparam os detectores deste módulo.
pub fn might_start(b0: u8) -> bool {
    matches!(
        b0,
        0x1F | b'B'
            | 0x1A
            | b'O'
            | b'R'
            | 0x7F
            | b'I'
            | b'M'
            | 0x00
            | b'r'
            | b'E'
            | 0x47
            | b'!'
            | b'F'
            | b'C'
            | b'Z'
            | 0x30
            | b'{'
            | 0x28
            | 0xD4
            | 0x0A
    )
}

/// Tenta os formatos adicionais. `h` são os primeiros bytes (>= 12) lidos em `off`.
pub fn detect(src: &dyn Source, off: u64, h: &[u8], max: u64) -> Option<Hit> {
    if h.starts_with(&[0x1F, 0x8B, 0x08]) {
        return gzip(src, off, max);
    }
    if h.starts_with(b"BZh")
        && (b'1'..=b'9').contains(&h[3])
        && h[4..10] == [0x31, 0x41, 0x59, 0x26, 0x53, 0x59]
    {
        return bzip2(src, off, max);
    }
    if h.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return matroska(src, off, max);
    }
    if h.starts_with(b"OggS") {
        return ogg(src, off, max);
    }
    if h.starts_with(b"Rar!\x1A\x07") {
        return rar(src, off, max);
    }
    if h.starts_with(&[0x7F, b'E', b'L', b'F']) {
        return elf(src, off, max);
    }
    if h.starts_with(&[b'I', b'I', 42, 0]) || h.starts_with(&[b'M', b'M', 0, 42]) {
        return tiff(src, off, max);
    }
    if h.starts_with(&[0, 0, 1, 0]) && h[4] > 0 && h[4] < 64 {
        return ico(src, off, max);
    }
    if h.starts_with(b"regf") {
        return regf(src, off, max);
    }
    if h.starts_with(b"ElfFile\0") {
        return evtx(src, off);
    }
    if h[0] == 0x47 && h.len() >= 12 {
        return mpegts(src, off, max);
    }
    if h.starts_with(b"MSCF") {
        let len = u32le(h, 8);
        return (len > 36 && len <= max)
            .then_some(())
            .and_then(|_| hit(len, "cab", Confidence::High));
    }
    if h.starts_with(b"!<arch>\n") {
        return ar(src, off, max);
    }
    if h.starts_with(b"FWS") || h.starts_with(b"CWS") {
        return swf(src, off, max);
    }
    if h.starts_with(&[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11]) {
        return asf(src, off, max);
    }
    if h.starts_with(b"!BDN") {
        return pst(src, off, max);
    }
    if h.starts_with(b"FLV\x01") {
        return flv(src, off, max);
    }
    if h.starts_with(&[0, 0, 0, 0x0C, b'j', b'P', b' ', b' ']) {
        return jp2(src, off, max);
    }
    if h.starts_with(b"{\\rtf") {
        return rtf(src, off, max);
    }
    if h.starts_with(b"MThd") {
        return midi(src, off, max);
    }
    if h.starts_with(b"FORM") && matches!(&h[8..12], b"AIFF" | b"AIFC") {
        let len = u32be(h, 4) + 8;
        return (len <= max)
            .then_some(())
            .and_then(|_| hit(len, "aif", Confidence::High));
    }
    if h.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return zstd(src, off, max);
    }
    if h.starts_with(&[0xD4, 0xC3, 0xB2, 0xA1]) || h.starts_with(&[0xA1, 0xB2, 0xC3, 0xD4]) {
        return pcap(src, off, max);
    }
    if h.starts_with(&[0x0A, 0x0D, 0x0D, 0x0A]) {
        return pcapng(src, off, max);
    }
    None
}

// -------------------------------------------------------------- compactadores

/// Alimenta um descompactador com blocos de `src` até o fim do fluxo; devolve (bytes lidos, saída crc32 opcional).
fn inflate_len(
    src: &dyn Source,
    start: u64,
    max: u64,
    raw: bool,
    zlib_header: bool,
) -> Option<(u64, u32, u64)> {
    let mut d = flate2::Decompress::new(zlib_header && !raw);
    let mut pos = start;
    let mut out = vec![0u8; 1 << 16];
    let mut crc = 0xFFFF_FFFFu32;
    let mut total_out = 0u64;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        if pos - start > max {
            return None;
        }
        let n = src.read_at(pos, &mut buf).ok()?;
        if n == 0 {
            return None;
        }
        let mut consumed = 0usize;
        while consumed < n {
            let before_in = d.total_in();
            let before_out = d.total_out();
            let status = d
                .decompress(&buf[consumed..n], &mut out, flate2::FlushDecompress::None)
                .ok()?;
            let used = (d.total_in() - before_in) as usize;
            let produced = (d.total_out() - before_out) as usize;
            for &b in &out[..produced] {
                crc ^= b as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0xEDB8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            total_out += produced as u64;
            consumed += used;
            if status == flate2::Status::StreamEnd {
                return Some((pos - start + consumed as u64, !crc, total_out));
            }
            if used == 0 && produced == 0 {
                return None;
            }
        }
        pos += n as u64;
    }
}

fn gzip(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 4096).ok()?;
    let flags = *h.get(3)?;
    let mut p = 10usize;
    if flags & 4 != 0 {
        p += 2 + u16le(&h, p) as usize;
    }
    for bit in [8u8, 16u8] {
        if flags & bit != 0 {
            while *h.get(p)? != 0 {
                p += 1;
            }
            p += 1;
        }
    }
    if flags & 2 != 0 {
        p += 2;
    }
    let (consumed, crc, out_len) = inflate_len(src, off + p as u64, max.min(2 << 30), true, false)?;
    let trailer = read_vec(src, off + p as u64 + consumed, 8).ok()?;
    if trailer.len() < 8 {
        return None;
    }
    let ok = u32le(&trailer, 0) as u32 == crc && u32le(&trailer, 4) == out_len & 0xFFFF_FFFF;
    hit(
        p as u64 + consumed + 8,
        "gz",
        if ok {
            Confidence::High
        } else {
            Confidence::Medium
        },
    )
}

fn bzip2(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let mut d = bzip2::Decompress::new(false);
    let mut pos = off;
    let mut buf = vec![0u8; 1 << 16];
    let mut out = vec![0u8; 1 << 16];
    loop {
        if pos - off > max.min(2 << 30) {
            return None;
        }
        let n = src.read_at(pos, &mut buf).ok()?;
        if n == 0 {
            return None;
        }
        let mut consumed = 0usize;
        while consumed < n {
            let (bi, bo) = (d.total_in(), d.total_out());
            let status = d.decompress(&buf[consumed..n], &mut out).ok()?;
            let used = (d.total_in() - bi) as usize;
            consumed += used;
            if status == bzip2::Status::StreamEnd {
                return hit(pos - off + consumed as u64, "bz2", Confidence::High);
            }
            if used == 0 && d.total_out() == bo {
                return None;
            }
        }
        pos += n as u64;
    }
}

fn zstd(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 32).ok()?;
    let fhd = *h.get(4)?;
    let single = fhd & 0x20 != 0;
    let checksum = fhd & 0x04 != 0;
    let dict = match fhd & 3 {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    let fcs = match fhd >> 6 {
        0 => {
            if single {
                1
            } else {
                0
            }
        }
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let mut p = 5 + if single { 0 } else { 1 } + dict + fcs;
    let mut pos = off + p as u64;
    loop {
        let bh = read_vec(src, pos, 3).ok()?;
        if bh.len() < 3 {
            return None;
        }
        let v = bh[0] as u64 | (bh[1] as u64) << 8 | (bh[2] as u64) << 16;
        let last = v & 1 != 0;
        let btype = (v >> 1) & 3;
        let bsize = v >> 3;
        if btype == 3 {
            return None;
        }
        // RLE: o conteúdo é 1 byte; os outros ocupam `bsize`.
        let body = if btype == 1 { 1 } else { bsize };
        pos += 3 + body;
        if pos - off > max {
            return None;
        }
        if last {
            break;
        }
    }
    if checksum {
        pos += 4;
    }
    p = (pos - off) as usize;
    hit(p as u64, "zst", Confidence::Medium)
}

// ------------------------------------------------------------------ mídia

fn vint(b: &[u8]) -> Option<(u64, usize, bool)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || b.len() < len {
        return None;
    }
    let mut v = (first as u64) & ((1u64 << (8 - len)) - 1);
    let mut all_ones = v == (1u64 << (8 - len)) - 1;
    for &x in &b[1..len] {
        v = (v << 8) | x as u64;
        all_ones &= x == 0xFF;
    }
    Some((v, len, all_ones))
}

fn matroska(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 4096).ok()?;
    let (hs, hl, _) = vint(&h[4..])?;
    let mut p = 4 + hl + hs as usize;
    let doctype =
        String::from_utf8_lossy(&h[4 + hl..(4 + hl + hs as usize).min(h.len())]).to_string();
    let ext = if doctype.contains("webm") {
        "webm"
    } else {
        "mkv"
    };
    if h.get(p..p + 4)? != [0x18, 0x53, 0x80, 0x67] {
        return None;
    }
    p += 4;
    let (ss, sl, unknown) = vint(h.get(p..)?)?;
    if !unknown {
        let total = p as u64 + sl as u64 + ss;
        if total <= max {
            return hit(total, ext, Confidence::High);
        }
    }
    // Tamanho desconhecido (gravação ao vivo): caminha pelos elementos de topo.
    let mut pos = off + (p + sl) as u64;
    let mut last_good = pos;
    for _ in 0..2_000_000 {
        let e = read_vec(src, pos, 16).ok()?;
        if e.len() < 6 {
            break;
        }
        let Some((id, idl, _)) = (e[0] >= 0x10).then(|| vint_id(&e)).flatten() else {
            break;
        };
        let _ = id;
        let Some((size, sl2, unk)) = vint(&e[idl..]) else {
            break;
        };
        if unk {
            break;
        }
        pos += (idl + sl2) as u64 + size;
        if pos - off > max {
            break;
        }
        last_good = pos;
    }
    hit(last_good - off, ext, Confidence::Low)
}

/// ID de elemento EBML (mantém os bits de marcação).
fn vint_id(b: &[u8]) -> Option<(u64, usize, bool)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 || b.len() < len {
        return None;
    }
    let mut v = 0u64;
    for &x in &b[..len] {
        v = (v << 8) | x as u64;
    }
    Some((v, len, false))
}

fn ogg(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let mut pos = off;
    let mut eos = false;
    let mut pages = 0u32;
    let mut serials: Vec<u32> = vec![];
    let mut done_serials = 0;
    loop {
        let h = read_vec(src, pos, 27).ok()?;
        if h.len() < 27 || &h[0..4] != b"OggS" || h[4] != 0 {
            break;
        }
        let nseg = h[26] as usize;
        let lac = read_vec(src, pos + 27, nseg).ok()?;
        if lac.len() < nseg {
            break;
        }
        let body: u64 = lac.iter().map(|&x| x as u64).sum();
        let serial = u32le(&h, 14) as u32;
        if !serials.contains(&serial) {
            serials.push(serial);
        }
        if h[5] & 4 != 0 {
            done_serials += 1;
            eos = done_serials >= serials.len();
        }
        pos += 27 + nseg as u64 + body;
        pages += 1;
        if pos - off > max {
            return None;
        }
        if eos {
            break;
        }
    }
    (pages >= 2).then_some(()).and_then(|_| {
        hit(
            pos - off,
            "ogg",
            if eos {
                Confidence::High
            } else {
                Confidence::Low
            },
        )
    })
}

fn mpegts(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    // 188 bytes por pacote (ou 192 com carimbo de 4 bytes antes do sync).
    let probe = read_vec(src, off, 188 * 8).ok()?;
    if probe.len() < 188 * 8 || !(0..8).all(|i| probe[i * 188] == 0x47) {
        return None;
    }
    let mut pos = off;
    let mut buf = vec![0u8; 188 * 4096];
    let mut n_packets = 0u64;
    'outer: loop {
        let n = src.read_at(pos, &mut buf).ok()?;
        for i in 0..n / 188 {
            if buf[i * 188] != 0x47 {
                break 'outer;
            }
            n_packets += 1;
        }
        pos += (n / 188 * 188) as u64;
        if n < 188 || pos - off > max {
            break;
        }
    }
    let len = n_packets * 188;
    (n_packets >= 16)
        .then_some(())
        .and_then(|_| hit(len, "ts", Confidence::Medium))
}

fn flv(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 9).ok()?;
    let mut pos = off + u32be(&h, 5).max(9);
    let mut tags = 0;
    loop {
        let t = read_vec(src, pos, 15).ok()?;
        if t.len() < 15 {
            break;
        }
        // PreviousTagSize(4) + tipo(1) + tamanho(3)
        let ty = t[4];
        if !matches!(ty, 8 | 9 | 18) {
            break;
        }
        let size = (t[5] as u64) << 16 | (t[6] as u64) << 8 | t[7] as u64;
        pos += 4 + 11 + size;
        tags += 1;
        if pos - off > max {
            return None;
        }
    }
    (tags > 3)
        .then_some(())
        .and_then(|_| hit(pos + 4 - off, "flv", Confidence::Medium))
}

fn asf(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 30).ok()?;
    let hdr_size = u64le(&h, 16);
    let n_objs = u32le(&h, 24);
    if !(30..=1 << 24).contains(&hdr_size) {
        return None;
    }
    let all = read_vec(src, off + 30, (hdr_size - 30) as usize).ok()?;
    let mut p = 0usize;
    for _ in 0..n_objs {
        if p + 24 > all.len() {
            break;
        }
        let size = u64le(&all, p + 16) as usize;
        if all[p..p + 16]
            == [
                0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11, 0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20,
                0x53, 0x65,
            ]
        {
            let fsize = u64le(&all, p + 40);
            if fsize >= hdr_size && fsize <= max {
                return hit(fsize, "wmv", Confidence::High);
            }
        }
        if size < 24 {
            break;
        }
        p += size;
    }
    None
}

fn jp2(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let mut pos = off;
    for _ in 0..64 {
        let b = read_vec(src, pos, 16).ok()?;
        if b.len() < 8 {
            return None;
        }
        let mut size = u32be(&b, 0);
        let ty = &b[4..8];
        let hdr = if size == 1 {
            size = u64::from_be_bytes(b.get(8..16)?.try_into().ok()?);
            16
        } else {
            8
        };
        if ty == b"jp2c" {
            if size == 0 {
                // até o fim do arquivo: procura EOC FF D9 depois
                return hit((pos - off) + 8 + (4 << 20).min(max), "jp2", Confidence::Low);
            }
            return hit(pos - off + size, "jp2", Confidence::High);
        }
        if size < hdr as u64 || pos - off + size > max {
            return None;
        }
        pos += size;
    }
    None
}

fn midi(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 14).ok()?;
    let ntrk = u16be(&h, 10);
    let mut pos = off + 8 + u32be(&h, 4);
    for _ in 0..ntrk {
        let t = read_vec(src, pos, 8).ok()?;
        if t.len() < 8 || &t[0..4] != b"MTrk" {
            return None;
        }
        pos += 8 + u32be(&t, 4);
        if pos - off > max {
            return None;
        }
    }
    hit(pos - off, "mid", Confidence::High)
}

// -------------------------------------------------------- arquivos compactados

fn rar(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 8).ok()?;
    if h.len() < 8 {
        return None;
    }
    let mut pos;
    if h[6] == 0 {
        // RAR 1.5-4.x
        pos = off + 7;
        for _ in 0..1_000_000 {
            let b = read_vec(src, pos, 11).ok()?;
            if b.len() < 7 {
                return None;
            }
            let ty = b[2];
            let flags = u16le(&b, 3);
            let hsize = u16le(&b, 5);
            if hsize < 7 {
                return None;
            }
            let add = if flags & 0x8000 != 0 { u32le(&b, 7) } else { 0 };
            pos += hsize + add;
            if pos - off > max {
                return None;
            }
            if ty == 0x7B {
                return hit(pos - off, "rar", Confidence::High);
            }
        }
        None
    } else if h[6] == 1 && h[7] == 0 {
        // RAR 5
        pos = off + 8;
        for _ in 0..1_000_000 {
            let b = read_vec(src, pos, 64).ok()?;
            if b.len() < 8 {
                return None;
            }
            let mut i = 4;
            let (hsize, l1) = rar_vint(&b[i..])?;
            i += l1;
            let hdr_start = i;
            let (ty, l2) = rar_vint(&b[i..])?;
            i += l2;
            let (flags, l3) = rar_vint(&b[i..])?;
            i += l3;
            if flags & 1 != 0 {
                let (_, l) = rar_vint(&b[i..])?;
                i += l;
            }
            let mut data = 0;
            if flags & 2 != 0 {
                let (d, _) = rar_vint(&b[i..])?;
                data = d;
            }
            pos += 4 + l1 as u64 + hsize + data;
            let _ = hdr_start;
            if pos - off > max {
                return None;
            }
            if ty == 5 {
                return hit(pos - off, "rar", Confidence::High);
            }
        }
        None
    } else {
        None
    }
}

fn rar_vint(b: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for (i, &x) in b.iter().enumerate().take(10) {
        v |= ((x & 0x7F) as u64) << (7 * i);
        if x & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

fn ar(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let mut pos = off + 8;
    let mut members = 0;
    loop {
        let h = read_vec(src, pos, 60).ok()?;
        if h.len() < 60 || &h[58..60] != b"`\n" {
            break;
        }
        let size: u64 = String::from_utf8_lossy(&h[48..58]).trim().parse().ok()?;
        pos += 60 + size + (size & 1);
        members += 1;
        if pos - off > max {
            return None;
        }
    }
    (members >= 2)
        .then_some(())
        .and_then(|_| hit(pos - off, "deb", Confidence::Medium))
}

fn swf(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 16).ok()?;
    let len = u32le(&h, 4);
    if h[3] > 40 {
        return None;
    }
    if h[0] == b'F' {
        return (len > 21 && len <= max)
            .then_some(())
            .and_then(|_| hit(len, "swf", Confidence::High));
    }
    // CWS: corpo em zlib a partir do byte 8
    let (consumed, _, out) = inflate_len(src, off + 8, max, false, true)?;
    hit(
        8 + consumed,
        "swf",
        if out + 8 == len {
            Confidence::High
        } else {
            Confidence::Medium
        },
    )
}

// ---------------------------------------------------------- binários e dados

fn elf(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 64).ok()?;
    if h.len() < 52 {
        return None;
    }
    let is64 = h[4] == 2;
    let le = h[5] == 1;
    let rd = |o: usize, n: usize| -> u64 {
        let b = &h[o..o + n];
        let mut v = 0u64;
        for i in 0..n {
            let x = if le { b[n - 1 - i] } else { b[i] };
            v = (v << 8) | x as u64;
        }
        v
    };
    if h[6] != 1 {
        return None;
    }
    let (shoff, shentsize, shnum, phoff, phentsize, phnum) = if is64 {
        (
            rd(40, 8),
            rd(58, 2),
            rd(60, 2),
            rd(32, 8),
            rd(54, 2),
            rd(56, 2),
        )
    } else {
        (
            rd(32, 4),
            rd(46, 2),
            rd(48, 2),
            rd(28, 4),
            rd(42, 2),
            rd(44, 2),
        )
    };
    let mut end = shoff + shentsize * shnum;
    // Segmentos de programa podem ir além (binários sem tabela de seções).
    if phnum > 0 && phnum < 256 && phentsize >= 32 {
        let ph = read_vec(src, off + phoff, (phnum * phentsize) as usize).ok()?;
        for i in 0..phnum as usize {
            let e = &ph[i * phentsize as usize..];
            let rd2 = |o: usize, n: usize| -> u64 {
                let mut v = 0u64;
                for k in 0..n {
                    let x = if le { e[o + n - 1 - k] } else { e[o + k] };
                    v = (v << 8) | x as u64;
                }
                v
            };
            let (po, fs) = if is64 {
                (rd2(8, 8), rd2(32, 8))
            } else {
                (rd2(4, 4), rd2(16, 4))
            };
            end = end.max(po + fs);
        }
    }
    (end > 64 && end <= max.min(4 << 30))
        .then_some(())
        .and_then(|_| {
            hit(
                end,
                "elf",
                if shoff > 0 {
                    Confidence::High
                } else {
                    Confidence::Medium
                },
            )
        })
}

fn tiff(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 16).ok()?;
    let le = h[0] == b'I';
    let r16 = |b: &[u8], o: usize| -> u64 {
        let s = b.get(o..o + 2).unwrap_or(&[0, 0]);
        if le {
            u16::from_le_bytes([s[0], s[1]]) as u64
        } else {
            u16::from_be_bytes([s[0], s[1]]) as u64
        }
    };
    let r32 = |b: &[u8], o: usize| -> u64 {
        let s: [u8; 4] = b
            .get(o..o + 4)
            .and_then(|x| x.try_into().ok())
            .unwrap_or([0; 4]);
        if le {
            u32::from_le_bytes(s) as u64
        } else {
            u32::from_be_bytes(s) as u64
        }
    };
    let mut ifd = r32(&h, 4);
    let mut end = 8u64;
    let mut subs: Vec<u64> = vec![];
    let mut seen = 0;
    let mut visited: Vec<u64> = vec![];
    while ifd != 0 && ifd < max && seen < 64 {
        if visited.contains(&ifd) {
            break;
        }
        visited.push(ifd);
        let d = read_vec(src, off + ifd, 2).ok()?;
        let n = r16(&d, 0) as usize;
        if n == 0 || n > 4096 {
            return None;
        }
        let e = read_vec(src, off + ifd + 2, n * 12 + 4).ok()?;
        if e.len() < n * 12 + 4 {
            return None;
        }
        end = end.max(ifd + 2 + n as u64 * 12 + 4);
        let mut strip_off: Vec<u64> = vec![];
        let mut strip_cnt: Vec<u64> = vec![];
        for i in 0..n {
            let en = &e[i * 12..];
            let (tag, ty, count) = (r16(en, 0), r16(en, 2), r32(en, 4));
            let sz = match ty {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => return None,
            };
            let total = sz * count;
            let value_off = r32(en, 8);
            if total > 4 {
                end = end.max(value_off + total);
            }
            let read_vals = |count: u64| -> Vec<u64> {
                let total = sz * count;
                let mut out = vec![];
                if count == 0 || count > 1 << 20 {
                    return out;
                }
                let bytes: Vec<u8> = if total <= 4 {
                    en[8..8 + total as usize].to_vec()
                } else {
                    read_vec(src, off + value_off, total as usize).unwrap_or_default()
                };
                for k in 0..count as usize {
                    let v = match sz {
                        2 => r16(&bytes, k * 2),
                        4 => r32(&bytes, k * 4),
                        _ => 0,
                    };
                    out.push(v);
                }
                out
            };
            match tag {
                273 | 324 | 513 => {
                    strip_off = if tag == 513 {
                        vec![r32(en, 8)]
                    } else {
                        read_vals(count)
                    }
                }
                279 | 325 | 514 => {
                    strip_cnt = if tag == 514 {
                        vec![r32(en, 8)]
                    } else {
                        read_vals(count)
                    }
                }
                330 | 34665 | 34853 | 40965 => {
                    if ty == 4 && count == 1 {
                        subs.push(value_off);
                    } else if ty == 4 {
                        for v in read_vals(count) {
                            subs.push(v);
                        }
                    }
                }
                _ => {}
            }
            if (tag == 273 || tag == 324 || tag == 513)
                && !strip_off.is_empty()
                && !strip_cnt.is_empty()
            {
                for (o, c) in strip_off.iter().zip(&strip_cnt) {
                    end = end.max(o + c);
                }
            }
        }
        for (o, c) in strip_off.iter().zip(&strip_cnt) {
            end = end.max(o + c);
        }
        seen += 1;
        ifd = if let Some(s) = subs.pop() {
            s
        } else {
            r32(&e, n * 12)
        };
        if end > max {
            return None;
        }
    }
    let ext = if h.get(8..10) == Some(b"CR") {
        "cr2"
    } else {
        "tif"
    };
    (end > 16)
        .then_some(())
        .and_then(|_| hit(end, ext, Confidence::Medium))
}

fn ico(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 6).ok()?;
    let n = u16le(&h, 4) as usize;
    let d = read_vec(src, off + 6, n * 16).ok()?;
    if d.len() < n * 16 {
        return None;
    }
    let mut end = 6 + n as u64 * 16;
    for i in 0..n {
        let e = &d[i * 16..];
        let (size, o) = (u32le(e, 8), u32le(e, 12));
        if size == 0 || o < 6 + n as u64 * 16 || o + size > max.min(64 << 20) {
            return None;
        }
        end = end.max(o + size);
    }
    // a imagem de cada entrada: BMP/DIB (tamanho do cabeçalho 40) ou PNG
    let first_off = u32le(&d, 12);
    let probe = read_vec(src, off + first_off, 8).ok()?;
    if !(probe.starts_with(&[0x89, b'P', b'N', b'G']) || u32le(&probe, 0) == 40) {
        return None;
    }
    hit(end, "ico", Confidence::High)
}

fn regf(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 4096).ok()?;
    if h.len() < 4096 {
        return None;
    }
    // soma XOR dos 127 primeiros u32 = valor em 0x1FC
    let mut x = 0u32;
    for i in 0..127 {
        x ^= u32le(&h, i * 4) as u32;
    }
    let ok = x == u32le(&h, 0x1FC) as u32;
    let len = 4096 + u32le(&h, 0x28);
    (len > 4096 && len <= max).then_some(()).and_then(|_| {
        hit(
            len,
            "dat",
            if ok {
                Confidence::High
            } else {
                Confidence::Medium
            },
        )
    })
}

fn evtx(src: &dyn Source, off: u64) -> Option<Hit> {
    let h = read_vec(src, off, 4096).ok()?;
    if h.len() < 4096 {
        return None;
    }
    let chunks = u16le(&h, 0x2A);
    let hdr_size = u32le(&h, 0x20);
    if hdr_size != 128 || chunks == 0 {
        return None;
    }
    hit(4096 + 65536 * chunks, "evtx", Confidence::High)
}

fn pst(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 0x200).ok()?;
    if h.len() < 0x200 || &h[8..10] != b"SM" {
        return None;
    }
    let ver = u16le(&h, 10);
    let len = if ver >= 23 {
        u64le(&h, 0xB8)
    } else {
        u32le(&h, 0xA8)
    };
    (len > 0x200 && len <= max)
        .then_some(())
        .and_then(|_| hit(len, "pst", Confidence::High))
}

fn rtf(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let mut depth = 0i64;
    let mut pos = off;
    let mut buf = vec![0u8; 1 << 16];
    let mut escape = false;
    loop {
        let n = src.read_at(pos, &mut buf).ok()?;
        if n == 0 {
            return None;
        }
        for (i, &b) in buf[..n].iter().enumerate() {
            if b == 0 {
                return None;
            }
            if escape {
                escape = false;
                continue;
            }
            match b {
                b'\\' => escape = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return hit(pos - off + i as u64 + 1, "rtf", Confidence::High);
                    }
                }
                _ => {}
            }
        }
        pos += n as u64;
        if pos - off > max.min(512 << 20) {
            return None;
        }
    }
}

fn pcap(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 24).ok()?;
    let le = h[0] == 0xD4;
    let r32 = |b: &[u8], o: usize| -> u64 {
        let s: [u8; 4] = b[o..o + 4].try_into().unwrap();
        if le {
            u32::from_le_bytes(s) as u64
        } else {
            u32::from_be_bytes(s) as u64
        }
    };
    let snaplen = r32(&h, 16);
    if snaplen == 0 || snaplen > 1 << 20 {
        return None;
    }
    let mut pos = off + 24;
    let mut n = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    let mut have = 0usize;
    let mut base = pos;
    loop {
        let rel = (pos - base) as usize;
        if rel + 16 > have {
            let got = src.read_at(pos, &mut buf).ok()?;
            if got < 16 {
                break;
            }
            base = pos;
            have = got;
        }
        let rel = (pos - base) as usize;
        let incl = r32(&buf[rel..], 8);
        let orig = r32(&buf[rel..], 12);
        if incl > snaplen || incl > orig.max(incl) || incl > 262_144 {
            break;
        }
        pos += 16 + incl;
        n += 1;
        if pos - off > max {
            return None;
        }
    }
    (n > 0)
        .then_some(())
        .and_then(|_| hit(pos - off, "pcap", Confidence::Medium))
}

fn pcapng(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 12).ok()?;
    let le = match u32be(&h, 8) {
        0x1A2B3C4D => false,
        0x4D3C2B1A => true,
        _ => return None,
    };
    let r32 = |b: &[u8], o: usize| -> u64 {
        let s: [u8; 4] = b[o..o + 4].try_into().unwrap();
        if le {
            u32::from_le_bytes(s) as u64
        } else {
            u32::from_be_bytes(s) as u64
        }
    };
    let mut pos = off;
    let mut blocks = 0;
    loop {
        let b = read_vec(src, pos, 8).ok()?;
        if b.len() < 8 {
            break;
        }
        let len = r32(&b, 4);
        if len < 12 || len % 4 != 0 || len > 1 << 24 {
            break;
        }
        let tail = read_vec(src, pos + len - 4, 4).ok()?;
        if tail.len() < 4 || r32(&tail, 0) != len {
            break;
        }
        pos += len;
        blocks += 1;
        if pos - off > max {
            return None;
        }
    }
    (blocks >= 2)
        .then_some(())
        .and_then(|_| hit(pos - off, "pcapng", Confidence::Medium))
}
