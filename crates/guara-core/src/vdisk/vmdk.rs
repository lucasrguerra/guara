//! VMDK (VMware, VirtualBox): extensões esparsas (inclusive `streamOptimized` com grãos
//! comprimidos) e descritores de texto com extensões `FLAT`, `SPARSE` e `ZERO`, como
//! `monolithicFlat` e `twoGbMaxExtentSparse`. Discos com pai (snapshots) leem o pai nos grãos
//! que o filho não alocou; o formato SEsparse é recusado.

use super::{
    bad, candidate_paths, check_table, le16, le32, le64, open_parent, read_exact, read_parent,
    read_vec, Backend,
};
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
    parent: Option<Box<dyn Source>>,
}

/// Valor de uma chave `chave=valor` do descritor, sem aspas.
fn desc_value(descriptor: &str, key: &str) -> Option<String> {
    descriptor.lines().find_map(|l| {
        l.trim()
            .strip_prefix(key)
            .and_then(|r| r.trim_start().strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
    })
}

/// Descritor (texto) de um arquivo VMDK: o próprio arquivo, se for descritor, ou o embutido
/// logo depois do cabeçalho de uma extensão esparsa.
fn descriptor_of(path: &Path) -> Option<String> {
    let f = FileSource::open(path).ok()?;
    let head = read_vec(&f, 0, (f.size() as usize).min(24 * 512)).ok()?;
    if head.starts_with(b"KDMV") {
        let off = (le64(&head, 28) * SECTOR) as usize;
        let len = (le64(&head, 36) * SECTOR) as usize;
        let d = head.get(off..(off + len).min(head.len()))?;
        let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
        return Some(String::from_utf8_lossy(&d[..end]).into_owned());
    }
    head.starts_with(b"# Disk Desc")
        .then(|| String::from_utf8_lossy(&head).into_owned())
}

/// Como o descritor identifica o pai: pelo nome (`parentFileNameHint`) e/ou, no VirtualBox, só
/// pelo UUID (`ddb.uuid.parent`); `cid` é o `parentCID` que o pai precisa ter.
struct ParentRef {
    name: Option<String>,
    uuid: Option<String>,
    cid: Option<String>,
}

fn parent_ref(descriptor: &str) -> Option<ParentRef> {
    let cid = desc_value(descriptor, "parentCID")
        .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("ffffffff"));
    let uuid = desc_value(descriptor, "ddb.uuid.parent")
        .filter(|u| !u.is_empty() && u.chars().any(|c| c != '0' && c != '-'));
    let name = desc_value(descriptor, "parentFileNameHint").filter(|n| !n.is_empty());
    (cid.is_some() || uuid.is_some()).then_some(ParentRef { name, uuid, cid })
}

/// Procura, na pasta do filho e nas duas acima, um VMDK cujo `ddb.uuid.image` seja `uuid`.
fn find_by_uuid(dir: &Path, uuid: &str) -> Option<std::path::PathBuf> {
    let mut d = Some(dir);
    for _ in 0..3 {
        let cur = d?;
        for e in std::fs::read_dir(cur).ok()?.flatten() {
            let p = e.path();
            if p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("vmdk"))
                && descriptor_of(&p)
                    .and_then(|ds| desc_value(&ds, "ddb.uuid.image"))
                    .is_some_and(|u| u.eq_ignore_ascii_case(uuid))
            {
                return Some(p);
            }
        }
        d = cur.parent();
    }
    None
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

    /// Lê dentro de um grão. O `bool` é `false` quando o grão não está alocado neste arquivo
    /// (o chamador completa com o pai); grão zerado de propósito devolve zeros.
    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<(usize, bool)> {
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
        if sector == 0 {
            return Ok((n, false));
        }
        if sector == 1 {
            buf[..n].fill(0);
            return Ok((n, true));
        }
        if !self.compressed {
            read_exact(&self.file, sector * SECTOR + within, &mut buf[..n])?;
            return Ok((n, true));
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
        Ok((n, true))
    }
}

/// Extensões de um descritor de texto: `RW 2048 FLAT "disco-f001.vmdk" 0`.
fn parse_descriptor(text: &str, dir: &Path) -> io::Result<Vec<Extent>> {
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
    fn open_parent_of(r: &ParentRef, dir: &Path, depth: u32) -> io::Result<Box<dyn Source>> {
        let mut found = r
            .name
            .as_ref()
            .map(|n| candidate_paths(dir, std::slice::from_ref(n)))
            .unwrap_or_default();
        let by_name = found.iter().any(|p| p.is_file());
        if !by_name {
            if let Some(p) = r.uuid.as_deref().and_then(|u| find_by_uuid(dir, u)) {
                found = vec![p];
            }
        }
        if r.name.is_none() && r.uuid.is_none() {
            return Err(bad(
                "VMDK com pai, mas o descritor não diz qual (sem parentFileNameHint nem ddb.uuid.parent)",
            ));
        }
        if r.name.is_none() && found.is_empty() {
            return Err(bad(
                "VMDK com pai identificado só por UUID (VirtualBox) e o pai não está na pasta do filho nem nas duas acima",
            ));
        }
        if let Some(cid) = &r.cid {
            // o pai precisa ser o mesmo de quando o filho foi criado
            if let Some(p) = found.iter().find(|p| p.is_file()) {
                let have = descriptor_of(p).and_then(|d| desc_value(&d, "CID"));
                if have
                    .as_deref()
                    .is_some_and(|h| !h.eq_ignore_ascii_case(cid))
                {
                    return Err(bad(format!(
                        "o pai {} foi alterado depois do filho (CID {} em vez de {cid}); os dados do filho não valem mais",
                        p.display(),
                        have.unwrap_or_default()
                    )));
                }
            }
        }
        open_parent(&found, depth, false)
    }

    pub(super) fn open(file: FileSource, path: &Path, depth: u32) -> io::Result<Self> {
        let mut head = [0u8; 4];
        let _ = file.read_at(0, &mut head);
        let dir = path.parent().unwrap_or(Path::new("."));
        let (extents, descriptor) = if &head == b"KDMV" {
            let (sp, size, descriptor) = Sparse::open(file)?;
            (
                vec![Extent {
                    start: 0,
                    len: size,
                    kind: Kind::Sparse(sp),
                }],
                descriptor,
            )
        } else {
            if file.size() > 1 << 20 {
                return Err(bad("descritor VMDK grande demais"));
            }
            let raw = read_vec(&file, 0, file.size() as usize)?;
            let text = String::from_utf8_lossy(&raw).into_owned();
            (parse_descriptor(&text, dir)?, Some(text))
        };
        let parent = match descriptor.as_deref().and_then(parent_ref) {
            Some(r) => Some(Self::open_parent_of(&r, dir, depth)?),
            None => None,
        };
        let size = extents.iter().map(|e| e.len).sum();
        Ok(Self {
            extents,
            size,
            parent,
        })
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
            Kind::Sparse(s) => {
                let (got, allocated) = s.read(rel, &mut buf[..n])?;
                if !allocated {
                    read_parent(&self.parent, voff, &mut buf[..got])?;
                }
                Ok(got)
            }
        }
    }
}
