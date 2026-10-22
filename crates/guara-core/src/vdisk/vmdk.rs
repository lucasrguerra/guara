//! VMDK (VMware, VirtualBox): extensões esparsas (inclusive `streamOptimized` com grãos
//! comprimidos) e descritores de texto com extensões `FLAT`, `SPARSE` e `ZERO`, como
//! `monolithicFlat` e `twoGbMaxExtentSparse`. Discos com pai (snapshots) e o formato SEsparse
//! são recusados.

use super::{bad, check_table, le16, le32, le64, read_exact, read_vec, Backend};
use crate::source::{FileSource, Source};
use std::io::{self, Read};
use std::path::Path;
use std::sync::Mutex;

const SECTOR: u64 = 512;
const GD_AT_END: u64 = u64::MAX;

struct Sparse {
    file: FileSource,
    grain: u64,
    gtes: u64,
    gd: Vec<u32>,
    compressed: bool,
    cache: Mutex<Option<(u64, Vec<u8>)>>,
}

enum Kind {
    Zero,
    Flat { file: FileSource, offset: u64 },
    Sparse(Sparse),
}

struct Extent {
    start: u64,
    len: u64,
    kind: Kind,
}

pub(super) struct Vmdk {
    extents: Vec<Extent>,
    size: u64,
}

fn parent_in(descriptor: &str) -> bool {
    descriptor.lines().any(|l| {
        let l = l.trim();
        l.strip_prefix("parentCID")
            .and_then(|r| r.trim_start().strip_prefix('='))
            .is_some_and(|v| {
                let v = v.trim().trim_matches('"');
                !v.is_empty() && !v.eq_ignore_ascii_case("ffffffff")
            })
    })
}

impl Sparse {
    fn open(file: FileSource) -> io::Result<(Self, u64, Option<String>)> {
        let mut h = read_vec(&file, 0, SECTOR as usize)?;
        if &h[..4] == b"SESp" || &h[..4] == b"seSp" {
            return Err(bad("VMDK no formato SEsparse (vSphere) não é suportado"));
        }
        if &h[..4] != b"KDMV" {
            return Err(bad("extensão VMDK sem assinatura KDMV"));
        }
        if le64(&h, 56) == GD_AT_END {
            let size = file.size();
            if size < 3 * SECTOR {
                return Err(bad("VMDK truncado"));
            }
            h = read_vec(&file, size - 2 * SECTOR, SECTOR as usize)?;
            if &h[..4] != b"KDMV" {
                return Err(bad("rodapé do VMDK streamOptimized ausente"));
            }
        }
        let flags = le32(&h, 8);
        let capacity = le64(&h, 12);
        let grain = le64(&h, 20);
        let desc_off = le64(&h, 28);
        let desc_len = le64(&h, 36);
        let gtes = le32(&h, 44) as u64;
        let gd_off = le64(&h, 56);
        let algo = le16(&h, 77);
        if grain == 0 || !grain.is_power_of_two() || gtes == 0 {
            return Err(bad("cabeçalho VMDK inválido"));
        }
        let compressed = flags & (1 << 16) != 0;
        if compressed && algo != 1 {
            return Err(bad("compressão de grão VMDK desconhecida"));
        }
        let descriptor = if desc_off != 0 && desc_len != 0 {
            let raw = read_vec(
                &file,
                desc_off * SECTOR,
                (desc_len * SECTOR).min(1 << 20) as usize,
            )?;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            Some(String::from_utf8_lossy(&raw[..end]).into_owned())
        } else {
            None
        };
        if descriptor.as_deref().is_some_and(parent_in) {
            return Err(bad(
                "VMDK com disco pai (snapshot); abrir só a camada filha daria dados errados",
            ));
        }
        let gde = capacity.div_ceil(gtes * grain);
        let n = check_table(gde, 4, "diretório de grãos")?;
        let raw = read_vec(&file, gd_off * SECTOR, n * 4)?;
        let gd = raw.chunks_exact(4).map(|c| le32(c, 0)).collect();
        Ok((
            Self {
                file,
                grain: grain * SECTOR,
                gtes,
                gd,
                compressed,
                cache: Mutex::new(None),
            },
            capacity * SECTOR,
            descriptor,
        ))
    }

    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let g = off / self.grain;
        let within = off % self.grain;
        let n = buf.len().min((self.grain - within) as usize);
        let gt_sector = self.gd.get((g / self.gtes) as usize).copied().unwrap_or(0) as u64;
        let mut sector = 0u64;
        if gt_sector > 1 {
            let mut b = [0u8; 4];
            read_exact(&self.file, gt_sector * SECTOR + (g % self.gtes) * 4, &mut b)?;
            sector = u32::from_le_bytes(b) as u64;
        }
        if sector <= 1 {
            buf[..n].fill(0);
            return Ok(n);
        }
        if !self.compressed {
            read_exact(&self.file, sector * SECTOR + within, &mut buf[..n])?;
            return Ok(n);
        }
        let mut cache = self.cache.lock().unwrap();
        if cache.as_ref().map(|c| c.0) != Some(g) {
            let mut m = [0u8; 12];
            read_exact(&self.file, sector * SECTOR, &mut m)?;
            let csize = le32(&m, 8) as usize;
            if csize == 0 || csize > self.grain as usize * 2 {
                return Err(bad("grão VMDK comprimido com tamanho inválido"));
            }
            let raw = read_vec(&self.file, sector * SECTOR + 12, csize)?;
            let mut out = Vec::with_capacity(self.grain as usize);
            flate2::read::ZlibDecoder::new(&raw[..]).read_to_end(&mut out)?;
            out.resize(self.grain as usize, 0);
            *cache = Some((g, out));
        }
        let data = &cache.as_ref().unwrap().1;
        buf[..n].copy_from_slice(&data[within as usize..within as usize + n]);
        Ok(n)
    }
}

/// Extensões de um descritor de texto: `RW 2048 FLAT "disco-f001.vmdk" 0`.
fn parse_descriptor(text: &str, dir: &Path) -> io::Result<Vec<Extent>> {
    if parent_in(text) {
        return Err(bad(
            "VMDK com disco pai (snapshot); abrir só a camada filha daria dados errados",
        ));
    }
    let mut out = vec![];
    let mut start = 0u64;
    for line in text.lines() {
        let l = line.trim();
        let mut it = l.split_whitespace();
        let (Some(access), Some(sectors), Some(kind)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if !matches!(access, "RW" | "RDONLY" | "NOACCESS") {
            continue;
        }
        let Ok(sectors) = sectors.parse::<u64>() else {
            continue;
        };
        let len = sectors * SECTOR;
        let name = match (l.find('"'), l.rfind('"')) {
            (Some(a), Some(b)) if b > a => Some(&l[a + 1..b]),
            _ => None,
        };
        let kind = match kind {
            "ZERO" => Kind::Zero,
            "FLAT" | "VMFS" => {
                let name = name.ok_or_else(|| bad("extensão FLAT sem nome de arquivo"))?;
                let offset = l
                    .rsplit_once('"')
                    .and_then(|(_, r)| r.trim().parse::<u64>().ok())
                    .unwrap_or(0);
                Kind::Flat {
                    file: FileSource::open(&dir.join(name))?,
                    offset: offset * SECTOR,
                }
            }
            "SPARSE" => {
                let name = name.ok_or_else(|| bad("extensão SPARSE sem nome de arquivo"))?;
                let (sp, cap, _) = Sparse::open(FileSource::open(&dir.join(name))?)?;
                if cap != len {
                    return Err(bad("tamanho da extensão VMDK difere do descritor"));
                }
                Kind::Sparse(sp)
            }
            other => return Err(bad(format!("tipo de extensão VMDK {other} não suportado"))),
        };
        out.push(Extent { start, len, kind });
        start += len;
    }
    if out.is_empty() {
        return Err(bad("descritor VMDK sem extensões"));
    }
    Ok(out)
}

impl Vmdk {
    pub(super) fn open(file: FileSource, path: &Path) -> io::Result<Self> {
        let mut head = [0u8; 4];
        let _ = file.read_at(0, &mut head);
        let extents = if &head == b"KDMV" {
            let (sp, size, _) = Sparse::open(file)?;
            vec![Extent {
                start: 0,
                len: size,
                kind: Kind::Sparse(sp),
            }]
        } else {
            if file.size() > 1 << 20 {
                return Err(bad("descritor VMDK grande demais"));
            }
            let raw = read_vec(&file, 0, file.size() as usize)?;
            let text = String::from_utf8_lossy(&raw);
            let dir = path.parent().unwrap_or(Path::new("."));
            parse_descriptor(&text, dir)?
        };
        let size = extents.iter().map(|e| e.len).sum();
        Ok(Self { extents, size })
    }
}

impl Backend for Vmdk {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_chunk(&self, voff: u64, buf: &mut [u8]) -> io::Result<usize> {
        let i = self.extents.partition_point(|e| e.start + e.len <= voff);
        let Some(e) = self.extents.get(i) else {
            return Ok(0);
        };
        let rel = voff - e.start;
        let n = buf.len().min((e.len - rel) as usize);
        match &e.kind {
            Kind::Zero => {
                buf[..n].fill(0);
                Ok(n)
            }
            Kind::Flat { file, offset } => {
                read_exact(file, offset + rel, &mut buf[..n])?;
                Ok(n)
            }
            Kind::Sparse(s) => s.read(rel, &mut buf[..n]),
        }
    }
}
