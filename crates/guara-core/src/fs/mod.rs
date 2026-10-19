//! Leitores de metadados de sistemas de arquivos (somente leitura).
//!
//! Cada leitor percorre estruturas do volume atrás de arquivos apagados,
//! devolve os extents ainda alocados ("live") para o carving pular e, quando
//! possível, os trechos livres.

pub mod apfs;
pub mod btrfs;
pub mod exfat;
pub mod ext;
pub mod fat;
pub mod hfsplus;
pub mod ntfs;
pub mod xfs;

use crate::signatures::{category_for_ext, matches_ext};
use crate::source::{read_vec, Source};
use crate::types::{Confidence, DataLoc, Extent, Found, Origin};
use std::sync::atomic::{AtomicU64, Ordering};

/// Contexto compartilhado por todos os leitores.
pub struct Ctx<'a> {
    pub src: &'a dyn Source,
    pub next_id: &'a AtomicU64,
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    /// (feito, total) dentro da fase atual do leitor.
    pub progress: &'a (dyn Fn(u64, u64) + Sync),
    pub emit: &'a mut dyn FnMut(Found),
}

impl Ctx<'_> {
    pub fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

/// Resultado de varrer um volume.
#[derive(Default)]
pub struct VolumeScan {
    /// Extents (absolutos) ocupados por arquivos existentes e metadados.
    pub live: Vec<Extent>,
    /// Se o leitor conhece o mapa de espaço livre (ext), os trechos livres (absolutos).
    pub free: Option<Vec<Extent>>,
    pub warnings: Vec<String>,
}

pub struct NewFound<'a> {
    pub name: String,
    pub dir: Option<String>,
    pub size: u64,
    pub origin: Origin,
    pub data: DataLoc,
    pub modified: Option<i64>,
    pub created: Option<i64>,
    pub accessed: Option<i64>,
    pub base: Confidence,
    pub note: Option<&'a str>,
}

/// Monta um `Found`, ajustando a confiança pelo conteúdo real no disco.
pub fn make_found(ctx: &Ctx, n: NewFound) -> Found {
    let ext = n
        .name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_default();
    let mut conf = n.base;
    let mut note = n.note.map(String::from);
    let head: Vec<u8> = match &n.data {
        DataLoc::Resident { bytes } => bytes.iter().take(32).copied().collect(),
        DataLoc::Parts { .. } => vec![],
        DataLoc::Extents { extents } => extents
            .iter()
            .find(|e| !e.sparse)
            .and_then(|e| read_vec(ctx.src, e.offset, 32.min(e.len as usize)).ok())
            .unwrap_or_default(),
    };
    if n.size > 0
        && !head.is_empty()
        && head.iter().all(|&b| b == 0)
        && !matches!(n.data, DataLoc::Resident { .. })
    {
        conf = Confidence::Low;
        note = Some("início do arquivo zerado (apagado/TRIM ou sobrescrito)".into());
    } else {
        match matches_ext(&ext, &head) {
            Some(false) => {
                conf = Confidence::Low;
                note =
                    Some("conteúdo não confere com a extensão: provavelmente sobrescrito".into());
            }
            Some(true) => {}
            None => conf = conf.min(Confidence::Medium),
        }
    }
    Found {
        id: ctx.id(),
        category: category_for_ext(&ext),
        ext,
        name: n.name,
        dir: n.dir,
        size: n.size,
        origin: n.origin,
        confidence: conf,
        data: n.data,
        modified: n.modified,
        created: n.created,
        accessed: n.accessed,
        taken: None,
        camera: None,
        px: None,
        gps: false,
        sig: None,
        note,
    }
}

/// Ordena e funde extents adjacentes/sobrepostos.
pub fn merge_extents(mut v: Vec<Extent>) -> Vec<Extent> {
    v.retain(|e| e.len > 0);
    v.sort_by_key(|e| e.offset);
    let mut out: Vec<Extent> = Vec::with_capacity(v.len());
    for e in v {
        if let Some(last) = out.last_mut() {
            if e.offset <= last.end() {
                if e.end() > last.end() {
                    last.len = e.end() - last.offset;
                }
                continue;
            }
        }
        out.push(e);
    }
    out
}

/// Há sobreposição entre [off, off+len) e algum extent de `sorted` (já mesclado)?
pub fn overlaps(sorted: &[Extent], off: u64, len: u64) -> bool {
    let i = sorted.partition_point(|e| e.end() <= off);
    sorted.get(i).map(|e| e.offset < off + len).unwrap_or(false)
}

pub fn dos_to_unix(date: u16, time: u16) -> Option<i64> {
    if date == 0 {
        return None;
    }
    let year = 1980 + (date >> 9) as i64;
    let month = ((date >> 5) & 0xF) as i64;
    let day = (date & 0x1F) as i64;
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let (h, m, s) = (
        (time >> 11) as i64,
        ((time >> 5) & 0x3F) as i64,
        ((time & 0x1F) * 2) as i64,
    );
    Some(days_from_civil(year, month, day) * 86400 + h * 3600 + m * 60 + s)
}

/// Dias desde 1970-01-01 (algoritmo de Howard Hinnant).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn utf16le(bytes: &[u8]) -> String {
    let u: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u)
}

pub fn sanitize_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() {
        "arquivo".into()
    } else {
        t
    }
}
