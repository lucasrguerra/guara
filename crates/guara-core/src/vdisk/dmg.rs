//! DMG (Apple UDIF): rodapé `koly`, tabelas `mish` do plist e chunks zerados, brutos, ADC,
//! zlib e bzip2. Imagens cifradas (`encrcdsa`) e chunks LZFSE (`ULFO`) são recusados com
//! mensagem clara.

use super::{bad, be32, be64, read_exact, read_vec, Backend};
use crate::source::{FileSource, Source};
use std::io::{self, Read};
use std::sync::Mutex;

const SECTOR: u64 = 512;

pub(super) fn is_dmg(file: &FileSource) -> bool {
    let size = file.size();
    if size < 1024 {
        return false;
    }
    let mut t = [0u8; 512];
    read_exact(file, size - SECTOR, &mut t).is_ok()
        && &t[..4] == b"koly"
        && be32(&t, 4) == 4
        && be32(&t, 8) == 512
}

/// Imagem cifrada pelo `hdiutil`: começa com `encrcdsa` ou `cdsaencr`.
pub(super) fn is_encrypted(head: &[u8]) -> bool {
    head.starts_with(b"encrcdsa") || head.starts_with(b"cdsaencr")
}

#[derive(Clone, Copy)]
struct Chunk {
    start: u64,
    len: u64,
    ty: u32,
    off: u64,
    comp_len: u64,
}

pub(super) struct Dmg {
    file: FileSource,
    size: u64,
    chunks: Vec<Chunk>,
    cache: Mutex<Option<(usize, Vec<u8>)>>,
}

fn base64(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// Descompacta ADC (Apple Data Compression).
fn adc(src: &[u8], max: usize) -> io::Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(max);
    let mut i = 0;
    while i < src.len() && out.len() < max {
        let c = src[i];
        i += 1;
        if c & 0x80 != 0 {
            let n = (c & 0x7F) as usize + 1;
            let lit = src.get(i..i + n).ok_or_else(|| bad("chunk ADC truncado"))?;
            out.extend_from_slice(lit);
            i += n;
        } else {
            let (len, off) = if c & 0x40 != 0 {
                let b = src.get(i..i + 2).ok_or_else(|| bad("chunk ADC truncado"))?;
                i += 2;
                (
                    (c & 0x3F) as usize + 4,
                    (b[0] as usize) << 8 | b[1] as usize,
                )
            } else {
                let b = *src.get(i).ok_or_else(|| bad("chunk ADC truncado"))?;
                i += 1;
                ((c >> 2) as usize + 3, ((c & 3) as usize) << 8 | b as usize)
            };
            let off = off + 1;
            if off > out.len() {
                return Err(bad("referência ADC inválida"));
            }
            for _ in 0..len {
                out.push(out[out.len() - off]);
            }
        }
    }
    Ok(out)
}

impl Dmg {
    pub(super) fn open(file: FileSource) -> io::Result<Self> {
        let mut head = [0u8; 8];
        let _ = file.read_at(0, &mut head);
        if is_encrypted(&head) {
            return Err(bad(
                "DMG cifrado (encrcdsa) não é suportado; desproteja a imagem antes",
            ));
        }
        let size = file.size();
        let t = read_vec(&file, size - SECTOR, SECTOR as usize)?;
        let fork = be64(&t, 24);
        let xml_off = be64(&t, 216);
        let xml_len = be64(&t, 224);
        let sectors = be64(&t, 492);
        if xml_len == 0 || xml_len > 256 << 20 {
            return Err(bad("plist do DMG ausente ou grande demais"));
        }
        let xml = read_vec(&file, xml_off, xml_len as usize)?;
        let xml = String::from_utf8_lossy(&xml);
        let from = xml
            .find("<key>blkx</key>")
            .ok_or_else(|| bad("tabela blkx não encontrada no DMG"))?;
        let rest = &xml[from..];
        let rest = &rest[..rest.find("</array>").unwrap_or(rest.len())];
        let mut chunks: Vec<Chunk> = vec![];
        let mut cur = rest;
        while let Some(a) = cur.find("<data>") {
            let after = &cur[a + 6..];
            let Some(b) = after.find("</data>") else {
                break;
            };
            let mish = base64(&after[..b]);
            cur = &after[b + 7..];
            if mish.len() < 204 || &mish[..4] != b"mish" {
                continue;
            }
            let first = be64(&mish, 8);
            let data_off = be64(&mish, 24);
            let n = be32(&mish, 200) as usize;
            for i in 0..n {
                let Some(c) = mish.get(204 + i * 40..204 + (i + 1) * 40) else {
                    break;
                };
                let ty = be32(c, 0);
                if ty == 0x7FFF_FFFE || ty == 0xFFFF_FFFF {
                    continue;
                }
                chunks.push(Chunk {
                    start: (first + be64(c, 8)) * SECTOR,
                    len: be64(c, 16) * SECTOR,
                    ty,
                    off: fork + data_off + be64(c, 24),
                    comp_len: be64(c, 32),
                });
            }
        }
        if chunks.is_empty() {
            return Err(bad("DMG sem chunks"));
        }
        chunks.sort_by_key(|c| c.start);
        let end = chunks.iter().map(|c| c.start + c.len).max().unwrap_or(0);
        Ok(Self {
            file,
            size: end.max(sectors * SECTOR),
            chunks,
            cache: Mutex::new(None),
        })
    }

    fn decode(&self, c: &Chunk) -> io::Result<Vec<u8>> {
        if c.len > 64 << 20 || c.comp_len > 64 << 20 {
            return Err(bad("chunk de DMG grande demais"));
        }
        let raw = read_vec(&self.file, c.off, c.comp_len as usize)?;
        let mut out = Vec::with_capacity(c.len as usize);
        match c.ty {
            0x8000_0004 => out = adc(&raw, c.len as usize)?,
            0x8000_0005 => {
                flate2::read::ZlibDecoder::new(&raw[..]).read_to_end(&mut out)?;
            }
            0x8000_0006 => {
                bzip2::read::BzDecoder::new(&raw[..]).read_to_end(&mut out)?;
            }
            0x8000_0007 => {
                return Err(bad("chunk LZFSE (formato ULFO) ainda não é suportado"));
            }
            t => return Err(bad(format!("tipo de chunk DMG 0x{t:08x} desconhecido"))),
        }
        out.resize(c.len as usize, 0);
        Ok(out)
    }
}

impl Backend for Dmg {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_chunk(&self, voff: u64, buf: &mut [u8]) -> io::Result<usize> {
        let i = self.chunks.partition_point(|c| c.start + c.len <= voff);
        let Some(c) = self.chunks.get(i).copied() else {
            let n = buf.len().min((self.size - voff) as usize);
            buf[..n].fill(0);
            return Ok(n);
        };
        if voff < c.start {
            // espaço que nenhuma tabela cobre
            let n = buf.len().min((c.start - voff) as usize);
            buf[..n].fill(0);
            return Ok(n);
        }
        let rel = voff - c.start;
        let n = buf.len().min((c.len - rel) as usize);
        match c.ty {
            0 | 2 => buf[..n].fill(0),
            1 => read_exact(&self.file, c.off + rel, &mut buf[..n])?,
            _ => {
                let mut cache = self.cache.lock().unwrap();
                if cache.as_ref().map(|x| x.0) != Some(i) {
                    *cache = Some((i, self.decode(&c)?));
                }
                let data = &cache.as_ref().unwrap().1;
                buf[..n].copy_from_slice(&data[rel as usize..rel as usize + n]);
            }
        }
        Ok(n)
    }
}
