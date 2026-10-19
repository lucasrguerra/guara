//! Imagem forense de um disco: leitura tolerante a setores ruins, com mapa de
//! erros e hash. Serve para trabalhar sempre sobre a cópia.

use crate::error::{Error, Result};
use crate::hashing::MultiHasher;
use crate::source::Source;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageReport {
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub md5: String,
    #[serde(default)]
    pub sha1: String,
    /// (offset, tamanho) dos trechos ilegíveis, preenchidos com zeros.
    pub bad_ranges: Vec<(u64, u64)>,
    pub cancelled: bool,
}

const BLOCK: usize = 1 << 20;
const SECTOR: usize = 512;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Raw,
    /// Expert Witness (EnCase 6): `.E01`, dividido em segmentos.
    E01 {
        #[serde(default)]
        case_number: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        evidence_number: String,
        #[serde(default)]
        examiner: String,
        #[serde(default)]
        notes: String,
        /// Tamanho máximo de cada segmento em MiB (0 = um arquivo só).
        #[serde(default = "default_segment_mib")]
        segment_mib: u64,
        /// 0 = sem compressão, 1 a 9 = deflate.
        #[serde(default = "default_compression")]
        compression: u32,
    },
}

fn default_segment_mib() -> u64 {
    1500
}
fn default_compression() -> u32 {
    1
}

pub fn create_image_as(
    src: &dyn Source,
    dest: &Path,
    format: &ImageFormat,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(u64, u64),
) -> Result<ImageReport> {
    match format {
        ImageFormat::Raw => create_image(src, dest, cancelled, progress),
        ImageFormat::E01 { case_number, description, evidence_number, examiner, notes, segment_mib, compression } => {
            let opts = crate::ewf::E01Options {
                case_number: case_number.clone(),
                description: description.clone(),
                evidence_number: evidence_number.clone(),
                examiner: examiner.clone(),
                notes: notes.clone(),
                segment_size: segment_mib * 1024 * 1024,
                compression: *compression,
                ..Default::default()
            };
            // O destino pode vir com ou sem a extensão .E01.
            let base = match dest.extension().and_then(|e| e.to_str()) {
                Some(e) if e.eq_ignore_ascii_case("e01") => dest.with_extension(""),
                _ => dest.to_path_buf(),
            };
            crate::ewf::write_e01(src, &base, &opts, cancelled, progress)
        }
    }
}

pub fn create_image(
    src: &dyn Source,
    dest: &Path,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(u64, u64),
) -> Result<ImageReport> {
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| {
            Error::msg(format!(
                "não foi possível criar {}: {e} (o arquivo não pode existir)",
                dest.display()
            ))
        })?;
    crate::owner::give_to_user(dest);
    let size = src.size();
    let mut rep = ImageReport::default();
    let mut hasher = MultiHasher::new();
    let mut buf = vec![0u8; BLOCK];
    let mut pos = 0u64;
    while pos < size {
        if cancelled() {
            rep.cancelled = true;
            break;
        }
        let want = (size - pos).min(BLOCK as u64) as usize;
        match read_full(src, pos, &mut buf[..want]) {
            Ok(()) => {}
            Err(_) => {
                // Refaz setor a setor para isolar os trechos ruins.
                let mut o = 0;
                while o < want {
                    let n = SECTOR.min(want - o);
                    if read_full(src, pos + o as u64, &mut buf[o..o + n]).is_err() {
                        buf[o..o + n].fill(0);
                        match rep.bad_ranges.last_mut() {
                            Some(l) if l.0 + l.1 == pos + o as u64 => l.1 += n as u64,
                            _ => rep.bad_ranges.push((pos + o as u64, n as u64)),
                        }
                    }
                    o += n;
                }
            }
        }
        out.write_all(&buf[..want])?;
        hasher.update(&buf[..want]);
        pos += want as u64;
        progress(pos, size);
    }
    out.flush()?;
    rep.bytes = pos;
    let h = hasher.finalize();
    rep.sha256 = h.sha256;
    rep.md5 = h.md5;
    rep.sha1 = h.sha1;
    let map = dest.with_extension("map.json");
    crate::owner::write_owned(&map, serde_json::to_vec_pretty(&rep)?)?;
    Ok(rep)
}

fn read_full(src: &dyn Source, off: u64, buf: &mut [u8]) -> std::io::Result<()> {
    crate::source::read_exact_at(src, off, buf)
}
