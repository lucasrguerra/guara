//! VHDX (Hyper-V): tabela de regiões, metadados, BAT com blocos de bitmap intercalados,
//! discos fixos, dinâmicos e diferenciais (pai achado pelo localizador de pai).
//!
//! O log de transações não é reproduzido: abrir a imagem nunca a escreve. Se o disco foi
//! desligado no meio de uma escrita, os dados ainda no log não aparecem.

use super::{
    bad, candidate_paths, check_table, le16, le32, le64, open_parent, read_exact, read_parent,
    read_vec, utf16, Backend,
};
use crate::source::{FileSource, Source};
use std::io;
use std::path::Path;

const MB: u64 = 1 << 20;

/// GUID em texto para os 16 bytes gravados em disco (três primeiros campos little-endian).
fn guid(s: &str) -> [u8; 16] {
    let hex: Vec<u8> = s
        .replace('-', "")
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    let mut g = [0u8; 16];
    g[..4].copy_from_slice(&[hex[3], hex[2], hex[1], hex[0]]);
    g[4..6].copy_from_slice(&[hex[5], hex[4]]);
    g[6..8].copy_from_slice(&[hex[7], hex[6]]);
    g[8..].copy_from_slice(&hex[8..]);
    g
}

pub(super) struct Vhdx {
    file: FileSource,
    size: u64,
    block_size: u64,
    sector_size: u64,
    chunk_ratio: u64,
    bat: Vec<u64>,
    parent: Option<Box<dyn Source>>,
}

impl Vhdx {
    pub(super) fn open(file: FileSource, path: &Path, depth: u32) -> io::Result<Self> {
        let rt = read_vec(&file, 192 * 1024, 64 * 1024)?;
        if &rt[..4] != b"regi" {
            return Err(bad("tabela de regiões VHDX inválida"));
        }
        let count = le32(&rt, 8).min(2047) as usize;
        let (mut bat_at, mut meta_at) = (None, None);
        for i in 0..count {
            let e = &rt[16 + i * 32..16 + (i + 1) * 32];
            let region = (le64(e, 16), le32(e, 24) as u64);
            if e[..16] == guid("2DC27766-F623-4200-9D64-115E9BFD4A08") {
                bat_at = Some(region);
            } else if e[..16] == guid("8B7CA206-4790-4B9A-B8FE-575F050F886E") {
                meta_at = Some(region);
            }
        }
        let (Some((bat_off, bat_len)), Some((meta_off, meta_len))) = (bat_at, meta_at) else {
            return Err(bad("regiões BAT e metadados não encontradas"));
        };
        let meta = read_vec(&file, meta_off, meta_len.min(MB) as usize)?;
        if &meta[..8] != b"metadata" {
            return Err(bad("região de metadados VHDX inválida"));
        }
        let items = le16(&meta, 10) as usize;
        let find = |id: [u8; 16]| -> Option<&[u8]> {
            (0..items).find_map(|i| {
                let e = meta.get(32 + i * 32..32 + (i + 1) * 32)?;
                if e[..16] != id {
                    return None;
                }
                let (o, l) = (le32(e, 16) as usize, le32(e, 20) as usize);
                meta.get(o..o + l)
            })
        };
        let params = find(guid("CAA16737-FA36-4D43-B3B6-33F0AA44E76B"))
            .filter(|p| p.len() >= 8)
            .ok_or_else(|| bad("parâmetros do arquivo VHDX ausentes"))?;
        let block_size = le32(params, 0) as u64;
        let has_parent = le32(params, 4) & 2 != 0;
        let size = find(guid("2FA54224-CD1B-4876-B211-5DBED83BF4B8"))
            .filter(|p| p.len() >= 8)
            .map(|p| le64(p, 0))
            .ok_or_else(|| bad("tamanho do disco VHDX ausente"))?;
        let sector_size = find(guid("8141BF1D-A96F-4709-BA47-F233A8FAAB5F"))
            .filter(|p| p.len() >= 4)
            .map(|p| le32(p, 0) as u64)
            .unwrap_or(512);
        if block_size < MB || !block_size.is_power_of_two() || !matches!(sector_size, 512 | 4096) {
            return Err(bad("parâmetros VHDX inválidos"));
        }
        let chunk_ratio = (1u64 << 23) * sector_size / block_size;
        let payload = size.div_ceil(block_size);
        // Com pai, cada trecho termina com a entrada do bitmap de setores, mesmo que o disco acabe antes.
        let want = if has_parent {
            payload.div_ceil(chunk_ratio) * (chunk_ratio + 1)
        } else {
            payload + payload.div_ceil(chunk_ratio)
        };
        let n = check_table(want.min(bat_len / 8), 8, "BAT")?;
        let raw = read_vec(&file, bat_off, n * 8)?;
        let bat = raw.chunks_exact(8).map(|c| le64(c, 0)).collect();

        let parent = if has_parent {
            let loc = find(guid("A8D35F2D-B30B-454D-ABF7-D3D84834AB0C"))
                .ok_or_else(|| bad("localizador de pai VHDX ausente"))?;
            let count = if loc.len() >= 20 {
                le16(loc, 18) as usize
            } else {
                0
            };
            let mut names = vec![];
            for i in 0..count {
                let Some(e) = loc.get(20 + i * 12..20 + (i + 1) * 12) else {
                    break;
                };
                let (ko, vo) = (le32(e, 0) as usize, le32(e, 4) as usize);
                let (ks, vs) = (le16(e, 8) as usize, le16(e, 10) as usize);
                let (Some(k), Some(v)) = (loc.get(ko..ko + ks), loc.get(vo..vo + vs)) else {
                    continue;
                };
                let key = utf16(k, false);
                if matches!(
                    key.as_str(),
                    "relative_path" | "absolute_win32_path" | "volume_path"
                ) {
                    names.push(utf16(v, false));
                }
            }
            let dir = path.parent().unwrap_or(Path::new("."));
            Some(open_parent(&candidate_paths(dir, &names), depth, false)?)
        } else {
            None
        };
        Ok(Self {
            file,
            size,
            block_size,
            sector_size,
            chunk_ratio,
            bat,
            parent,
        })
    }

    fn bat(&self, i: u64) -> u64 {
        self.bat.get(i as usize).copied().unwrap_or(0)
    }
}

impl Backend for Vhdx {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_chunk(&self, voff: u64, buf: &mut [u8]) -> io::Result<usize> {
        let block = voff / self.block_size;
        let within = voff % self.block_size;
        let mut n = buf.len().min((self.block_size - within) as usize);
        let e = self.bat(block + block / self.chunk_ratio);
        let off = e & 0xFFFF_FFFF_FFF0_0000;
        match e & 7 {
            6 => {
                read_exact(&self.file, off + within, &mut buf[..n])?;
            }
            7 => {
                // Parcialmente presente: o bitmap do trecho diz, setor a setor, onde está o dado.
                let chunk = block / self.chunk_ratio;
                let bm_entry = self.bat(chunk * (self.chunk_ratio + 1) + self.chunk_ratio);
                let ss = self.sector_size;
                let per_block = self.block_size / ss;
                let first = within / ss;
                let last = (within + n as u64 - 1) / ss;
                let base_bit = (block % self.chunk_ratio) * per_block;
                let bm_off = bm_entry & 0xFFFF_FFFF_FFF0_0000;
                if bm_entry & 7 != 6 {
                    read_parent(&self.parent, voff, &mut buf[..n])?;
                    return Ok(n);
                }
                let b0 = (base_bit + first) / 8;
                let b1 = (base_bit + last) / 8;
                let bm = read_vec(&self.file, bm_off + b0, (b1 - b0 + 1) as usize)?;
                let bit = |s: u64| {
                    let p = base_bit + s;
                    bm[(p / 8 - b0) as usize] >> (p % 8) & 1 == 1
                };
                let present = bit(first);
                let mut s = first;
                while s <= last && bit(s) == present {
                    s += 1;
                }
                n = n.min((s * ss - within) as usize);
                if present {
                    read_exact(&self.file, off + within, &mut buf[..n])?;
                } else {
                    read_parent(&self.parent, voff, &mut buf[..n])?;
                }
            }
            2 | 3 => buf[..n].fill(0),
            _ => read_parent(&self.parent, voff, &mut buf[..n])?,
        }
        Ok(n)
    }
}
