//! Gravação dos arquivos recuperados (sempre em outro destino) com SHA-256
//! e manifesto da sessão.

use crate::error::{Error, Result};
use crate::fs::sanitize_name;
use crate::source::{read_vec, Source};
use crate::types::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveredFile {
    pub id: u64,
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub confidence: Confidence,
    pub origin: Origin,
    pub partial: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecoverReport {
    pub destination: String,
    pub recovered: Vec<RecoveredFile>,
    pub failed: Vec<(u64, String)>,
    pub total_bytes: u64,
    pub cancelled: bool,
}

/// Recusa destinos que ficam no mesmo dispositivo da origem.
pub fn check_destination(source_path: Option<&Path>, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let dest = dest.canonicalize()?;
    let Some(sp) = source_path else { return Ok(()) };
    let sp_str = sp.to_string_lossy().to_string();
    for (dev, mnt) in crate::device::mount_table() {
        let same = dev == sp_str
            || (dev.starts_with(&sp_str)
                && dev[sp_str.len()..]
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit() || c == 'p' || c == 's')
                    .unwrap_or(false));
        if same {
            let m = Path::new(&mnt);
            if dest.starts_with(m) && m != Path::new("/") {
                return Err(Error::msg(format!(
                    "O destino {} fica em {} (montado a partir da origem {}). Escolha outro disco para não sobrescrever dados recuperáveis.",
                    dest.display(),
                    mnt,
                    sp_str
                )));
            }
            if m == Path::new("/") {
                return Err(Error::msg(format!("A origem {sp_str} contém o sistema (/). Recupere para um disco externo ou use uma imagem.")));
            }
        }
    }
    if let Ok(meta) = std::fs::metadata(sp) {
        if meta.is_file() {
            // Imagem: evita gravar ao lado só se for o mesmo arquivo (não faz sentido), mas aceita.
        }
    }
    Ok(())
}

pub fn recover(
    src: &dyn Source,
    files: &[&Found],
    dest: &Path,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(usize, usize, u64),
) -> Result<RecoverReport> {
    std::fs::create_dir_all(dest)?;
    let mut report = RecoverReport {
        destination: dest.display().to_string(),
        ..Default::default()
    };
    let mut used: HashSet<PathBuf> = HashSet::new();
    for (i, f) in files.iter().enumerate() {
        if cancelled() {
            report.cancelled = true;
            break;
        }
        progress(i, files.len(), report.total_bytes);
        let rel = target_rel(f);
        let mut path = dest.join(&rel);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                report.failed.push((f.id, e.to_string()));
                continue;
            }
        }
        let mut n = 1;
        while path.exists() || used.contains(&path) {
            let stem = Path::new(&f.name)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let name = if f.ext.is_empty() {
                format!("{stem} ({n})")
            } else {
                format!("{stem} ({n}).{}", f.ext)
            };
            path = path.with_file_name(sanitize_name(&name));
            n += 1;
        }
        used.insert(path.clone());
        match write_file(src, f, &path) {
            Ok(hash) => {
                report.total_bytes += f.size;
                report.recovered.push(RecoveredFile {
                    id: f.id,
                    path: path
                        .strip_prefix(dest)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    size: f.size,
                    sha256: hash,
                    confidence: f.confidence,
                    origin: f.origin,
                    partial: f.confidence == Confidence::Low,
                });
            }
            Err(e) => report.failed.push((f.id, e.to_string())),
        }
    }
    progress(files.len(), files.len(), report.total_bytes);
    let manifest = serde_json::json!({
        "ferramenta": "Guará",
        "versao": env!("CARGO_PKG_VERSION"),
        "gerado_em": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "algoritmo_hash": "SHA-256",
        "relatorio": &report,
    });
    std::fs::write(
        dest.join("guara-manifesto.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(report)
}

fn target_rel(f: &Found) -> PathBuf {
    let mut p = PathBuf::new();
    match &f.dir {
        Some(d) if !d.is_empty() => {
            p.push("Recuperados");
            for c in d.split('/') {
                if !c.is_empty() {
                    p.push(sanitize_name(c));
                }
            }
        }
        _ => {
            p.push(f.category.label_pt());
        }
    }
    p.push(sanitize_name(&f.name));
    p
}

/// Escreve o conteúdo e devolve o SHA-256.
pub fn write_file(src: &dyn Source, f: &Found, path: &Path) -> Result<String> {
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let mut hasher = Sha256::new();
    match &f.data {
        DataLoc::Resident { bytes } => {
            out.write_all(bytes)?;
            hasher.update(bytes);
        }
        DataLoc::Extents { extents } => {
            let mut left = f.size;
            let mut buf = vec![0u8; 1 << 20];
            for e in extents {
                let mut done = 0u64;
                while done < e.len && left > 0 {
                    let n = (e.len - done).min(left).min(buf.len() as u64) as usize;
                    if e.sparse {
                        buf[..n].fill(0);
                    } else {
                        let got = read_vec(src, e.offset + done, n)?;
                        if got.len() < n {
                            return Err(Error::msg("leitura além do fim da origem"));
                        }
                        buf[..n].copy_from_slice(&got);
                    }
                    out.write_all(&buf[..n])?;
                    hasher.update(&buf[..n]);
                    done += n as u64;
                    left -= n as u64;
                }
            }
        }
    }
    if let Some(m) = f.modified.filter(|m| *m > 0) {
        let _ = out.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(m as u64));
    }
    out.flush()?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
