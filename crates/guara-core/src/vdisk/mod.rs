//! Imagens de disco virtual e contêineres: QCOW2, VMDK, VHD, VHDX e DMG (UDIF).
//!
//! Cada formato é aberto como o disco que contém, somente para leitura, e entra na cadeia
//! normal (partições, sistemas de arquivos, cifras) por meio de [`Source`]. Discos
//! diferenciais (VHD e VHDX com pai) seguem a cadeia até o disco base; QCOW2 e VMDK com
//! arquivo de apoio são recusados com mensagem clara, porque ler só a camada filha
//! devolveria dados errados.

mod dmg;
mod qcow2;
mod vhd;
mod vhdx;
mod vmdk;

use crate::error::{Error, Result};
use crate::source::{FileSource, Source};
use std::io;
use std::path::Path;

pub(crate) fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes(b[o..o + 2].try_into().unwrap())
}
pub(crate) fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}
pub(crate) fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
pub(crate) fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Leitura dentro de um único bloco do formato; devolve quantos bytes entregou (pode ser
/// menos que `buf.len()`, nunca zero antes do fim do disco).
pub(crate) trait Backend: Send + Sync {
    fn size(&self) -> u64;
    fn read_chunk(&self, voff: u64, buf: &mut [u8]) -> io::Result<usize>;
}

/// Disco virtual aberto de uma imagem de contêiner.
pub struct VDisk {
    inner: Box<dyn Backend>,
    format: &'static str,
}

impl VDisk {
    /// Nome do formato (`QCOW2`, `VMDK`, `VHD`, `VHDX`, `DMG`).
    pub fn format(&self) -> &'static str {
        self.format
    }
}

impl Source for VDisk {
    fn size(&self) -> u64 {
        self.inner.size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let size = self.inner.size();
        if offset >= size {
            return Ok(0);
        }
        let want = buf.len().min((size - offset) as usize);
        let mut done = 0;
        while done < want {
            let n = self
                .inner
                .read_chunk(offset + done as u64, &mut buf[done..want])?;
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }
}

pub(crate) fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

pub(crate) fn io_err(e: io::Error, what: &str) -> Error {
    Error::msg(format!("{what}: {e}"))
}

/// Cadeias de arquivos pai (VHD/VHDX diferenciais) não passam deste limite, o que também
/// corta ciclos.
const MAX_DEPTH: u32 = 16;

/// Formato de contêiner reconhecido pelos primeiros bytes (e, para VHD fixo e DMG, pelo
/// fim do arquivo). `None` quando é um disco bruto.
pub fn detect(file: &FileSource, head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"QFI\xfb") {
        return Some("QCOW2");
    }
    if head.starts_with(b"KDMV") || head.starts_with(b"# Disk DescriptorFile") {
        return Some("VMDK");
    }
    if head.starts_with(b"vhdxfile") {
        return Some("VHDX");
    }
    if head.starts_with(b"conectix") || vhd::is_fixed(file) {
        return Some("VHD");
    }
    if dmg::is_encrypted(head) || dmg::is_dmg(file) {
        return Some("DMG");
    }
    None
}

/// Abre `path` como disco virtual, ou `Ok(None)` se não for um contêiner conhecido.
pub fn open(path: &Path) -> Result<Option<VDisk>> {
    open_depth(path, 0)
}

pub(crate) fn open_depth(path: &Path, depth: u32) -> Result<Option<VDisk>> {
    if depth > MAX_DEPTH {
        return Err(Error::msg("cadeia de discos pai longa demais ou circular"));
    }
    let file = FileSource::open(path)?;
    let mut head = [0u8; 96];
    let got = file.read_at(0, &mut head).unwrap_or(0);
    let Some(format) = detect(&file, &head[..got]) else {
        return Ok(None);
    };
    let name = format!("{format} {}", path.display());
    let inner: Box<dyn Backend> = match format {
        "QCOW2" => Box::new(qcow2::Qcow2::open(file).map_err(|e| io_err(e, &name))?),
        "VMDK" => Box::new(vmdk::Vmdk::open(file, path).map_err(|e| io_err(e, &name))?),
        "VHD" => Box::new(vhd::Vhd::open(file, path, depth).map_err(|e| io_err(e, &name))?),
        "VHDX" => Box::new(vhdx::Vhdx::open(file, path, depth).map_err(|e| io_err(e, &name))?),
        _ => Box::new(dmg::Dmg::open(file).map_err(|e| io_err(e, &name))?),
    };
    Ok(Some(VDisk { inner, format }))
}

/// Abre o disco pai de um VHD/VHDX diferencial.
pub(crate) fn open_parent(
    candidates: &[std::path::PathBuf],
    depth: u32,
) -> io::Result<Box<dyn Source>> {
    for p in candidates {
        if !p.is_file() {
            continue;
        }
        if let Some(v) = open_depth(p, depth + 1).map_err(|e| bad(e.to_string()))? {
            return Ok(Box::new(v));
        }
        let f = FileSource::open(p)?;
        return Ok(Box::new(f));
    }
    Err(bad(format!(
        "disco pai não encontrado (procurado: {})",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// Lê de um arquivo por posição e exige todos os bytes.
pub(crate) fn read_exact(f: &FileSource, off: u64, buf: &mut [u8]) -> io::Result<()> {
    crate::source::read_exact_at(f, off, buf)
}

pub(crate) fn read_vec(f: &FileSource, off: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut v = vec![0u8; len];
    read_exact(f, off, &mut v)?;
    Ok(v)
}

/// Tabelas em memória têm teto: um cabeçalho adulterado não pode pedir gigabytes.
pub(crate) fn check_table(entries: u64, entry_size: u64, what: &str) -> io::Result<usize> {
    const MAX: u64 = 512 * 1024 * 1024;
    if entries.saturating_mul(entry_size) > MAX {
        return Err(bad(format!("{what} grande demais ({entries} entradas)")));
    }
    Ok(entries as usize)
}

/// UTF-16 (big ou little endian) até o primeiro nulo.
pub(crate) fn utf16(b: &[u8], big: bool) -> String {
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| if big { be16(c, 0) } else { le16(c, 0) })
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Caminhos onde procurar o disco pai: o caminho gravado (relativo ao arquivo filho, com `\`
/// virando `/`) e, como alternativa, só o nome do arquivo na mesma pasta.
pub(crate) fn candidate_paths(dir: &Path, names: &[String]) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = vec![];
    for n in names {
        let n = n.trim().replace('\\', "/");
        let n = n.strip_prefix("file:///").unwrap_or(&n).to_string();
        if n.is_empty() {
            continue;
        }
        let rel = n.trim_start_matches("./");
        let full = std::path::PathBuf::from(rel);
        for p in [
            if full.is_absolute() {
                full.clone()
            } else {
                dir.join(rel)
            },
            dir.join(rel.rsplit('/').next().unwrap_or(rel)),
        ] {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Lê do disco pai (ou zeros, sem pai); o que o pai não cobre também é zero.
pub(crate) fn read_parent(
    parent: &Option<Box<dyn Source>>,
    voff: u64,
    buf: &mut [u8],
) -> io::Result<()> {
    let Some(p) = parent else {
        buf.fill(0);
        return Ok(());
    };
    let mut done = 0;
    while done < buf.len() {
        let n = p.read_at(voff + done as u64, &mut buf[done..])?;
        if n == 0 {
            buf[done..].fill(0);
            break;
        }
        done += n;
    }
    Ok(())
}
