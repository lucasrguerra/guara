//! Assinaturas de formatos e validação estrutural para o carving.
//!
//! Cada detector lê a estrutura real do arquivo (cabeçalhos, blocos, rodapés)
//! para determinar o tamanho exato e o nível de confiança, em vez de apenas
//! procurar bytes mágicos.

use crate::source::{read_vec, Source};
use crate::types::{Category, Confidence};

/// Resultado de um detector: arquivo reconhecido a partir de `offset`.
#[derive(Debug, Clone)]
pub struct Hit {
    pub len: u64,
    pub ext: &'static str,
    pub confidence: Confidence,
}

pub fn category_for_ext(ext: &str) -> Category {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "heic" | "avif" | "tif" | "tiff"
        | "psd" | "svg" | "ico" | "cr2" | "nef" | "dng" => Category::Image,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
        | "txt" | "rtf" | "epub" | "md" | "csv" | "html" | "xml" | "json" => Category::Document,
        "mp4" | "mov" | "m4v" | "avi" | "mkv" | "webm" | "wmv" | "flv" | "mpg" | "mpeg" | "3gp" => {
            Category::Video
        }
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "wma" | "opus" => Category::Audio,
        "zip" | "rar" | "7z" | "gz" | "tar" | "bz2" | "xz" | "jar" | "apk" => Category::Archive,
        "sqlite" | "db" | "sqlite3" | "mdb" | "accdb" => Category::Database,
        _ => Category::Other,
    }
}

/// Verifica se o início do conteúdo é coerente com a extensão.
/// `None` = extensão sem assinatura conhecida (não dá para julgar).
pub fn matches_ext(ext: &str, head: &[u8]) -> Option<bool> {
    let ext = ext.to_ascii_lowercase();
    let starts = |m: &[u8]| head.starts_with(m);
    let r = match ext.as_str() {
        "jpg" | "jpeg" => starts(&[0xFF, 0xD8, 0xFF]),
        "png" => starts(&[0x89, b'P', b'N', b'G']),
        "gif" => starts(b"GIF8"),
        "bmp" => starts(b"BM"),
        "pdf" => starts(b"%PDF"),
        "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "jar" | "epub" | "apk" => {
            starts(b"PK\x03\x04") || starts(b"PK\x05\x06")
        }
        "doc" | "xls" | "ppt" => starts(&[0xD0, 0xCF, 0x11, 0xE0]),
        "7z" => starts(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C]),
        "rar" => starts(b"Rar!"),
        "gz" => starts(&[0x1F, 0x8B]),
        "mp4" | "mov" | "m4v" | "m4a" | "3gp" | "heic" | "avif" => {
            head.len() >= 8 && &head[4..8] == b"ftyp"
                || (head.len() >= 8 && matches!(&head[4..8], b"moov" | b"mdat" | b"free" | b"wide"))
        }
        "avi" | "wav" | "webp" => starts(b"RIFF"),
        "mp3" => starts(b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0),
        "flac" => starts(b"fLaC"),
        "ogg" | "opus" => starts(b"OggS"),
        "mkv" | "webm" => starts(&[0x1A, 0x45, 0xDF, 0xA3]),
        "sqlite" | "sqlite3" | "db" => starts(b"SQLite format 3\0"),
        "exe" | "dll" => starts(b"MZ"),
        _ => return None,
    };
    Some(r)
}

/// Tabela de primeiros bytes -> detectores candidatos.
/// O carving consulta `candidates(first_byte)` e chama `detect`.
pub fn might_start(b0: u8) -> bool {
    matches!(
        b0,
        0xFF | 0x89
            | b'G'
            | b'B'
            | b'%'
            | b'P'
            | 0xD0
            | b'7'
            | b'S'
            | b'R'
            | b'I'
            | b'M'
            | 0x00
            | b'f'
            | 0x1F
    )
}

const MAX_HEADER: usize = 64 * 1024;

/// Tenta reconhecer um arquivo que começa em `off`.
/// `head` já contém os primeiros bytes lidos em `off` (pelo menos 16, se existirem).
pub fn detect(src: &dyn Source, off: u64, head: &[u8], max_size: u64) -> Option<Hit> {
    if head.len() < 12 {
        return None;
    }
    let h = head;
    if h.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return jpeg(src, off, max_size);
    }
    if h.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return png(src, off, max_size);
    }
    if h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a") {
        return gif(src, off, max_size);
    }
    if h.starts_with(b"%PDF-") {
        return pdf(src, off, max_size);
    }
    if h.starts_with(b"PK\x03\x04") {
        return zip(src, off, max_size);
    }
    if &h[4..8] == b"ftyp" {
        return isobmff(src, off, max_size);
    }
    if h.starts_with(b"RIFF") {
        return riff(h, max_size);
    }
    if h.starts_with(b"BM") {
        return bmp(h, max_size);
    }
    if h.starts_with(b"SQLite format 3\0") {
        return sqlite(h, max_size);
    }
    if h.starts_with(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C]) {
        return sevenz(h, max_size);
    }
    if h.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return ole(src, off, max_size);
    }
    if h.starts_with(b"ID3") {
        return mp3(src, off, max_size);
    }
    if h.starts_with(b"MZ") {
        return pe(src, off, max_size);
    }
    None
}

// ---------------------------------------------------------------- JPEG

fn jpeg(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let head = read_vec(src, off, MAX_HEADER).ok()?;
    let mut p = 2usize;
    let mut sos_end = None;
    let mut saw_sof = false;
    while p + 4 <= head.len() {
        if head[p] != 0xFF {
            return None;
        }
        let m = head[p + 1];
        if m == 0xFF {
            p += 1;
            continue;
        }
        if m == 0xD9 {
            return Some(Hit {
                len: p as u64 + 2,
                ext: "jpg",
                confidence: Confidence::Medium,
            });
        }
        if (0xD0..=0xD8).contains(&m) || m == 0x01 {
            p += 2;
            continue;
        }
        let seg = u16::from_be_bytes([head[p + 2], head[p + 3]]) as usize;
        if seg < 2 {
            return None;
        }
        if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            saw_sof = true;
        }
        p += 2 + seg;
        if m == 0xDA {
            sos_end = Some(p);
            break;
        }
    }
    let start = sos_end?;
    if !saw_sof {
        return None;
    }
    // Dados entropy-coded: procurar EOI (FF D9) ignorando FF00 e RSTn.
    let mut pos = off + start as u64;
    let limit = off + max.min(512 * 1024 * 1024);
    let mut buf = vec![0u8; 1 << 20];
    let mut prev_ff = false;
    while pos < limit {
        let n = src.read_at(pos, &mut buf).ok()?;
        if n == 0 {
            break;
        }
        for (i, &b) in buf[..n].iter().enumerate() {
            if prev_ff {
                if b == 0xD9 {
                    return Some(Hit {
                        len: pos + i as u64 + 1 - off,
                        ext: "jpg",
                        confidence: Confidence::High,
                    });
                }
                prev_ff = b == 0xFF;
            } else {
                prev_ff = b == 0xFF;
            }
        }
        pos += n as u64;
    }
    // Sem EOI: imagem truncada/sobrescrita; ainda útil, mas incerta.
    Some(Hit {
        len: (pos - off).min(4 * 1024 * 1024),
        ext: "jpg",
        confidence: Confidence::Low,
    })
}

// ---------------------------------------------------------------- PNG

fn png(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let mut p = 8u64;
    let mut first = true;
    for _ in 0..200_000 {
        let c = read_vec(src, off + p, 8).ok()?;
        if c.len() < 8 {
            break;
        }
        let len = u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as u64;
        let ty = &c[4..8];
        if !ty.iter().all(|b| b.is_ascii_alphabetic()) || len > 0x7FFF_FFFF {
            break;
        }
        if first && ty != b"IHDR" {
            return None;
        }
        first = false;
        p += 12 + len;
        if p > max {
            break;
        }
        if ty == b"IEND" {
            return Some(Hit {
                len: p,
                ext: "png",
                confidence: Confidence::High,
            });
        }
    }
    if first {
        return None;
    }
    Some(Hit {
        len: p.min(max).min(src.size().saturating_sub(off)),
        ext: "png",
        confidence: Confidence::Low,
    })
}

// ---------------------------------------------------------------- GIF

fn gif(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let d = read_vec(src, off, (16usize * 1024 * 1024).min(max as usize)).ok()?;
    if d.len() < 13 {
        return None;
    }
    let flags = d[10];
    let mut p = 13usize;
    if flags & 0x80 != 0 {
        p += 3 * (1usize << ((flags & 7) + 1));
    }
    while p < d.len() {
        match d[p] {
            0x3B => {
                return Some(Hit {
                    len: p as u64 + 1,
                    ext: "gif",
                    confidence: Confidence::High,
                })
            }
            0x21 => {
                p += 2;
                p = skip_subblocks(&d, p)?;
            }
            0x2C => {
                if p + 10 > d.len() {
                    break;
                }
                let f = d[p + 9];
                p += 10;
                if f & 0x80 != 0 {
                    p += 3 * (1usize << ((f & 7) + 1));
                }
                p += 1;
                p = skip_subblocks(&d, p)?;
            }
            _ => return None,
        }
    }
    Some(Hit {
        len: d.len() as u64,
        ext: "gif",
        confidence: Confidence::Low,
    })
}

fn skip_subblocks(d: &[u8], mut p: usize) -> Option<usize> {
    loop {
        let n = *d.get(p)? as usize;
        p += 1 + n;
        if n == 0 {
            return Some(p);
        }
    }
}

// ---------------------------------------------------------------- BMP

fn bmp(h: &[u8], max: u64) -> Option<Hit> {
    if h.len() < 18 {
        return None;
    }
    let size = u32::from_le_bytes([h[2], h[3], h[4], h[5]]) as u64;
    let reserved = u32::from_le_bytes([h[6], h[7], h[8], h[9]]);
    let dib = u32::from_le_bytes([h[14], h[15], h[16], h[17]]);
    if reserved != 0
        || !matches!(dib, 12 | 40 | 52 | 56 | 64 | 108 | 124)
        || size < 26
        || size > max
    {
        return None;
    }
    Some(Hit {
        len: size,
        ext: "bmp",
        confidence: Confidence::High,
    })
}

// ---------------------------------------------------------------- PDF

fn pdf(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let limit = max.min(1 << 30);
    let mut pos = off;
    let mut buf = vec![0u8; 1 << 20 | 16];
    let mut last_eof: Option<u64> = None;
    let mut carry: Vec<u8> = Vec::new();
    while pos - off < limit {
        let n = src.read_at(pos, &mut buf[..(1 << 20)]).ok()?;
        if n == 0 {
            break;
        }
        let mut hay = std::mem::take(&mut carry);
        let base = pos - hay.len() as u64;
        hay.extend_from_slice(&buf[..n]);
        let mut i = 0;
        while let Some(f) = find(&hay[i..], b"%%EOF") {
            let end = base + (i + f) as u64 + 5;
            // Continua se logo após o %%EOF vier uma atualização incremental.
            let tail = read_vec(src, end, 32).ok()?;
            let rest: Vec<u8> = tail
                .iter()
                .copied()
                .skip_while(|b| b.is_ascii_whitespace())
                .collect();
            let skipped = tail.len() - rest.len();
            last_eof = Some(end + skipped.min(2) as u64 - off);
            let more = rest.first().map(|b| b.is_ascii_digit()).unwrap_or(false)
                || rest.starts_with(b"%PDF");
            if !more {
                let conf = if last_eof.is_some() {
                    Confidence::High
                } else {
                    Confidence::Low
                };
                return Some(Hit {
                    len: last_eof.unwrap(),
                    ext: "pdf",
                    confidence: conf,
                });
            }
            i += f + 5;
        }
        let keep = hay.len().min(8);
        carry = hay[hay.len() - keep..].to_vec();
        pos += n as u64;
    }
    last_eof.map(|l| Hit {
        len: l,
        ext: "pdf",
        confidence: Confidence::Medium,
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------- ZIP & derivados

fn zip(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let limit = max.min(4 << 30);
    let mut pos = off;
    let mut buf = vec![0u8; 1 << 20];
    let mut carry: Vec<u8> = Vec::new();
    while pos - off < limit {
        let n = src.read_at(pos, &mut buf).ok()?;
        if n == 0 {
            break;
        }
        let mut hay = std::mem::take(&mut carry);
        let base = pos - hay.len() as u64;
        hay.extend_from_slice(&buf[..n]);
        let mut i = 0;
        while let Some(f) = find(&hay[i..], b"PK\x05\x06") {
            let at = base + (i + f) as u64;
            if let Some(hit) = check_eocd(src, off, at) {
                return Some(hit);
            }
            i += f + 4;
        }
        let keep = hay.len().min(3);
        carry = hay[hay.len() - keep..].to_vec();
        pos += n as u64;
    }
    None
}

fn check_eocd(src: &dyn Source, start: u64, at: u64) -> Option<Hit> {
    let e = read_vec(src, at, 22).ok()?;
    if e.len() < 22 {
        return None;
    }
    let entries = u16::from_le_bytes([e[10], e[11]]) as u64;
    let cd_size = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64;
    let cd_off = u32::from_le_bytes([e[16], e[17], e[18], e[19]]) as u64;
    let comment = u16::from_le_bytes([e[20], e[21]]) as u64;
    if entries == 0 || cd_off + cd_size != at - start {
        return None;
    }
    let cd = read_vec(src, start + cd_off, cd_size.min(4 << 20) as usize).ok()?;
    let names = zip_names(&cd);
    let ext = classify_zip(src, start, &names);
    Some(Hit {
        len: at - start + 22 + comment,
        ext,
        confidence: Confidence::High,
    })
}

fn zip_names(cd: &[u8]) -> Vec<String> {
    let mut out = vec![];
    let mut p = 0;
    while p + 46 <= cd.len() && &cd[p..p + 4] == b"PK\x01\x02" {
        let n = u16::from_le_bytes([cd[p + 28], cd[p + 29]]) as usize;
        let m = u16::from_le_bytes([cd[p + 30], cd[p + 31]]) as usize;
        let k = u16::from_le_bytes([cd[p + 32], cd[p + 33]]) as usize;
        if p + 46 + n > cd.len() {
            break;
        }
        out.push(String::from_utf8_lossy(&cd[p + 46..p + 46 + n]).into_owned());
        p += 46 + n + m + k;
        if out.len() > 20_000 {
            break;
        }
    }
    out
}

fn classify_zip(src: &dyn Source, start: u64, names: &[String]) -> &'static str {
    let has = |pre: &str| names.iter().any(|n| n.starts_with(pre));
    if has("word/") {
        "docx"
    } else if has("xl/") {
        "xlsx"
    } else if has("ppt/") {
        "pptx"
    } else if names.first().map(|n| n == "mimetype").unwrap_or(false) {
        // O primeiro item (sem compressão) traz o tipo MIME logo após o cabeçalho.
        let head = read_vec(src, start, 256).unwrap_or_default();
        let s = String::from_utf8_lossy(&head);
        if s.contains("opendocument.text") {
            "odt"
        } else if s.contains("opendocument.spreadsheet") {
            "ods"
        } else if s.contains("opendocument.presentation") {
            "odp"
        } else if s.contains("epub+zip") {
            "epub"
        } else {
            "zip"
        }
    } else if names.iter().any(|n| n == "AndroidManifest.xml") {
        "apk"
    } else if has("META-INF/") {
        "jar"
    } else {
        "zip"
    }
}

// ---------------------------------------------------------------- MP4 / MOV / HEIC (ISO BMFF)

fn isobmff(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let head = read_vec(src, off, 32).ok()?;
    let brand = head.get(8..12)?.to_vec();
    let mut p = 0u64;
    let mut saw_moov = false;
    let mut saw_mdat = false;
    let total = src.size().saturating_sub(off);
    for _ in 0..100_000 {
        let b = read_vec(src, off + p, 16).ok()?;
        if b.len() < 8 {
            break;
        }
        let mut size = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
        let ty = [b[4], b[5], b[6], b[7]];
        let known = matches!(
            &ty,
            b"ftyp"
                | b"moov"
                | b"mdat"
                | b"free"
                | b"skip"
                | b"wide"
                | b"uuid"
                | b"meta"
                | b"moof"
                | b"mfra"
                | b"sidx"
                | b"pnot"
                | b"styp"
                | b"pdin"
                | b"junk"
        );
        if !known {
            break;
        }
        if size == 1 {
            if b.len() < 16 {
                break;
            }
            size = u64::from_be_bytes(b[8..16].try_into().unwrap());
        } else if size == 0 {
            // Até o fim do arquivo: tamanho desconhecido.
            size = total - p;
        }
        if size < 8 || p + size > max {
            break;
        }
        match &ty {
            b"moov" | b"moof" => saw_moov = true,
            b"mdat" => saw_mdat = true,
            _ => {}
        }
        p += size;
        if p >= total {
            break;
        }
    }
    if p < 16 || !(saw_moov || saw_mdat) {
        return None;
    }
    let ext = match &brand[..] {
        b"qt  " => "mov",
        b"heic" | b"heix" | b"mif1" | b"msf1" | b"heim" | b"hevc" => "heic",
        b"avif" | b"avis" => "avif",
        b"M4A " | b"M4B " => "m4a",
        b"M4V " => "m4v",
        b"3gp4" | b"3gp5" | b"3gp6" | b"3g2a" => "3gp",
        _ => "mp4",
    };
    let conf = if saw_moov && p <= total {
        Confidence::High
    } else {
        Confidence::Low
    };
    Some(Hit {
        len: p.min(total),
        ext,
        confidence: conf,
    })
}

// ---------------------------------------------------------------- RIFF (AVI / WAV / WEBP)

fn riff(h: &[u8], max: u64) -> Option<Hit> {
    let size = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as u64 + 8;
    let ext = match &h[8..12] {
        b"AVI " => "avi",
        b"WAVE" => "wav",
        b"WEBP" => "webp",
        _ => return None,
    };
    if size < 16 || size > max {
        return None;
    }
    Some(Hit {
        len: size + (size & 1),
        ext,
        confidence: Confidence::High,
    })
}

// ---------------------------------------------------------------- SQLite

fn sqlite(h: &[u8], max: u64) -> Option<Hit> {
    if h.len() < 100 {
        return None;
    }
    let ps = u16::from_be_bytes([h[16], h[17]]) as u64;
    let ps = if ps == 1 { 65536 } else { ps };
    if !(512..=65536).contains(&ps) || !ps.is_power_of_two() {
        return None;
    }
    let pages = u32::from_be_bytes([h[28], h[29], h[30], h[31]]) as u64;
    let change = &h[24..28];
    let valid_for = &h[92..96];
    if pages == 0 {
        return None;
    }
    let len = pages * ps;
    if len > max {
        return None;
    }
    let conf = if change == valid_for {
        Confidence::High
    } else {
        Confidence::Medium
    };
    Some(Hit {
        len,
        ext: "sqlite",
        confidence: conf,
    })
}

// ---------------------------------------------------------------- 7z

fn sevenz(h: &[u8], max: u64) -> Option<Hit> {
    if h.len() < 32 {
        return None;
    }
    let crc = u32::from_le_bytes([h[8], h[9], h[10], h[11]]);
    if crc32(&h[12..32]) != crc {
        return None;
    }
    let next_off = u64::from_le_bytes(h[12..20].try_into().unwrap());
    let next_len = u64::from_le_bytes(h[20..28].try_into().unwrap());
    let len = 32u64.checked_add(next_off)?.checked_add(next_len)?;
    if len > max {
        return None;
    }
    Some(Hit {
        len,
        ext: "7z",
        confidence: Confidence::High,
    })
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

// ---------------------------------------------------------------- OLE2 (doc/xls/ppt legado)

fn ole(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let hdr = read_vec(src, off, 512).ok()?;
    if hdr.len() < 512 {
        return None;
    }
    let shift = u16::from_le_bytes([hdr[30], hdr[31]]) as u32;
    if !(shift == 9 || shift == 12) {
        return None;
    }
    let ssz = 1u64 << shift;
    let n_fat = u32::from_le_bytes(hdr[44..48].try_into().unwrap()) as usize;
    if n_fat == 0 || n_fat > 109 {
        return None;
    }
    let mut last_used = 0u64;
    for i in 0..n_fat {
        let sec = u32::from_le_bytes(hdr[76 + i * 4..80 + i * 4].try_into().unwrap()) as u64;
        let fat = read_vec(src, off + (sec + 1) * ssz, ssz as usize).ok()?;
        for (j, e) in fat.chunks_exact(4).enumerate() {
            let v = u32::from_le_bytes(e.try_into().unwrap());
            if v != 0xFFFF_FFFF {
                last_used = last_used.max(i as u64 * (ssz / 4) + j as u64);
            }
        }
    }
    let len = (last_used + 2) * ssz;
    if len > max {
        return None;
    }
    let data = read_vec(src, off, len.min(8 << 20) as usize).ok()?;
    let u16s = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect() };
    let ext = if find(&data, &u16s("WordDocument")).is_some() {
        "doc"
    } else if find(&data, &u16s("Workbook")).is_some() || find(&data, &u16s("Book")).is_some() {
        "xls"
    } else if find(&data, &u16s("PowerPoint Document")).is_some() {
        "ppt"
    } else {
        "ole"
    };
    Some(Hit {
        len,
        ext,
        confidence: Confidence::High,
    })
}

// ---------------------------------------------------------------- MP3

fn mp3(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 10).ok()?;
    if h.len() < 10 || h[3] == 0xFF || h[4] == 0xFF {
        return None;
    }
    let tag = ((h[6] as u64 & 0x7F) << 21)
        | ((h[7] as u64 & 0x7F) << 14)
        | ((h[8] as u64 & 0x7F) << 7)
        | (h[9] as u64 & 0x7F);
    let mut p = 10 + tag;
    let mut frames = 0u32;
    loop {
        let f = read_vec(src, off + p, 4).ok()?;
        if f.len() < 4 {
            break;
        }
        let Some(len) = mpeg_frame_len(&f) else { break };
        p += len;
        frames += 1;
        if p > max {
            break;
        }
    }
    if frames < 20 {
        return None;
    }
    // Possível tag ID3v1 de 128 bytes ao final.
    if let Ok(t) = read_vec(src, off + p, 3) {
        if t == b"TAG" {
            p += 128;
        }
    }
    Some(Hit {
        len: p,
        ext: "mp3",
        confidence: Confidence::Medium,
    })
}

fn mpeg_frame_len(h: &[u8]) -> Option<u64> {
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let ver = (h[1] >> 3) & 3; // 3 = MPEG1, 2 = MPEG2, 0 = 2.5
    let layer = (h[1] >> 1) & 3; // 1 = layer III
    if ver == 1 || layer != 1 {
        return None;
    }
    let br_idx = (h[2] >> 4) as usize;
    let sr_idx = ((h[2] >> 2) & 3) as usize;
    if br_idx == 0 || br_idx == 15 || sr_idx == 3 {
        return None;
    }
    const BR1: [u32; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const BR2: [u32; 16] = [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ];
    const SR1: [u32; 3] = [44100, 48000, 32000];
    let (br, sr, coef) = if ver == 3 {
        (BR1[br_idx], SR1[sr_idx], 144)
    } else {
        (BR2[br_idx], SR1[sr_idx] / if ver == 2 { 2 } else { 4 }, 72)
    };
    let pad = ((h[2] >> 1) & 1) as u32;
    Some((coef * br * 1000 / sr + pad) as u64)
}

// ---------------------------------------------------------------- PE (exe/dll)

fn pe(src: &dyn Source, off: u64, max: u64) -> Option<Hit> {
    let h = read_vec(src, off, 4096).ok()?;
    if h.len() < 0x40 {
        return None;
    }
    let lfanew = u32::from_le_bytes(h[0x3C..0x40].try_into().unwrap()) as usize;
    if lfanew + 24 > h.len() || &h[lfanew..lfanew + 4] != b"PE\0\0" {
        return None;
    }
    let nsec = u16::from_le_bytes([h[lfanew + 6], h[lfanew + 7]]) as usize;
    let opt = u16::from_le_bytes([h[lfanew + 20], h[lfanew + 21]]) as usize;
    let mut s = lfanew + 24 + opt;
    let mut end = 0u64;
    for _ in 0..nsec.min(96) {
        if s + 40 > h.len() {
            return None;
        }
        let raw_size = u32::from_le_bytes(h[s + 16..s + 20].try_into().unwrap()) as u64;
        let raw_ptr = u32::from_le_bytes(h[s + 20..s + 24].try_into().unwrap()) as u64;
        end = end.max(raw_ptr + raw_size);
        s += 40;
    }
    if end < 512 || end > max {
        return None;
    }
    let ext = if u16::from_le_bytes([h[lfanew + 22], h[lfanew + 23]]) & 0x2000 != 0 {
        "dll"
    } else {
        "exe"
    };
    Some(Hit {
        len: end,
        ext,
        confidence: Confidence::Medium,
    })
}
