//! Gravação dos arquivos recuperados (sempre em outro destino) com SHA-256
//! e manifesto da sessão.

use crate::error::{Error, Result};
use crate::fs::sanitize_name;
use crate::hashing::{Hashes, MultiHasher};
use crate::source::{read_vec, Source};
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveredFile {
    pub id: u64,
    pub path: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub md5: String,
    #[serde(default)]
    pub sha1: String,
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
    /// Arquivos já recuperados e conferidos numa execução anterior (pulados ao retomar).
    #[serde(default)]
    pub skipped: usize,
}

/// Recusa destinos que ficam no mesmo dispositivo da origem.
pub fn check_destination(source_path: Option<&Path>, dest: &Path) -> Result<()> {
    crate::owner::create_dir_all_owned(dest)?;
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

/// Como organizar os arquivos recuperados dentro do destino.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecoverLayout {
    /// Pastas originais quando conhecidas; o resto separado por tipo (padrão).
    #[default]
    Folders,
    /// Uma pasta por tipo (Fotos, Documentos, Vídeos...).
    Type,
    /// Pastas por ano e mês (data da foto, ou a de modificação).
    Date,
    /// Tudo direto na pasta de destino.
    Flat,
}

impl RecoverLayout {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "folders" | "pastas" => Self::Folders,
            "type" | "tipo" => Self::Type,
            "date" | "data" => Self::Date,
            "flat" | "plano" => Self::Flat,
            _ => return None,
        })
    }
}

pub fn recover(
    src: &dyn Source,
    files: &[&Found],
    dest: &Path,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(usize, usize, u64),
) -> Result<RecoverReport> {
    recover_with(
        src,
        files,
        dest,
        RecoverLayout::Folders,
        cancelled,
        progress,
    )
}

pub fn recover_with(
    src: &dyn Source,
    files: &[&Found],
    dest: &Path,
    layout: RecoverLayout,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(usize, usize, u64),
) -> Result<RecoverReport> {
    recover_resumable(src, files, dest, layout, "", false, cancelled, progress)
}

/// Uma linha do diário de recuperação (`guara-diario.jsonl`): cada arquivo gravado e conferido.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalEntry {
    source: String,
    id: u64,
    name: String,
    file: RecoveredFile,
}

pub const JOURNAL: &str = "guara-diario.jsonl";

fn sha256_of_file(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = MultiHasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().sha256)
}

/// Como `recover_with`, mas mantém um diário e, com `resume`, pula o que já foi
/// recuperado (e conferido por SHA-256) numa execução interrompida.
/// `source_key` identifica a origem para que o diário de outro disco não seja reaproveitado.
#[allow(clippy::too_many_arguments)]
pub fn recover_resumable(
    src: &dyn Source,
    files: &[&Found],
    dest: &Path,
    layout: RecoverLayout,
    source_key: &str,
    resume: bool,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(usize, usize, u64),
) -> Result<RecoverReport> {
    use std::io::Write;
    crate::owner::create_dir_all_owned(dest)?;
    let mut report = RecoverReport {
        destination: dest.display().to_string(),
        ..Default::default()
    };
    let mut used: HashSet<PathBuf> = HashSet::new();
    let journal_path = dest.join(JOURNAL);
    let mut prior: std::collections::HashMap<u64, JournalEntry> = std::collections::HashMap::new();
    if resume {
        if let Ok(text) = std::fs::read_to_string(&journal_path) {
            for line in text.lines() {
                if let Ok(e) = serde_json::from_str::<JournalEntry>(line) {
                    if e.source == source_key {
                        prior.insert(e.id, e);
                    }
                }
            }
        }
    }
    let mut journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal_path)
        .ok();
    crate::owner::give_to_user(&journal_path);
    for (i, f) in files.iter().enumerate() {
        if cancelled() {
            report.cancelled = true;
            break;
        }
        progress(i, files.len(), report.total_bytes);
        // retomada: já está lá e confere?
        if let Some(e) = prior.get(&f.id) {
            let p = dest.join(&e.file.path);
            if e.name == f.name && e.file.size == f.size && p.is_file() {
                if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) == f.size
                    && sha256_of_file(&p).as_deref() == Some(e.file.sha256.as_str())
                {
                    used.insert(p);
                    report.skipped += 1;
                    report.total_bytes += f.size;
                    report.recovered.push(e.file.clone());
                    continue;
                }
                // incompleto ou alterado: refaz (é um arquivo nosso)
                let _ = std::fs::remove_file(&p);
            }
        }
        let rel = target_rel(f, layout);
        let mut path = dest.join(&rel);
        if let Some(parent) = path.parent() {
            if let Err(e) = crate::owner::create_dir_all_owned(parent) {
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
            Ok(h) => {
                report.total_bytes += f.size;
                let rec = RecoveredFile {
                    id: f.id,
                    path: path
                        .strip_prefix(dest)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    size: f.size,
                    sha256: h.sha256,
                    md5: h.md5,
                    sha1: h.sha1,
                    confidence: f.confidence,
                    origin: f.origin,
                    partial: f.confidence == Confidence::Low,
                };
                if let Some(j) = journal.as_mut() {
                    let line = JournalEntry {
                        source: source_key.to_string(),
                        id: f.id,
                        name: f.name.clone(),
                        file: rec.clone(),
                    };
                    if let Ok(mut l) = serde_json::to_string(&line) {
                        l.push('\n');
                        let _ = j.write_all(l.as_bytes());
                        let _ = j.flush();
                    }
                }
                report.recovered.push(rec);
            }
            Err(e) => report.failed.push((f.id, e.to_string())),
        }
    }
    progress(files.len(), files.len(), report.total_bytes);
    let manifest = serde_json::json!({
        "ferramenta": "Guará",
        "versao": env!("CARGO_PKG_VERSION"),
        "gerado_em": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "algoritmos_hash": ["MD5", "SHA-1", "SHA-256"],
        "relatorio": &report,
    });
    crate::owner::write_owned(
        &dest.join("guara-manifesto.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(report)
}

fn target_rel(f: &Found, layout: RecoverLayout) -> PathBuf {
    let mut p = PathBuf::new();
    match layout {
        RecoverLayout::Folders => match &f.dir {
            Some(d) if !d.is_empty() => {
                p.push("Recuperados");
                for c in d.split('/') {
                    if !c.is_empty() {
                        p.push(sanitize_name(c));
                    }
                }
            }
            _ => p.push(f.category.label_pt()),
        },
        RecoverLayout::Type => p.push(f.category.label_pt()),
        RecoverLayout::Date => match f.when() {
            Some(t) => {
                let (y, m, _) = crate::meta::ymd(t);
                p.push(format!("{y:04}"));
                p.push(format!("{y:04}-{m:02}"));
            }
            None => p.push("Sem data"),
        },
        RecoverLayout::Flat => {}
    }
    p.push(sanitize_name(&f.name));
    p
}

/// Espaço livre (bytes) no volume onde `path` está (ou estaria).
pub fn free_space(path: &Path) -> Option<u64> {
    let mut p = path;
    while !p.exists() {
        p = p.parent()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c` é uma string C válida e `st` é uma estrutura de saída local.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = p
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free = 0u64;
        // SAFETY: `wide` termina em NUL e o ponteiro de saída é válido.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free)
    }
}

/// Escreve o conteúdo e devolve o SHA-256.
pub fn write_file(src: &dyn Source, f: &Found, path: &Path) -> Result<Hashes> {
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    crate::owner::give_to_user(path);
    let mut hasher = MultiHasher::new();
    match &f.data {
        DataLoc::Resident { bytes } => {
            out.write_all(bytes)?;
            hasher.update(bytes);
        }
        DataLoc::Parts { parts } => {
            for part in parts {
                match part {
                    Part::Bytes { bytes } => {
                        out.write_all(bytes)?;
                        hasher.update(bytes);
                    }
                    Part::Run { offset, len } => {
                        let mut done = 0u64;
                        while done < *len {
                            let n = (*len - done).min(1 << 20) as usize;
                            let got = read_vec(src, offset + done, n)?;
                            if got.len() < n {
                                return Err(Error::msg("leitura além do fim da origem"));
                            }
                            out.write_all(&got)?;
                            hasher.update(&got);
                            done += n as u64;
                        }
                    }
                }
            }
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
    Ok(hasher.finalize())
}

/// Lê `buf.len()` bytes do conteúdo lógico de um achado a partir de `offset`
/// (usado para streaming/pré-visualização sem gravar em disco).
pub fn read_found_at(
    src: &dyn Source,
    f: &Found,
    offset: u64,
    buf: &mut [u8],
) -> std::io::Result<usize> {
    if offset >= f.size || buf.is_empty() {
        return Ok(0);
    }
    let want = buf.len().min((f.size - offset) as usize);
    match &f.data {
        DataLoc::Resident { bytes } => {
            let o = offset as usize;
            let n = want.min(bytes.len().saturating_sub(o));
            buf[..n].copy_from_slice(&bytes[o..o + n]);
            Ok(n)
        }
        DataLoc::Parts { parts } => {
            let mut pos = 0u64;
            for p in parts {
                let plen = p.len();
                if offset < pos + plen {
                    let within = offset - pos;
                    let n = want.min((plen - within) as usize);
                    return match p {
                        Part::Bytes { bytes } => {
                            buf[..n].copy_from_slice(&bytes[within as usize..within as usize + n]);
                            Ok(n)
                        }
                        Part::Run { offset: o, .. } => src.read_at(o + within, &mut buf[..n]),
                    };
                }
                pos += plen;
            }
            Ok(0)
        }
        DataLoc::Extents { extents } => {
            let mut pos = 0u64;
            for e in extents {
                let elen = e.len.min(f.size.saturating_sub(pos));
                if offset < pos + elen {
                    let within = offset - pos;
                    let n = want.min((elen - within) as usize);
                    if e.sparse {
                        buf[..n].fill(0);
                        return Ok(n);
                    }
                    return src.read_at(e.offset + within, &mut buf[..n]);
                }
                pos += elen;
            }
            Ok(0)
        }
    }
}

/// Um arquivo achado visto como origem de leitura posicional (para leitores de formato).
pub struct FoundSource<'a> {
    pub src: &'a dyn Source,
    pub found: &'a Found,
}

impl Source for FoundSource<'_> {
    fn size(&self) -> u64 {
        self.found.size
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        read_found_at(self.src, self.found, offset, buf)
    }
}
