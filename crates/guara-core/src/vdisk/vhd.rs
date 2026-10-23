//! VHD (Microsoft Virtual PC e Hyper-V): discos fixos, dinâmicos e diferenciais. O disco
//! diferencial lê o pai (achado pelos localizadores do cabeçalho) nos setores que a camada
//! filha não preencheu.

use super::{
    bad, be32, be64, candidate_paths, check_table, open_parent, read_exact, read_parent, read_vec,
    utf16, Backend,
};
use crate::source::{FileSource, Source};
use std::io;
use std::path::Path;

const SECTOR: u64 = 512;
const NONE: u32 = 0xFFFF_FFFF;

enum Kind {
    Fixed,
    Dynamic {
        bat: Vec<u32>,
        block_size: u64,
        bitmap_size: u64,
    },
}

pub(super) struct Vhd {
    file: FileSource,
    size: u64,
    kind: Kind,
    parent: Option<Box<dyn Source>>,
}

fn footer_ok(f: &[u8]) -> bool {
    if f.len() < 512 || &f[..8] != b"conectix" {
        return false;
    }
    let mut sum = 0u32;
    for (i, &b) in f[..512].iter().enumerate() {
        if !(64..68).contains(&i) {
            sum = sum.wrapping_add(b as u32);
        }
    }
    !sum == be32(f, 64)
}

/// VHD fixo: disco bruto seguido de um rodapé de 512 bytes.
pub(super) fn is_fixed(file: &FileSource) -> bool {
    let size = file.size();
    if size < 2 * SECTOR {
        return false;
    }
    let mut f = [0u8; 512];
    read_exact(file, size - SECTOR, &mut f).is_ok()
        && footer_ok(&f)
        && be32(&f, 60) == 2
        && be64(&f, 16) == u64::MAX
}

impl Vhd {
    pub(super) fn open(file: FileSource, path: &Path, depth: u32) -> io::Result<Self> {
        let size = file.size();
        let mut footer = [0u8; 512];
        let tail_ok = size >= SECTOR && {
            read_exact(&file, size - SECTOR, &mut footer)?;
            footer_ok(&footer)
        };
        if !tail_ok {
            read_exact(&file, 0, &mut footer)?;
            if !footer_ok(&footer) {
                return Err(bad("rodapé VHD inválido"));
            }
        }
        let current = be64(&footer, 48);
        let disk_type = be32(&footer, 60);
        match disk_type {
            2 => {
                return Ok(Self {
                    file,
                    size: current,
                    kind: Kind::Fixed,
                    parent: None,
                });
            }
            3 | 4 => {}
            t => return Err(bad(format!("tipo de VHD {t} desconhecido"))),
        }
        let dh = read_vec(&file, be64(&footer, 16), 1024)?;
        if &dh[..8] != b"cxsparse" {
            return Err(bad("cabeçalho de disco dinâmico VHD inválido"));
        }
        let bat_off = be64(&dh, 16);
        let entries = be32(&dh, 28) as u64;
        let block_size = be32(&dh, 32) as u64;
        if block_size < SECTOR || !block_size.is_power_of_two() {
            return Err(bad("tamanho de bloco VHD inválido"));
        }
        let n = check_table(entries, 4, "tabela de alocação")?;
        let raw = read_vec(&file, bat_off, n * 4)?;
        let bat = raw.chunks_exact(4).map(|c| be32(c, 0)).collect();
        let bitmap_size = (block_size / SECTOR / 8).div_ceil(SECTOR) * SECTOR;
        let parent = if disk_type == 4 {
            let dir = path.parent().unwrap_or(Path::new("."));
            let mut names = vec![];
            for i in 0..8 {
                let o = 576 + i * 24;
                let code = &dh[o..o + 4];
                let len = be32(&dh, o + 8) as usize;
                let off = be64(&dh, o + 16);
                if matches!(code, b"W2ru" | b"W2ku" | b"Wi2r" | b"Wi2k") && len > 0 && len < 4096 {
                    if let Ok(b) = read_vec(&file, off, len) {
                        names.push(utf16(&b, false));
                    }
                }
            }
            names.push(utf16(&dh[64..64 + 512], true));
            Some(open_parent(&candidate_paths(dir, &names), depth, false)?)
        } else {
            None
        };
        Ok(Self {
            file,
            size: current,
            kind: Kind::Dynamic {
                bat,
                block_size,
                bitmap_size,
            },
            parent,
        })
    }
}

impl Backend for Vhd {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_chunk(&self, voff: u64, buf: &mut [u8]) -> io::Result<usize> {
        let Kind::Dynamic {
            bat,
            block_size,
            bitmap_size,
        } = &self.kind
        else {
            let n = buf.len().min((self.size - voff) as usize);
            read_exact(&self.file, voff, &mut buf[..n])?;
            return Ok(n);
        };
        let block = (voff / block_size) as usize;
        let within = voff % block_size;
        let mut n = buf.len().min((block_size - within) as usize);
        let entry = bat.get(block).copied().unwrap_or(NONE);
        if entry == NONE {
            read_parent(&self.parent, voff, &mut buf[..n])?;
            return Ok(n);
        }
        let base = entry as u64 * SECTOR;
        let first = within / SECTOR;
        let last = (within + n as u64 - 1) / SECTOR;
        let nbytes = (last / 8 - first / 8 + 1) as usize;
        let bm = read_vec(&self.file, base + first / 8, nbytes)?;
        let bit = |s: u64| bm[(s / 8 - first / 8) as usize] & (0x80 >> (s % 8)) != 0;
        let present = bit(first);
        let mut s = first;
        while s <= last && bit(s) == present {
            s += 1;
        }
        n = n.min((s * SECTOR - within) as usize);
        if present {
            read_exact(&self.file, base + bitmap_size + within, &mut buf[..n])?;
        } else {
            read_parent(&self.parent, voff, &mut buf[..n])?;
        }
        Ok(n)
    }
}
