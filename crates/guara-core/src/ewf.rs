//! Imagens forenses Expert Witness (EWF): E01 (EnCase 1 a 6, SMART) e Ex01 (EnCase 7, v2).
//!
//! Leitura de conjuntos com vários segmentos (`.E01`, `.E02`, ...), com chunks
//! deflate, bzip2, sem compressão ou com preenchimento por padrão. Escrita de
//! E01 no formato EnCase 6. A origem nunca é alterada.

use crate::error::{Error, Result};
use crate::hashing::MultiHasher;
use crate::image::ImageReport;
use crate::source::Source;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SIG_V1: [u8; 8] = [0x45, 0x56, 0x46, 0x09, 0x0d, 0x0a, 0xff, 0x00];
pub const SIG_V2: [u8; 8] = [0x45, 0x56, 0x46, 0x32, 0x0d, 0x0a, 0x81, 0x00];

/// Reconhece o cabeçalho de um segmento EWF.
pub fn is_ewf_header(head: &[u8]) -> bool {
    head.len() >= 8 && (head[..8] == SIG_V1 || head[..8] == SIG_V2)
}

pub fn adler32(data: &[u8]) -> u32 {
    adler32_update(1, data)
}

pub fn adler32_update(adler: u32, data: &[u8]) -> u32 {
    let (mut a, mut b) = (adler & 0xFFFF, adler >> 16);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn pread(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(f, buf, off)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(f, buf, off)
    }
}

fn pread_exact(f: &File, buf: &mut [u8], mut off: u64) -> io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        let n = pread(f, &mut buf[done..], off)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "segmento EWF truncado",
            ));
        }
        done += n;
        off += n as u64;
    }
    Ok(())
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    Raw,
    Deflate,
    Bzip2,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// Dados guardados como estão; `checksum` indica 4 bytes de Adler-32 no fim.
    Stored {
        checksum: bool,
    },
    Packed(Codec),
    /// Chunk inteiro repetindo estes 8 bytes (EnCase 7 "empty block").
    Pattern([u8; 8]),
}

#[derive(Clone, Copy, Debug)]
struct Chunk {
    seg: u32,
    off: u64,
    len: u32,
    kind: Kind,
}

/// Metadados lidos do conjunto de segmentos.
#[derive(Debug, Clone, Default)]
pub struct EwfInfo {
    /// 1 para E01, 2 para Ex01.
    pub version: u8,
    pub segments: usize,
    pub chunk_size: u64,
    pub bytes_per_sector: u32,
    pub sectors: u64,
    /// Caso, examinador, notas etc. (chaves legíveis).
    pub fields: BTreeMap<String, String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
}

pub struct EwfSource {
    files: Vec<File>,
    chunks: Vec<Chunk>,
    chunk_size: u64,
    size: u64,
    pub info: EwfInfo,
    cache: Mutex<Option<(usize, Vec<u8>)>>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Próximo nome de segmento: E01..E99, EAA..EZZ, FAA... (a mesma letra inicial vira Ex).
pub fn next_segment_ext(ext: &str) -> Option<String> {
    let b = ext.as_bytes();
    if b.len() != 3 {
        return None;
    }
    let upper = b[0].is_ascii_uppercase();
    let first = b[0].to_ascii_uppercase();
    let rest = &ext[1..];
    let out = if let Ok(n) = rest.parse::<u32>() {
        if n < 99 {
            format!("{}{:02}", first as char, n + 1)
        } else {
            format!("{}AA", first as char)
        }
    } else {
        let mut c = [
            first,
            rest.as_bytes()[0].to_ascii_uppercase(),
            rest.as_bytes()[1].to_ascii_uppercase(),
        ];
        let mut i = 2;
        loop {
            if c[i] < b'Z' {
                c[i] += 1;
                break;
            }
            c[i] = b'A';
            if i == 0 {
                return None;
            }
            i -= 1;
            if i == 0 {
                if c[0] >= b'Z' {
                    return None;
                }
                c[0] += 1;
                break;
            }
        }
        String::from_utf8(c.to_vec()).ok()?
    };
    Some(if upper { out } else { out.to_ascii_lowercase() })
}

/// Nome do segmento seguinte para E01 (`.E01` -> `.E02`) e Ex01 (`.Ex01` -> `.Ex02`).
fn next_segment_path(p: &Path) -> Option<PathBuf> {
    let ext = p.extension()?.to_str()?;
    let new = if ext.len() == 4 && ext[..2].eq_ignore_ascii_case("ex") {
        let n: u32 = ext[2..].parse().ok()?;
        let tag = &ext[..2];
        if n >= 99 {
            return None;
        }
        format!("{tag}{:02}", n + 1)
    } else {
        next_segment_ext(ext)?
    };
    Some(p.with_extension(new))
}

impl EwfSource {
    pub fn open(first: &Path) -> Result<Self> {
        let mut files = vec![];
        let mut path = first.to_path_buf();
        loop {
            match File::open(&path) {
                Ok(f) => files.push(f),
                Err(_) if !files.is_empty() => break,
                Err(e) => return Err(e.into()),
            }
            match next_segment_path(&path) {
                Some(n) if n.exists() => path = n,
                _ => break,
            }
        }
        let mut head = [0u8; 8];
        pread_exact(&files[0], &mut head, 0)?;
        match head {
            SIG_V1 => Self::open_v1(files),
            SIG_V2 => Self::open_v2(files),
            _ => Err(Error::msg("não é uma imagem EWF (E01/Ex01)")),
        }
    }

    fn finish(
        files: Vec<File>,
        chunks: Vec<Chunk>,
        mut info: EwfInfo,
        size_hint: u64,
    ) -> Result<Self> {
        if chunks.is_empty() {
            return Err(Error::msg("imagem EWF sem tabela de chunks"));
        }
        let chunk_size = info.chunk_size;
        let max = chunks.len() as u64 * chunk_size;
        let size = if size_hint > 0 {
            size_hint.min(max)
        } else {
            max
        };
        info.segments = files.len();
        Ok(Self {
            files,
            chunks,
            chunk_size,
            size,
            info,
            cache: Mutex::new(None),
        })
    }

    // ------------------------------------------------------------ E01 (v1)

    fn open_v1(files: Vec<File>) -> Result<Self> {
        let mut info = EwfInfo {
            version: 1,
            ..Default::default()
        };
        let mut chunks: Vec<Chunk> = vec![];
        for (si, f) in files.iter().enumerate() {
            let flen = f.metadata()?.len();
            let mut pos = 13u64;
            let mut sectors_end = 0u64;
            let mut guard = 0;
            while pos + 76 <= flen && guard < 1_000_000 {
                guard += 1;
                let mut d = [0u8; 76];
                pread_exact(f, &mut d, pos)?;
                let ty_end = d[..16].iter().position(|&c| c == 0).unwrap_or(16);
                let ty = String::from_utf8_lossy(&d[..ty_end]).to_string();
                let next = le64(&d, 16);
                let size = le64(&d, 24);
                let data_len = size.saturating_sub(76).min(flen);
                let read_data = |max: u64| -> io::Result<Vec<u8>> {
                    let n = data_len.min(max) as usize;
                    let mut v = vec![0u8; n];
                    pread_exact(f, &mut v, pos + 76)?;
                    Ok(v)
                };
                match ty.as_str() {
                    "header" | "header2" if si == 0 && info.fields.is_empty() => {
                        if let Ok(raw) = read_data(1 << 20) {
                            parse_header_text(&inflate_all(&raw), &mut info.fields);
                        }
                    }
                    "volume" | "disk" if si == 0 => {
                        let v = read_data(2048)?;
                        if v.len() >= 24 {
                            let spc = le32(&v, 8) as u64;
                            let bps = le32(&v, 12);
                            info.bytes_per_sector = bps;
                            info.chunk_size = spc * bps as u64;
                            info.sectors = le64(&v, 16);
                        }
                    }
                    "sectors" => sectors_end = pos + size,
                    "table" => {
                        let hdr = read_data(24)?;
                        if hdr.len() == 24 {
                            let n = le32(&hdr, 0) as usize;
                            let base = le64(&hdr, 8);
                            let mut raw = vec![0u8; n * 4];
                            pread_exact(f, &mut raw, pos + 76 + 24)?;
                            let offs: Vec<(u64, bool)> = (0..n)
                                .map(|i| {
                                    let e = le32(&raw, i * 4);
                                    (base + (e & 0x7FFF_FFFF) as u64, e & 0x8000_0000 != 0)
                                })
                                .collect();
                            for i in 0..n {
                                let (off, comp) = offs[i];
                                let end = if i + 1 < n {
                                    offs[i + 1].0
                                } else {
                                    sectors_end
                                };
                                if end <= off {
                                    return Err(Error::msg("tabela EWF inconsistente"));
                                }
                                let len = (end - off) as u32;
                                chunks.push(Chunk {
                                    seg: si as u32,
                                    off,
                                    len,
                                    kind: if comp {
                                        Kind::Packed(Codec::Deflate)
                                    } else {
                                        Kind::Stored { checksum: true }
                                    },
                                });
                            }
                        }
                    }
                    "hash" if si == files.len() - 1 || info.md5.is_none() => {
                        if let Ok(v) = read_data(16) {
                            if v.len() == 16 {
                                info.md5 = Some(hex(&v));
                            }
                        }
                    }
                    "digest" => {
                        if let Ok(v) = read_data(36) {
                            if v.len() >= 36 {
                                info.md5 = Some(hex(&v[..16]));
                                info.sha1 = Some(hex(&v[16..36]));
                            }
                        }
                    }
                    _ => {}
                }
                if ty == "done" || next == 0 || next <= pos {
                    break;
                }
                pos = next;
            }
        }
        if info.chunk_size == 0 {
            return Err(Error::msg("cabeçalho de volume EWF ausente"));
        }
        let hint = info.sectors * info.bytes_per_sector as u64;
        Self::finish(files, chunks, info, hint)
    }

    // ----------------------------------------------------------- Ex01 (v2)

    fn open_v2(files: Vec<File>) -> Result<Self> {
        let mut info = EwfInfo {
            version: 2,
            ..Default::default()
        };
        let mut hdr = [0u8; 32];
        pread_exact(&files[0], &mut hdr, 0)?;
        let method = match le16(&hdr, 10) {
            2 => Codec::Bzip2,
            0 => Codec::Raw,
            _ => Codec::Deflate,
        };
        let mut slots: BTreeMap<u64, Chunk> = BTreeMap::new();
        let mut total_bytes = 0u64;
        for (si, f) in files.iter().enumerate() {
            let flen = f.metadata()?.len();
            if flen < 96 {
                continue;
            }
            // Os descritores formam uma lista encadeada para trás, a partir do fim do arquivo.
            let mut pos = flen - 64;
            let mut guard = 0;
            loop {
                guard += 1;
                let mut d = [0u8; 64];
                pread_exact(f, &mut d, pos)?;
                let ty = le32(&d, 0);
                let prev = le64(&d, 8);
                let dsize = le64(&d, 16);
                let pad = le32(&d, 28) as u64;
                let dstart = pos.saturating_sub(dsize);
                let data_len = dsize; // inclui o preenchimento; os decodificadores param no fim do fluxo
                let _ = pad;
                let read_data = |max: u64| -> io::Result<Vec<u8>> {
                    let n = data_len.min(max) as usize;
                    let mut v = vec![0u8; n];
                    pread_exact(f, &mut v, dstart)?;
                    Ok(v)
                };
                match ty {
                    1 | 2 if si == 0 => {
                        if let Ok(raw) = read_data(1 << 20) {
                            let text = utf16_text(&inflate_all(&raw));
                            parse_v2_text(&text, &mut info, &mut total_bytes, ty);
                        }
                    }
                    4 => {
                        let raw = read_data(1 << 28)?;
                        if raw.len() >= 32 {
                            let first = le64(&raw, 0);
                            let n = le32(&raw, 8) as usize;
                            for i in 0..n {
                                let o = 32 + i * 16;
                                if o + 16 > raw.len() {
                                    break;
                                }
                                let off = le64(&raw, o);
                                let len = le32(&raw, o + 8);
                                let fl = le32(&raw, o + 12);
                                let kind = if fl & 4 != 0 {
                                    Kind::Pattern(off.to_le_bytes())
                                } else if fl & 1 != 0 {
                                    Kind::Packed(if method == Codec::Raw {
                                        Codec::Deflate
                                    } else {
                                        method
                                    })
                                } else {
                                    Kind::Stored {
                                        checksum: fl & 2 != 0,
                                    }
                                };
                                slots.insert(
                                    first + i as u64,
                                    Chunk {
                                        seg: si as u32,
                                        off,
                                        len,
                                        kind,
                                    },
                                );
                            }
                        }
                    }
                    8 if info.md5.is_none() => {
                        if let Ok(v) = read_data(16) {
                            if v.len() == 16 {
                                info.md5 = Some(hex(&v));
                            }
                        }
                    }
                    9 if info.sha1.is_none() => {
                        if let Ok(v) = read_data(20) {
                            if v.len() == 20 {
                                info.sha1 = Some(hex(&v));
                            }
                        }
                    }
                    _ => {}
                }
                if prev == 0 || prev >= pos || guard > 100_000 {
                    break;
                }
                pos = prev;
            }
        }
        if info.chunk_size == 0 {
            info.chunk_size = 32768;
        }
        let mut chunks = Vec::with_capacity(slots.len());
        for (i, (k, c)) in slots.into_iter().enumerate() {
            if k != i as u64 {
                return Err(Error::msg(
                    "segmento Ex01 ausente: sequência de chunks com lacuna",
                ));
            }
            chunks.push(c);
        }
        Self::finish(files, chunks, info, total_bytes)
    }

    fn decode(&self, idx: usize) -> io::Result<Vec<u8>> {
        let c = self.chunks[idx];
        let want = self.chunk_size as usize;
        let mut out = match c.kind {
            Kind::Pattern(p) => (0..want).map(|i| p[i % 8]).collect(),
            Kind::Stored { checksum } => {
                let mut v = vec![0u8; c.len as usize];
                pread_exact(&self.files[c.seg as usize], &mut v, c.off)?;
                if checksum && v.len() > 4 {
                    v.truncate(v.len() - 4);
                }
                v
            }
            Kind::Packed(codec) => {
                let mut raw = vec![0u8; c.len as usize];
                pread_exact(&self.files[c.seg as usize], &mut raw, c.off)?;
                let mut v = Vec::with_capacity(want);
                let r: io::Result<usize> = match codec {
                    Codec::Bzip2 => bzip2::read::BzDecoder::new(&raw[..])
                        .take(want as u64)
                        .read_to_end(&mut v),
                    _ => flate2::read::ZlibDecoder::new(&raw[..])
                        .take(want as u64)
                        .read_to_end(&mut v),
                };
                r.map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("chunk EWF {idx} corrompido: {e}"),
                    )
                })?;
                v
            }
        };
        out.truncate(want);
        Ok(out)
    }

    /// Verifica o hash MD5 do conteúdo contra o guardado na imagem (`None` se a imagem não o guarda).
    pub fn verify(&self) -> Result<Option<bool>> {
        let Some(stored) = self.info.md5.clone() else {
            return Ok(None);
        };
        let mut h = MultiHasher::new();
        let mut off = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        while off < self.size {
            let n = self.read_at(off, &mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            off += n as u64;
        }
        Ok(Some(h.finalize().md5 == stored))
    }
}

impl Source for EwfSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((self.size - offset) as usize);
        let mut done = 0;
        let mut cache = self.cache.lock().unwrap();
        while done < want {
            let pos = offset + done as u64;
            let idx = (pos / self.chunk_size) as usize;
            if idx >= self.chunks.len() {
                break;
            }
            if cache.as_ref().map(|c| c.0) != Some(idx) {
                *cache = Some((idx, self.decode(idx)?));
            }
            let data = &cache.as_ref().unwrap().1;
            let within = (pos % self.chunk_size) as usize;
            if within >= data.len() {
                // Chunk curto (último): o resto é zero até o fim lógico.
                let n = (self.chunk_size as usize - within).min(want - done);
                buf[done..done + n].fill(0);
                done += n;
                continue;
            }
            let n = (data.len() - within).min(want - done);
            buf[done..done + n].copy_from_slice(&data[within..within + n]);
            done += n;
        }
        Ok(done)
    }
}

fn inflate_all(raw: &[u8]) -> Vec<u8> {
    let mut v = vec![];
    if raw.len() > 2 && raw[0] == 0x78 {
        let _ = flate2::read::ZlibDecoder::new(raw)
            .take(1 << 24)
            .read_to_end(&mut v);
        if !v.is_empty() {
            return v;
        }
    }
    raw.to_vec()
}

fn utf16_text(b: &[u8]) -> String {
    let u: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u)
        .trim_start_matches('\u{feff}')
        .to_string()
}

fn header_key_name(k: &str) -> &str {
    match k {
        "c" | "cn" => "Número do caso",
        "n" | "en" => "Número da evidência",
        "a" | "nm" => "Descrição",
        "e" | "ex" => "Examinador",
        "t" | "nt" => "Notas",
        "m" => "Data de aquisição",
        "u" => "Data do sistema",
        "ov" | "os" => "Sistema operacional",
        "av" => "Versão do software",
        other => other,
    }
}

/// Texto do "header" (E01): linhas "1 / main / chaves / valores", separados por TAB.
fn parse_header_text(raw: &[u8], out: &mut BTreeMap<String, String>) {
    let text = if raw.len() >= 2 && (raw[0] == 0xFF && raw[1] == 0xFE || raw.get(1) == Some(&0)) {
        utf16_text(raw)
    } else {
        String::from_utf8_lossy(raw).to_string()
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() >= 4 {
        let keys: Vec<&str> = lines[2].split('\t').collect();
        let vals: Vec<&str> = lines[3].split('\t').collect();
        for (k, v) in keys.iter().zip(vals.iter()) {
            if !v.is_empty() && !matches!(*k, "p") {
                out.insert(header_key_name(k).to_string(), v.to_string());
            }
        }
    }
}

fn parse_v2_text(text: &str, info: &mut EwfInfo, total_bytes: &mut u64, ty: u32) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 4 {
        return;
    }
    let keys: Vec<&str> = lines[2].split('\t').collect();
    let vals: Vec<&str> = lines[3].split('\t').collect();
    for (k, v) in keys.iter().zip(vals.iter()) {
        if v.is_empty() {
            continue;
        }
        match (ty, *k) {
            (1, "ts") => info.sectors = v.parse().unwrap_or(0),
            (1, "bp") => info.bytes_per_sector = v.parse().unwrap_or(512),
            (2, "sb") => {
                let spc: u64 = v.parse().unwrap_or(64);
                info.chunk_size = spc * 512;
            }
            (2, "tb")
            | (2, "gr")
            | (_, "dt")
            | (_, "pid")
            | (_, "rs")
            | (_, "ls")
            | (_, "ph")
            | (_, "tt")
            | (_, "at") => {}
            (2, key) => {
                info.fields
                    .insert(header_key_name(key).to_string(), v.to_string());
            }
            _ => {}
        }
    }
    if info.bytes_per_sector > 0 {
        if info.chunk_size > 0 && info.bytes_per_sector != 512 {
            let spc = info.chunk_size / 512;
            info.chunk_size = spc * info.bytes_per_sector as u64;
        }
        *total_bytes = info.sectors * info.bytes_per_sector as u64;
    }
}

// ======================================================================
// Escrita de E01 (formato EnCase 6)
// ======================================================================

#[derive(Debug, Clone)]
pub struct E01Options {
    pub chunk_sectors: u32,
    pub bytes_per_sector: u32,
    /// 0 = sem compressão, 1..=9 = nível deflate.
    pub compression: u32,
    /// Tamanho máximo de cada segmento (0 = um segmento só).
    pub segment_size: u64,
    pub case_number: String,
    pub description: String,
    pub evidence_number: String,
    pub examiner: String,
    pub notes: String,
}

impl Default for E01Options {
    fn default() -> Self {
        Self {
            chunk_sectors: 64,
            bytes_per_sector: 512,
            compression: 1,
            segment_size: 1500 * 1024 * 1024,
            case_number: String::new(),
            description: String::new(),
            evidence_number: String::new(),
            examiner: String::new(),
            notes: String::new(),
        }
    }
}

fn section_desc(ty: &str, next: u64, size: u64) -> [u8; 76] {
    let mut d = [0u8; 76];
    d[..ty.len()].copy_from_slice(ty.as_bytes());
    d[16..24].copy_from_slice(&next.to_le_bytes());
    d[24..32].copy_from_slice(&size.to_le_bytes());
    let c = adler32(&d[..72]);
    d[72..76].copy_from_slice(&c.to_le_bytes());
    d
}

fn zlib(data: &[u8], level: u32) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::new(level.min(9)));
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

struct SegWriter {
    f: File,
    pos: u64,
}

impl SegWriter {
    fn put(&mut self, b: &[u8]) -> io::Result<()> {
        self.f.write_all(b)?;
        self.pos += b.len() as u64;
        Ok(())
    }
    /// Seção com dados completos (cabeçalho, volume, hash...).
    fn section(&mut self, ty: &str, data: &[u8], last: bool) -> io::Result<()> {
        let size = 76 + data.len() as u64;
        let next = if last { self.pos } else { self.pos + size };
        self.put(&section_desc(ty, next, size))?;
        self.put(data)
    }
}

fn header_text(o: &E01Options, now: &str) -> Vec<u8> {
    let t = format!(
        "1\nmain\nc\tn\ta\te\tt\tav\tov\tm\tu\tp\n{}\t{}\t{}\t{}\t{}\t{}\t{}\t{now}\t{now}\t0\n\n",
        o.case_number,
        o.evidence_number,
        o.description,
        o.examiner,
        o.notes,
        crate::VERSION,
        std::env::consts::OS
    );
    t.into_bytes()
}

fn volume_data(o: &E01Options, chunks: u32, sectors: u64) -> Vec<u8> {
    let mut v = vec![0u8; 1052];
    v[0] = 1; // disco fixo
    v[4..8].copy_from_slice(&chunks.to_le_bytes());
    v[8..12].copy_from_slice(&o.chunk_sectors.to_le_bytes());
    v[12..16].copy_from_slice(&o.bytes_per_sector.to_le_bytes());
    v[16..24].copy_from_slice(&sectors.to_le_bytes());
    v[36] = 1; // mídia física
    v[52] = if o.compression == 0 { 0 } else { 1 };
    v[56..60].copy_from_slice(&o.chunk_sectors.to_le_bytes());
    let c = adler32(&v[..1048]);
    v[1048..1052].copy_from_slice(&c.to_le_bytes());
    v
}

fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // Conversão civil (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y} {m} {d} {} {} {}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Cria `base.E01` (e `.E02`... se houver divisão) a partir de `src`.
/// Devolve o relatório com os hashes do conteúdo lido.
pub fn write_e01(
    src: &dyn Source,
    base: &Path,
    opts: &E01Options,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(u64, u64),
) -> Result<ImageReport> {
    let chunk_size = opts.chunk_sectors as u64 * opts.bytes_per_sector as u64;
    if chunk_size == 0 || chunk_size > (1 << 26) {
        return Err(Error::msg("tamanho de chunk EWF inválido"));
    }
    let total = src.size();
    let chunks_total = total.div_ceil(chunk_size);
    if chunks_total > u32::MAX as u64 {
        return Err(Error::msg("imagem grande demais para o formato E01"));
    }
    let now = utc_now();
    let mut rep = ImageReport::default();
    let mut hasher = MultiHasher::new();
    let mut seg_no = 1u32;
    let mut chunk_idx = 0u64;
    let mut buf = vec![0u8; chunk_size as usize];
    let mut created: Vec<PathBuf> = vec![];

    let seg_path = |n: u32| -> PathBuf {
        let ext = if n <= 99 {
            format!("E{n:02}")
        } else {
            let mut e = "E99".to_string();
            for _ in 99..n {
                e = next_segment_ext(&e).unwrap_or(e);
            }
            e
        };
        base.with_extension(ext)
    };

    // O segmento está cheio? (reserva espaço para as tabelas e os hashes finais)
    let full = |pos: u64, entries: u64| {
        opts.segment_size > 0
            && pos + chunk_size + 2 * (entries * 4 + 110) + 1024 >= opts.segment_size
    };
    let result = (|| -> Result<()> {
        loop {
            let path = seg_path(seg_no);
            let f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| {
                    Error::msg(format!("não foi possível criar {}: {e}", path.display()))
                })?;
            crate::owner::give_to_user(&path);
            created.push(path);
            let mut w = SegWriter { f, pos: 0 };
            let mut fh = Vec::from(SIG_V1);
            fh.push(1);
            fh.extend_from_slice(&(seg_no as u16).to_le_bytes());
            fh.extend_from_slice(&[0, 0]);
            w.put(&fh)?;
            if seg_no == 1 {
                let hdr = zlib(&header_text(opts, &now), 9);
                w.section("header", &hdr, false)?;
                w.section("header2", &hdr, false)?;
                w.section(
                    "volume",
                    &volume_data(
                        opts,
                        chunks_total as u32,
                        total.div_ceil(opts.bytes_per_sector as u64),
                    ),
                    false,
                )?;
            }
            // Grupos "sectors" + "table" + "table2" até estourar o limite do segmento.
            loop {
                let remaining = chunks_total - chunk_idx;
                if remaining == 0 {
                    break;
                }
                let sec_start = w.pos;
                // Espaço reservado para a descrição da seção "sectors" (preenchida depois).
                w.put(&[0u8; 76])?;
                let base_off = w.pos;
                let mut entries: Vec<u32> = vec![];
                let mut group_chunks = 0u64;
                while group_chunks < remaining && group_chunks < 16375 {
                    if cancelled() {
                        rep.cancelled = true;
                        return Ok(());
                    }
                    let off = chunk_idx * chunk_size;
                    let want = (total - off).min(chunk_size) as usize;
                    // Leitura tolerante: setor ruim vira zeros e entra no mapa de erros.
                    let b = &mut buf[..want];
                    if crate::source::read_exact_at(src, off, b).is_err() {
                        let mut o = 0;
                        while o < want {
                            let n = 512.min(want - o);
                            if crate::source::read_exact_at(src, off + o as u64, &mut b[o..o + n])
                                .is_err()
                            {
                                b[o..o + n].fill(0);
                                match rep.bad_ranges.last_mut() {
                                    Some(l) if l.0 + l.1 == off + o as u64 => l.1 += n as u64,
                                    _ => rep.bad_ranges.push((off + o as u64, n as u64)),
                                }
                            }
                            o += n;
                        }
                    }
                    hasher.update(b);
                    let rel = w.pos - base_off;
                    let packed = if opts.compression > 0 {
                        zlib(b, opts.compression)
                    } else {
                        vec![]
                    };
                    if opts.compression > 0 && packed.len() < want {
                        entries.push(rel as u32 | 0x8000_0000);
                        w.put(&packed)?;
                    } else {
                        entries.push(rel as u32);
                        w.put(b)?;
                        w.put(&adler32(b).to_le_bytes())?;
                    }
                    chunk_idx += 1;
                    group_chunks += 1;
                    progress(chunk_idx * chunk_size.min(total), total);
                    // Mantém cada tabela dentro de 31 bits e o segmento perto do limite.
                    if rel > 0x7000_0000 || full(w.pos, entries.len() as u64) {
                        break;
                    }
                }
                // Preenche a seção "sectors".
                let sec_end = w.pos;
                let table_size = 76 + 24 + entries.len() as u64 * 4 + 4;
                let sectors_desc = section_desc("sectors", sec_end, sec_end - sec_start);
                // Descrição "sectors" aponta para a tabela que vem logo depois.
                use std::io::{Seek, SeekFrom};
                w.f.seek(SeekFrom::Start(sec_start))?;
                w.f.write_all(&sectors_desc)?;
                w.f.seek(SeekFrom::Start(sec_end))?;
                // table e table2 idênticas.
                for ty in ["table", "table2"] {
                    let start = w.pos;
                    w.put(&section_desc(ty, start + table_size, table_size))?;
                    let mut h = Vec::with_capacity(24);
                    h.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                    h.extend_from_slice(&[0; 4]);
                    h.extend_from_slice(&base_off.to_le_bytes());
                    h.extend_from_slice(&[0; 4]);
                    let c = adler32(&h);
                    h.extend_from_slice(&c.to_le_bytes());
                    w.put(&h)?;
                    let mut e = Vec::with_capacity(entries.len() * 4);
                    for x in &entries {
                        e.extend_from_slice(&x.to_le_bytes());
                    }
                    w.put(&e)?;
                    w.put(&adler32(&e).to_le_bytes())?;
                }
                if rep.cancelled {
                    return Ok(());
                }
                if full(w.pos, 0) && chunk_idx < chunks_total {
                    break;
                }
            }
            if chunk_idx >= chunks_total {
                // Segmento final: hashes e "done".
                let h = std::mem::take(&mut hasher).finalize();
                let md5: Vec<u8> = (0..16)
                    .map(|i| u8::from_str_radix(&h.md5[i * 2..i * 2 + 2], 16).unwrap())
                    .collect();
                let sha1: Vec<u8> = (0..20)
                    .map(|i| u8::from_str_radix(&h.sha1[i * 2..i * 2 + 2], 16).unwrap())
                    .collect();
                let mut hs = md5.clone();
                hs.extend_from_slice(&[0; 16]);
                let c = adler32(&hs);
                hs.extend_from_slice(&c.to_le_bytes());
                w.section("hash", &hs, false)?;
                let mut dg = md5;
                dg.extend_from_slice(&sha1);
                dg.extend_from_slice(&[0; 40]);
                let c = adler32(&dg);
                dg.extend_from_slice(&c.to_le_bytes());
                w.section("digest", &dg, false)?;
                w.section("done", &[], true)?;
                rep.md5 = h.md5;
                rep.sha1 = h.sha1;
                rep.sha256 = h.sha256;
                rep.bytes = total;
                break;
            } else {
                w.section("next", &[], true)?;
                seg_no += 1;
            }
            w.f.flush()?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        for p in &created {
            let _ = std::fs::remove_file(p);
        }
        return Err(e);
    }
    if rep.cancelled {
        rep.bytes = chunk_idx * chunk_size;
    }
    crate::owner::write_owned(
        &base.with_extension("map.json"),
        serde_json::to_vec_pretty(&rep)?,
    )?;
    Ok(rep)
}
