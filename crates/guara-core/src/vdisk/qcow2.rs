//! QCOW2 (QEMU), versões 2 e 3: tabelas L1/L2, clusters comprimidos (deflate e zstd) e
//! clusters zerados. Imagens cifradas, com arquivo de apoio, com arquivo de dados externo ou
//! com L2 estendido são recusadas.

use super::{bad, be32, be64, check_table, read_exact, read_vec, Backend};
use crate::source::{FileSource, Source};
use std::io::{self, Read};
use std::sync::Mutex;

const OFFSET_MASK: u64 = 0x00ff_ffff_ffff_fe00;
const COMPRESSED: u64 = 1 << 62;

pub(super) struct Qcow2 {
    file: FileSource,
    size: u64,
    cluster_bits: u32,
    l1: Vec<u64>,
    zstd: bool,
    /// Último cluster comprimido decodificado: (deslocamento do cluster virtual, dados).
    cache: Mutex<Option<(u64, Vec<u8>)>>,
}

impl Qcow2 {
    pub(super) fn open(file: FileSource) -> io::Result<Self> {
        let h = read_vec(&file, 0, 112.min(file.size() as usize))?;
        if h.len() < 72 {
            return Err(bad("cabeçalho QCOW2 truncado"));
        }
        let version = be32(&h, 4);
        if version != 2 && version != 3 {
            return Err(bad(format!("versão QCOW {version} não suportada")));
        }
        if be64(&h, 8) != 0 {
            return Err(bad(
                "a imagem QCOW2 usa arquivo de apoio (backing file); abrir só a camada filha daria dados errados",
            ));
        }
        let cluster_bits = be32(&h, 20);
        if !(9..=21).contains(&cluster_bits) {
            return Err(bad(format!(
                "tamanho de cluster inválido (2^{cluster_bits})"
            )));
        }
        let size = be64(&h, 24);
        match be32(&h, 32) {
            0 => {}
            1 => return Err(bad("imagem QCOW2 cifrada (AES legado) não é suportada")),
            _ => return Err(bad("imagem QCOW2 cifrada (LUKS) não é suportada; abra o volume LUKS depois de convertê-la")),
        }
        let mut zstd = false;
        if version == 3 && h.len() >= 108 {
            let incompat = be64(&h, 72);
            if incompat & (1 << 2) != 0 {
                return Err(bad("QCOW2 com arquivo de dados externo não é suportado"));
            }
            if incompat & (1 << 4) != 0 {
                return Err(bad("QCOW2 com L2 estendido não é suportado"));
            }
            if incompat & (1 << 3) != 0 {
                zstd = h[104] == 1;
                if h[104] > 1 {
                    return Err(bad("tipo de compressão QCOW2 desconhecido"));
                }
            }
        }
        let l1_size = be32(&h, 36) as u64;
        let l1_off = be64(&h, 40);
        let n = check_table(l1_size, 8, "tabela L1")?;
        let raw = read_vec(&file, l1_off, n * 8)?;
        let l1 = raw.chunks_exact(8).map(|c| be64(c, 0)).collect();
        Ok(Self {
            file,
            size,
            cluster_bits,
            l1,
            zstd,
            cache: Mutex::new(None),
        })
    }

    fn cluster_size(&self) -> u64 {
        1 << self.cluster_bits
    }

    fn l2_entry(&self, cluster_idx: u64) -> io::Result<u64> {
        let per_l2 = self.cluster_size() / 8;
        let l1_idx = (cluster_idx / per_l2) as usize;
        let l2_idx = cluster_idx % per_l2;
        let Some(&l1e) = self.l1.get(l1_idx) else {
            return Ok(0);
        };
        let l2_off = l1e & OFFSET_MASK;
        if l2_off == 0 {
            return Ok(0);
        }
        let mut b = [0u8; 8];
        read_exact(&self.file, l2_off + l2_idx * 8, &mut b)?;
        Ok(u64::from_be_bytes(b))
    }

    fn inflate(&self, entry: u64) -> io::Result<Vec<u8>> {
        let shift = 62 - (self.cluster_bits - 8);
        let host = entry & ((1u64 << shift) - 1);
        let nsect = ((entry >> shift) & ((1u64 << (self.cluster_bits - 8)) - 1)) + 1;
        let csize = (nsect * 512 - (host & 511)) as usize;
        let avail = self.file.size().saturating_sub(host) as usize;
        let raw = read_vec(&self.file, host, csize.min(avail))?;
        let mut out = Vec::with_capacity(self.cluster_size() as usize);
        if self.zstd {
            let mut d = ruzstd::decoding::StreamingDecoder::new(&raw[..])
                .map_err(|e| bad(format!("cluster zstd inválido: {e}")))?;
            d.read_to_end(&mut out)?;
        } else {
            flate2::read::DeflateDecoder::new(&raw[..]).read_to_end(&mut out)?;
        }
        out.resize(self.cluster_size() as usize, 0);
        Ok(out)
    }
}

impl Backend for Qcow2 {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_chunk(&self, voff: u64, buf: &mut [u8]) -> io::Result<usize> {
        let cs = self.cluster_size();
        let idx = voff / cs;
        let within = voff % cs;
        let n = buf.len().min((cs - within) as usize);
        let entry = self.l2_entry(idx)?;
        if entry & COMPRESSED != 0 {
            let mut cache = self.cache.lock().unwrap();
            if cache.as_ref().map(|c| c.0) != Some(idx) {
                *cache = Some((idx, self.inflate(entry)?));
            }
            let data = &cache.as_ref().unwrap().1;
            buf[..n].copy_from_slice(&data[within as usize..within as usize + n]);
            return Ok(n);
        }
        let off = entry & OFFSET_MASK;
        if off == 0 || entry & 1 != 0 {
            buf[..n].fill(0);
        } else {
            read_exact(&self.file, off + within, &mut buf[..n])?;
        }
        Ok(n)
    }
}
