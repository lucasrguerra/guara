//! Extração de metadados de arquivos recuperados: EXIF (câmera, datas, GPS),
//! informações de PDF, propriedades de documentos Office/OpenDocument e
//! dados de vídeo MP4/MOV. Tudo é leitura posicional, sem gravar em disco.

use crate::recover::read_found_at;
use crate::source::Source;
use crate::types::Found;
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Meta {
    pub kind: String,
    pub fields: Vec<(String, String)>,
    /// (latitude, longitude) em graus decimais.
    pub gps: Option<(f64, f64)>,
    /// Data em que a foto foi tirada (EXIF), s desde 1970 (hora da câmera lida como UTC).
    #[serde(default)]
    pub taken: Option<i64>,
    #[serde(default)]
    pub camera: Option<String>,
    #[serde(default)]
    pub px: Option<(u32, u32)>,
    /// Orientação EXIF (1..8).
    #[serde(default)]
    pub orientation: u16,
}

impl Meta {
    fn add(&mut self, k: &str, v: impl Into<String>) {
        let v = v.into();
        let v = v.trim().to_string();
        if !v.is_empty() {
            self.fields.push((k.to_string(), v));
        }
    }
}

struct Fr<'a> {
    src: &'a dyn Source,
    f: &'a Found,
}

impl Fr<'_> {
    fn read(&self, off: u64, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len.min(self.f.size.saturating_sub(off) as usize)];
        let mut got = 0;
        while got < out.len() {
            match read_found_at(self.src, self.f, off + got as u64, &mut out[got..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        out.truncate(got);
        out
    }
}

pub fn extract(src: &dyn Source, f: &Found) -> Meta {
    let fr = Fr { src, f };
    let mut m = Meta::default();
    match f.ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => {
            m.kind = "Imagem JPEG".into();
            jpeg(&fr, &mut m);
        }
        "png" => {
            m.kind = "Imagem PNG".into();
            let h = fr.read(0, 32);
            if h.len() >= 24 && &h[12..16] == b"IHDR" {
                m.px = Some((be32(&h[16..]), be32(&h[20..])));
                m.add(
                    "Dimensões",
                    format!("{} × {} px", be32(&h[16..]), be32(&h[20..])),
                );
            }
        }
        "gif" => {
            m.kind = "Imagem GIF".into();
            let h = fr.read(0, 16);
            if h.len() >= 10 {
                m.px = Some((le16(&h[6..]) as u32, le16(&h[8..]) as u32));
                m.add(
                    "Dimensões",
                    format!("{} × {} px", le16(&h[6..]), le16(&h[8..])),
                );
            }
        }
        "bmp" => {
            m.kind = "Imagem BMP".into();
            let h = fr.read(0, 32);
            if h.len() >= 26 {
                m.px = Some((le32(&h[18..]), (le32(&h[22..]) as i32).unsigned_abs()));
                m.add(
                    "Dimensões",
                    format!(
                        "{} × {} px",
                        le32(&h[18..]),
                        (le32(&h[22..]) as i32).unsigned_abs()
                    ),
                );
            }
        }
        "pdf" => {
            m.kind = "Documento PDF".into();
            pdf(&fr, &mut m);
        }
        "docx" | "xlsx" | "pptx" => {
            m.kind = "Documento Office".into();
            ooxml(&fr, &mut m);
        }
        "odt" | "ods" | "odp" => {
            m.kind = "Documento OpenDocument".into();
            odf(&fr, &mut m);
        }
        "mp4" | "mov" | "m4v" | "m4a" | "3gp" => {
            m.kind = "Mídia MP4/MOV".into();
            mp4(&fr, &mut m);
        }
        _ => {}
    }
    m
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

// ------------------------------------------------------------------ EXIF

fn jpeg(fr: &Fr, m: &mut Meta) {
    let d = fr.read(0, 256 * 1024);
    jpeg_bytes(&d, m);
}

/// Lê os segmentos de cabeçalho de um JPEG (já em memória): dimensões e EXIF.
pub fn jpeg_bytes(d: &[u8], m: &mut Meta) {
    let mut p = 2usize;
    while p + 4 <= d.len() && d[p] == 0xFF {
        let mk = d[p + 1];
        if mk == 0xDA || mk == 0xD9 {
            break;
        }
        let len = be16(&d[p + 2..]) as usize;
        if len < 2 || p + 2 + len > d.len() {
            break;
        }
        if mk == 0xE1 && d[p + 4..].starts_with(b"Exif\0\0") {
            exif(&d[p + 10..p + 2 + len], m);
        }
        if (mk == 0xC0 || mk == 0xC2) && p + 9 <= d.len() {
            m.px = Some((be16(&d[p + 7..]) as u32, be16(&d[p + 5..]) as u32));
            m.add(
                "Dimensões",
                format!("{} × {} px", be16(&d[p + 7..]), be16(&d[p + 5..])),
            );
        }
        p += 2 + len;
    }
}

/// Miniatura JPEG embutida no EXIF (IFD1), útil quando a imagem principal está danificada.
pub fn exif_thumbnail(d: &[u8]) -> Option<Vec<u8>> {
    let mut p = 2usize;
    while p + 4 <= d.len() && d[p] == 0xFF {
        let mk = d[p + 1];
        if mk == 0xDA || mk == 0xD9 {
            return None;
        }
        let len = be16(&d[p + 2..]) as usize;
        if len < 2 || p + 2 + len > d.len() {
            return None;
        }
        if mk == 0xE1 && d[p + 4..].starts_with(b"Exif\0\0") {
            let tiff = &d[p + 10..p + 2 + len];
            if tiff.len() < 8 {
                return None;
            }
            let le = match &tiff[0..2] {
                b"II" => true,
                b"MM" => false,
                _ => return None,
            };
            let t = Tiff { d: tiff, le };
            let ifd0 = t.u32(4)? as usize;
            let n = t.u16(ifd0)? as usize;
            let ifd1 = t.u32(ifd0 + 2 + n * 12)? as usize;
            if ifd1 == 0 {
                return None;
            }
            let (mut off, mut length) = (None, None);
            for e in t.ifd(ifd1) {
                match e.tag {
                    0x0201 => off = t.u32(e.pos),
                    0x0202 => length = t.u32(e.pos),
                    _ => {}
                }
            }
            let (o, l) = (off? as usize, length? as usize);
            return tiff.get(o..o + l).map(|b| b.to_vec());
        }
        p += 2 + len;
    }
    None
}

struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    pos: usize, // onde estão os dados do valor dentro do TIFF
}

impl Tiff<'_> {
    fn u16(&self, o: usize) -> Option<u16> {
        let b = self.d.get(o..o + 2)?;
        Some(if self.le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let b = self.d.get(o..o + 4)?;
        Some(if self.le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    }
    fn ifd(&self, off: usize) -> Vec<Entry> {
        let mut v = vec![];
        let Some(n) = self.u16(off) else { return v };
        for i in 0..(n as usize).min(200) {
            let e = off + 2 + i * 12;
            let (Some(tag), Some(typ), Some(count)) =
                (self.u16(e), self.u16(e + 2), self.u32(e + 4))
            else {
                break;
            };
            let size = match typ {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => 1,
            } * count as usize;
            let pos = if size <= 4 {
                e + 8
            } else {
                self.u32(e + 8).unwrap_or(0) as usize
            };
            v.push(Entry {
                tag,
                typ,
                count,
                pos,
            });
        }
        v
    }
    fn ascii(&self, e: &Entry) -> String {
        let end = (e.pos + e.count as usize).min(self.d.len());
        let b = self.d.get(e.pos..end).unwrap_or(&[]);
        String::from_utf8_lossy(b)
            .trim_end_matches('\0')
            .trim()
            .to_string()
    }
    fn rational(&self, e: &Entry, i: usize) -> Option<f64> {
        let o = e.pos + i * 8;
        let (n, d) = (self.u32(o)?, self.u32(o + 4)?);
        (d != 0).then(|| n as f64 / d as f64)
    }
    fn num(&self, e: &Entry) -> Option<u32> {
        match e.typ {
            3 => self.u16(e.pos).map(u32::from),
            4 => self.u32(e.pos),
            _ => None,
        }
    }
}

fn exif(d: &[u8], m: &mut Meta) {
    if d.len() < 8 {
        return;
    }
    let le = match &d[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return,
    };
    let t = Tiff { d, le };
    let Some(ifd0) = t.u32(4) else { return };
    let mut exif_ptr = None;
    let mut gps_ptr = None;
    let (mut make, mut model) = (String::new(), String::new());
    for e in t.ifd(ifd0 as usize) {
        match e.tag {
            0x010F => {
                make = t.ascii(&e);
                m.add("Fabricante da câmera", make.clone());
            }
            0x0110 => {
                model = t.ascii(&e);
                m.add("Modelo da câmera", model.clone());
            }
            0x0112 => m.orientation = t.num(&e).unwrap_or(1) as u16,
            0x0131 => m.add("Software", t.ascii(&e)),
            0x0132 => m.add("Data (EXIF)", exif_date(&t.ascii(&e))),
            0x013B => m.add("Autor", t.ascii(&e)),
            0x8298 => m.add("Copyright", t.ascii(&e)),
            0x8769 => exif_ptr = t.u32(e.pos),
            0x8825 => gps_ptr = t.u32(e.pos),
            _ => {}
        }
    }
    let first = make.split_whitespace().next().unwrap_or("").to_lowercase();
    let cam = if model.is_empty() {
        make.clone()
    } else if make.is_empty() || (!first.is_empty() && model.to_lowercase().starts_with(&first)) {
        model.clone()
    } else {
        format!("{make} {model}")
    };
    if !cam.trim().is_empty() {
        m.camera = Some(cam.trim().to_string());
    }
    if let Some(o) = exif_ptr {
        let (mut w, mut h) = (None, None);
        for e in t.ifd(o as usize) {
            match e.tag {
                0x9003 => {
                    m.taken = exif_epoch(&t.ascii(&e));
                    m.add("Foto tirada em", exif_date(&t.ascii(&e)));
                }
                0x9004 => m.add("Digitalizada em", exif_date(&t.ascii(&e))),
                0xA434 => m.add("Lente", t.ascii(&e)),
                0xA431 => m.add("Nº de série do corpo", t.ascii(&e)),
                0xA430 => m.add("Proprietário da câmera", t.ascii(&e)),
                0xA002 => w = t.num(&e),
                0xA003 => h = t.num(&e),
                0x829A => {
                    if let Some(v) = t.rational(&e, 0) {
                        m.add(
                            "Exposição",
                            if v > 0.0 && v < 1.0 {
                                format!("1/{:.0} s", 1.0 / v)
                            } else {
                                format!("{v:.1} s")
                            },
                        );
                    }
                }
                0x829D => {
                    if let Some(v) = t.rational(&e, 0) {
                        m.add("Abertura", format!("f/{v:.1}"));
                    }
                }
                0x8827 => {
                    if let Some(v) = t.num(&e) {
                        m.add("ISO", v.to_string());
                    }
                }
                0x920A => {
                    if let Some(v) = t.rational(&e, 0) {
                        m.add("Distância focal", format!("{v:.0} mm"));
                    }
                }
                _ => {}
            }
        }
        if let (Some(w), Some(h)) = (w, h) {
            if m.px.is_none() {
                m.px = Some((w, h));
            }
            if !m.fields.iter().any(|(k, _)| k == "Dimensões") {
                m.add("Dimensões", format!("{w} × {h} px"));
            }
        }
    }
    if let Some(o) = gps_ptr {
        let (mut lat, mut lon, mut lat_ref, mut lon_ref, mut alt) = (None, None, 'N', 'E', None);
        for e in t.ifd(o as usize) {
            let dms = |e: &Entry| -> Option<f64> {
                Some(t.rational(e, 0)? + t.rational(e, 1)? / 60.0 + t.rational(e, 2)? / 3600.0)
            };
            match e.tag {
                0x0001 => lat_ref = t.ascii(&e).chars().next().unwrap_or('N'),
                0x0002 => lat = dms(&e),
                0x0003 => lon_ref = t.ascii(&e).chars().next().unwrap_or('E'),
                0x0004 => lon = dms(&e),
                0x0006 => alt = t.rational(&e, 0),
                0x001D => m.add("Data do GPS", t.ascii(&e).replace(':', "-")),
                _ => {}
            }
        }
        if let (Some(a), Some(b)) = (lat, lon) {
            let a = if lat_ref == 'S' { -a } else { a };
            let b = if lon_ref == 'W' { -b } else { b };
            if a.abs() <= 90.0 && b.abs() <= 180.0 && !(a == 0.0 && b == 0.0) {
                m.gps = Some((a, b));
                m.add("Localização (GPS)", format!("{a:.6}, {b:.6}"));
                if let Some(h) = alt {
                    m.add("Altitude", format!("{h:.0} m"));
                }
            }
        }
    }
}

/// "2023:07:14 18:22:05" -> segundos desde 1970 (hora da câmera lida como UTC)
fn exif_epoch(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b':' || b[7] != b':' {
        return None;
    }
    let n = |a: usize, c: usize| s.get(a..c)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (
        n(0, 4)?,
        n(5, 7)?,
        n(8, 10)?,
        n(11, 13)?,
        n(14, 16)?,
        n(17, 19)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || y < 1970 {
        return None;
    }
    Some(crate::fs::days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se)
}

/// "2023:07:14 18:22:05" -> "14/07/2023 18:22:05"
fn exif_date(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 19 && b[4] == b':' && b[7] == b':' {
        format!("{}/{}/{} {}", &s[8..10], &s[5..7], &s[0..4], &s[11..19])
    } else {
        s.to_string()
    }
}

// ------------------------------------------------------------------ PDF

fn pdf(fr: &Fr, m: &mut Meta) {
    let size = fr.f.size;
    let head = fr.read(0, 96 * 1024);
    let tail = if size > 192 * 1024 {
        fr.read(size - 96 * 1024, 96 * 1024)
    } else {
        Vec::new()
    };
    let mut all = head.clone();
    all.extend_from_slice(&tail);
    if head.starts_with(b"%PDF-") {
        m.add(
            "Versão do PDF",
            String::from_utf8_lossy(&head[5..8.min(head.len())]).to_string(),
        );
    }
    for (key, label) in [
        ("/Title", "Título"),
        ("/Author", "Autor"),
        ("/Subject", "Assunto"),
        ("/Creator", "Criado com"),
        ("/Producer", "Gerado por"),
    ] {
        if let Some(v) = pdf_string(&all, key) {
            m.add(label, v);
        }
    }
    for (key, label) in [
        ("/CreationDate", "Criado em"),
        ("/ModDate", "Modificado em"),
    ] {
        if let Some(v) = pdf_string(&all, key) {
            m.add(label, pdf_date(&v));
        }
    }
    let pages = find_all(&all, b"/Count ").into_iter().filter_map(|i| {
        let s: String = all[i + 7..]
            .iter()
            .take(8)
            .map(|&c| c as char)
            .take_while(|c| c.is_ascii_digit())
            .collect();
        s.parse::<u32>().ok()
    });
    if let Some(n) = pages.max() {
        m.add("Páginas (aprox.)", n.to_string());
    }
    if !find_all(&all, b"/Encrypt").is_empty() {
        m.add("Criptografado", "sim");
    }
}

fn find_all(h: &[u8], n: &[u8]) -> Vec<usize> {
    let mut v = vec![];
    let mut i = 0;
    while i + n.len() <= h.len() {
        if &h[i..i + n.len()] == n {
            v.push(i);
            i += n.len();
        } else {
            i += 1;
        }
    }
    v
}

fn pdf_string(d: &[u8], key: &str) -> Option<String> {
    let k = key.as_bytes();
    for at in find_all(d, k) {
        let mut p = at + k.len();
        // evita /Creator casar com /CreatorXyz
        if d.get(p).map(|c| c.is_ascii_alphanumeric()).unwrap_or(false) {
            continue;
        }
        while d.get(p).map(|c| c.is_ascii_whitespace()).unwrap_or(false) {
            p += 1;
        }
        match d.get(p)? {
            b'(' => {
                let mut depth = 0;
                let mut out = vec![];
                let mut i = p;
                while i < d.len() && out.len() < 400 {
                    let c = d[i];
                    match c {
                        b'\\' if i + 1 < d.len() => {
                            i += 1;
                            if d[i].is_ascii_digit() && d[i] < b'8' {
                                // escape octal \ddd (1 a 3 dígitos)
                                let mut v = 0u32;
                                let mut n = 0;
                                while n < 3 && i < d.len() && d[i] >= b'0' && d[i] < b'8' {
                                    v = v * 8 + (d[i] - b'0') as u32;
                                    i += 1;
                                    n += 1;
                                }
                                out.push((v & 0xFF) as u8);
                                continue;
                            }
                            out.push(match d[i] {
                                b'n' => b'\n',
                                b'r' => b'\r',
                                b't' => b'\t',
                                o => o,
                            });
                        }
                        b'(' => {
                            depth += 1;
                            if depth > 1 {
                                out.push(c);
                            }
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            out.push(c);
                        }
                        _ => out.push(c),
                    }
                    i += 1;
                }
                return Some(pdf_text(&out));
            }
            b'<' if d.get(p + 1) != Some(&b'<') => {
                let hex: Vec<u8> = d[p + 1..]
                    .iter()
                    .take_while(|&&c| c != b'>')
                    .copied()
                    .filter(|c| c.is_ascii_hexdigit())
                    .collect();
                let bytes: Vec<u8> = hex
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap_or(0))
                    .collect();
                return Some(pdf_text(&bytes));
            }
            _ => {}
        }
    }
    None
}

fn pdf_text(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let u: Vec<u16> = b[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&u);
    }
    // PDFDocEncoding ~ Latin-1 na prática
    b.iter().map(|&c| c as char).collect()
}

/// "D:20230714182205-03'00'" -> "14/07/2023 18:22:05"
fn pdf_date(s: &str) -> String {
    let t = s.trim_start_matches("D:");
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 8 {
        let g = |a: usize, b: usize| digits.get(a..b).unwrap_or("00");
        return format!(
            "{}/{}/{} {}:{}:{}",
            g(6, 8),
            g(4, 6),
            g(0, 4),
            g(8, 10),
            g(10, 12),
            g(12, 14)
        );
    }
    s.to_string()
}

// ------------------------------------------------------------------ ZIP (Office/ODF)

struct ZipEnt {
    name: String,
    method: u16,
    csize: u64,
    usize: u64,
    off: u64,
}

fn zip_dir(fr: &Fr) -> Vec<ZipEnt> {
    let size = fr.f.size;
    let tail_len = size.min(66_000);
    let tail = fr.read(size - tail_len, tail_len as usize);
    let Some(e) = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..].starts_with(b"PK\x05\x06"))
    else {
        return vec![];
    };
    let n = le16(&tail[e + 10..]) as usize;
    let cd_size = le32(&tail[e + 12..]) as usize;
    let cd_off = le32(&tail[e + 16..]) as u64;
    if cd_size > 8 << 20 || cd_off + cd_size as u64 > size {
        return vec![];
    }
    let cd = fr.read(cd_off, cd_size);
    let mut out = vec![];
    let mut p = 0;
    while out.len() < n.min(5000) && p + 46 <= cd.len() && &cd[p..p + 4] == b"PK\x01\x02" {
        let nl = le16(&cd[p + 28..]) as usize;
        let xl = le16(&cd[p + 30..]) as usize;
        let cl = le16(&cd[p + 32..]) as usize;
        if p + 46 + nl > cd.len() {
            break;
        }
        out.push(ZipEnt {
            name: String::from_utf8_lossy(&cd[p + 46..p + 46 + nl]).to_string(),
            method: le16(&cd[p + 10..]),
            csize: le32(&cd[p + 20..]) as u64,
            usize: le32(&cd[p + 24..]) as u64,
            off: le32(&cd[p + 42..]) as u64,
        });
        p += 46 + nl + xl + cl;
    }
    out
}

fn zip_read(fr: &Fr, e: &ZipEnt) -> Option<Vec<u8>> {
    if e.usize > 4 << 20 {
        return None;
    }
    let lh = fr.read(e.off, 30);
    if lh.len() < 30 || &lh[0..4] != b"PK\x03\x04" {
        return None;
    }
    let start = e.off + 30 + le16(&lh[26..]) as u64 + le16(&lh[28..]) as u64;
    let data = fr.read(start, e.csize as usize);
    match e.method {
        0 => Some(data),
        8 => {
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(&data[..])
                .take(4 << 20)
                .read_to_end(&mut out)
                .ok()?;
            Some(out)
        }
        _ => None,
    }
}

/// Texto do primeiro elemento `<tag ...>texto</tag>`.
fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(i) = xml[from..].find(&open) {
        let s = from + i + open.len();
        match xml[s..].chars().next() {
            Some('>') | Some(' ') | Some('/') => {
                let gt = xml[s..].find('>')? + s;
                if xml[..gt].ends_with('/') {
                    return None;
                }
                let end = xml[gt + 1..].find(&format!("</{tag}>"))? + gt + 1;
                return Some(unescape(&xml[gt + 1..end]));
            }
            _ => from = s,
        }
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// "2023-07-14T18:22:05Z" -> "14/07/2023 18:22:05 (UTC)"
fn iso_date(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 19 && b[4] == b'-' && b[10] == b'T' {
        let utc = if s.ends_with('Z') { " (UTC)" } else { "" };
        return format!(
            "{}/{}/{} {}{}",
            &s[8..10],
            &s[5..7],
            &s[0..4],
            &s[11..19],
            utc
        );
    }
    s.to_string()
}

fn ooxml(fr: &Fr, m: &mut Meta) {
    let ents = zip_dir(fr);
    let get = |n: &str| {
        ents.iter()
            .find(|e| e.name == n)
            .and_then(|e| zip_read(fr, e))
            .map(|b| String::from_utf8_lossy(&b).to_string())
    };
    if let Some(x) = get("docProps/core.xml") {
        for (tag, label) in [
            ("dc:title", "Título"),
            ("dc:subject", "Assunto"),
            ("dc:creator", "Autor"),
            ("cp:lastModifiedBy", "Última modificação por"),
            ("cp:keywords", "Palavras-chave"),
        ] {
            if let Some(v) = xml_tag(&x, tag) {
                m.add(label, v);
            }
        }
        for (tag, label) in [
            ("dcterms:created", "Criado em"),
            ("dcterms:modified", "Modificado em"),
        ] {
            if let Some(v) = xml_tag(&x, tag) {
                m.add(label, iso_date(&v));
            }
        }
    }
    if let Some(x) = get("docProps/app.xml") {
        for (tag, label) in [
            ("Application", "Aplicativo"),
            ("Company", "Empresa"),
            ("Pages", "Páginas"),
            ("Words", "Palavras"),
        ] {
            if let Some(v) = xml_tag(&x, tag) {
                m.add(label, v);
            }
        }
    }
}

fn odf(fr: &Fr, m: &mut Meta) {
    let ents = zip_dir(fr);
    let Some(x) = ents
        .iter()
        .find(|e| e.name == "meta.xml")
        .and_then(|e| zip_read(fr, e))
        .map(|b| String::from_utf8_lossy(&b).to_string())
    else {
        return;
    };
    for (tag, label) in [
        ("dc:title", "Título"),
        ("meta:initial-creator", "Criado por"),
        ("dc:creator", "Última modificação por"),
        ("meta:generator", "Aplicativo"),
    ] {
        if let Some(v) = xml_tag(&x, tag) {
            m.add(label, v);
        }
    }
    for (tag, label) in [
        ("meta:creation-date", "Criado em"),
        ("dc:date", "Modificado em"),
    ] {
        if let Some(v) = xml_tag(&x, tag) {
            m.add(label, iso_date(&v));
        }
    }
}

// ------------------------------------------------------------------ MP4 / MOV

fn mp4(fr: &Fr, m: &mut Meta) {
    let size = fr.f.size;
    let mut p = 0u64;
    for _ in 0..64 {
        let h = fr.read(p, 16);
        if h.len() < 8 {
            return;
        }
        let mut sz = be32(&h) as u64;
        if sz == 1 && h.len() >= 16 {
            sz = u64::from_be_bytes(h[8..16].try_into().unwrap());
        } else if sz == 0 {
            sz = size - p;
        }
        if sz < 8 {
            return;
        }
        if &h[4..8] == b"moov" {
            let body = fr.read(p + 8, (sz - 8).min(4 << 20) as usize);
            moov(&body, m);
            return;
        }
        p += sz;
        if p >= size {
            return;
        }
    }
}

fn boxes(d: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut v = vec![];
    let mut p = 0;
    while p + 8 <= d.len() {
        let sz = be32(&d[p..]) as usize;
        if sz < 8 || p + sz > d.len() {
            break;
        }
        v.push((&d[p + 4..p + 8], &d[p + 8..p + sz]));
        p += sz;
    }
    v
}

fn moov(d: &[u8], m: &mut Meta) {
    const EPOCH_1904: u64 = 2_082_844_800;
    for (ty, body) in boxes(d) {
        match ty {
            b"mvhd" if body.len() >= 24 => {
                let v = body[0];
                let (created, ts, dur) = if v == 1 && body.len() >= 32 {
                    (
                        u64::from_be_bytes(body[4..12].try_into().unwrap()),
                        be32(&body[20..]) as u64,
                        u64::from_be_bytes(body[24..32].try_into().unwrap()),
                    )
                } else {
                    (
                        be32(&body[4..]) as u64,
                        be32(&body[12..]) as u64,
                        be32(&body[16..]) as u64,
                    )
                };
                if created > EPOCH_1904 {
                    m.add(
                        "Gravado em (metadado)",
                        fmt_unix((created - EPOCH_1904) as i64),
                    );
                }
                if ts > 0 && dur > 0 {
                    let s = dur / ts;
                    m.add(
                        "Duração",
                        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60),
                    );
                }
            }
            b"trak" => {
                for (t2, b2) in boxes(body) {
                    if t2 == b"tkhd" && b2.len() >= 84 {
                        let off = if b2[0] == 1 { 88 } else { 76 };
                        if b2.len() >= off + 8 {
                            let (w, h) = (be32(&b2[off..]) >> 16, be32(&b2[off + 4..]) >> 16);
                            if w > 0 && h > 0 && !m.fields.iter().any(|(k, _)| k == "Dimensões") {
                                m.add("Dimensões", format!("{w} × {h} px"));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn fmt_unix(t: i64) -> String {
    let days = t.div_euclid(86400);
    let rem = t.rem_euclid(86400);
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{d:02}/{mo:02}/{y} {:02}:{:02}:{:02} (UTC)",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// (ano, mês, dia) de um instante em segundos desde 1970.
pub fn ymd(t: i64) -> (i64, u32, u32) {
    let (y, m, d) = civil_from_days(t.div_euclid(86400));
    (y, m as u32, d as u32)
}

/// Inverso de `days_from_civil` (Howard Hinnant).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
