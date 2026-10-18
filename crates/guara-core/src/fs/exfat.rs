//! exFAT: conjuntos de entradas apagados (0x05/0x40/0x41) e bitmap de alocação.

use super::{
    make_found, merge_extents, overlaps, sanitize_name, utf16le, Ctx, NewFound, VolumeScan,
};
use crate::source::{read_vec, Source};
use crate::types::{Confidence, DataLoc, Extent, Origin};
use std::collections::HashSet;

struct Vol<'a> {
    src: &'a dyn Source,
    base: u64,
    fat_off: u64,
    heap_off: u64,
    cbytes: u64,
    clusters: u32,
    root: u32,
    bitmap: Vec<u8>,
}

impl Vol<'_> {
    fn coff(&self, c: u32) -> u64 {
        self.heap_off + (c as u64 - 2) * self.cbytes
    }
    fn valid(&self, c: u32) -> bool {
        c >= 2 && (c as u64) < self.clusters as u64 + 2
    }
    fn allocated(&self, c: u32) -> bool {
        let i = (c - 2) as usize;
        self.bitmap
            .get(i / 8)
            .map(|b| b >> (i % 8) & 1 == 1)
            .unwrap_or(false)
    }
    fn fat(&self, c: u32) -> Option<u32> {
        let v = read_vec(self.src, self.fat_off + c as u64 * 4, 4).ok()?;
        (v.len() == 4).then(|| u32::from_le_bytes(v.try_into().unwrap()))
    }
    fn chain(&self, start: u32) -> Vec<u32> {
        let mut out = vec![];
        let mut c = start;
        while self.valid(c) && (out.len() as u64) <= self.clusters as u64 {
            out.push(c);
            match self.fat(c) {
                Some(n) if self.valid(n) => c = n,
                _ => break,
            }
        }
        out
    }
    fn read(&self, cl: &[u32], limit: usize) -> Vec<u8> {
        let mut out = vec![];
        for &c in cl {
            if let Ok(v) = read_vec(self.src, self.coff(c), self.cbytes as usize) {
                out.extend_from_slice(&v);
            }
            if out.len() >= limit {
                break;
            }
        }
        out
    }
    fn extents(&self, cl: &[u32]) -> Vec<Extent> {
        merge_extents(
            cl.iter()
                .map(|&c| Extent::new(self.coff(c), self.cbytes))
                .collect(),
        )
    }
}

pub fn scan(ctx: &mut Ctx, base: u64) -> VolumeScan {
    let mut res = VolumeScan::default();
    let Some(b) = read_vec(ctx.src, base, 512).ok().filter(|b| b.len() == 512) else {
        res.warnings.push("exFAT: setor de boot ilegível.".into());
        return res;
    };
    let ss = 1u64 << b[108];
    let cbytes = ss << b[109];
    let fat_off = base + u32::from_le_bytes(b[80..84].try_into().unwrap()) as u64 * ss;
    let heap_off = base + u32::from_le_bytes(b[88..92].try_into().unwrap()) as u64 * ss;
    let clusters = u32::from_le_bytes(b[92..96].try_into().unwrap());
    let root = u32::from_le_bytes(b[96..100].try_into().unwrap());
    let mut vol = Vol {
        src: ctx.src,
        base,
        fat_off,
        heap_off,
        cbytes,
        clusters,
        root,
        bitmap: vec![],
    };

    // Diretório raiz: procura a entrada do bitmap (0x81).
    let root_chain = vol.chain(vol.root);
    let root_data = vol.read(&root_chain, 64 << 20);
    let mut bm: Option<(u32, u64)> = None;
    for e in root_data.chunks_exact(32) {
        if e[0] == 0x81 {
            bm = Some((
                u32::from_le_bytes(e[20..24].try_into().unwrap()),
                u64::from_le_bytes(e[24..32].try_into().unwrap()),
            ));
            break;
        }
    }
    if let Some((c, len)) = bm {
        if vol.valid(c) {
            vol.bitmap =
                read_vec(ctx.src, vol.coff(c), len.min(256 << 20) as usize).unwrap_or_default();
        }
    } else {
        res.warnings
            .push("exFAT: bitmap de alocação não encontrado; confiança será menor.".into());
    }

    // Extents vivos: metadados (antes do heap) + clusters alocados no bitmap.
    let mut live = vec![Extent::new(base, heap_off - base)];
    let mut run_start: Option<u32> = None;
    for c in 2..clusters + 2 {
        let a = vol.allocated(c);
        match (a, run_start) {
            (true, None) => run_start = Some(c),
            (false, Some(s)) => {
                live.push(Extent::new(vol.coff(s), (c - s) as u64 * cbytes));
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = run_start {
        live.push(Extent::new(vol.coff(s), (clusters + 2 - s) as u64 * cbytes));
    }
    let live = merge_extents(live);

    let mut visited = HashSet::new();
    visited.insert(vol.root);
    walk(ctx, &vol, &root_data, "", false, 0, &live, &mut visited);
    res.live = live;
    res
}

#[allow(clippy::too_many_arguments)]
fn walk(
    ctx: &mut Ctx,
    vol: &Vol,
    data: &[u8],
    path: &str,
    in_deleted: bool,
    depth: u32,
    live: &[Extent],
    visited: &mut HashSet<u32>,
) {
    if depth > 48 || (ctx.cancelled)() {
        return;
    }
    let ents: Vec<&[u8]> = data.chunks_exact(32).collect();
    let mut i = 0;
    while i < ents.len() {
        let e = ents[i];
        if e[0] == 0x00 {
            break;
        }
        let ty = e[0];
        // 0x85 = arquivo/pasta vivo; 0x05 = apagado.
        if ty != 0x85 && ty != 0x05 {
            i += 1;
            continue;
        }
        let deleted = ty == 0x05;
        let secondary = e[1] as usize;
        let attr = u16::from_le_bytes([e[4], e[5]]);
        let mdate = u16::from_le_bytes([e[14], e[15]]);
        let mtime = u16::from_le_bytes([e[12], e[13]]);
        let mod_unix = super::dos_to_unix(mdate, mtime);
        let cre_unix = super::dos_to_unix(
            u16::from_le_bytes([e[10], e[11]]),
            u16::from_le_bytes([e[8], e[9]]),
        );
        let acc_unix = super::dos_to_unix(
            u16::from_le_bytes([e[18], e[19]]),
            u16::from_le_bytes([e[16], e[17]]),
        );
        let mut stream: Option<&[u8]> = None;
        let mut name_u16: Vec<u8> = vec![];
        for s in 1..=secondary.min(19) {
            let Some(se) = ents.get(i + s) else { break };
            match se[0] & 0x7F {
                0x40 => stream = Some(se),
                0x41 => name_u16.extend_from_slice(&se[2..32]),
                _ => {}
            }
        }
        i += 1 + secondary;
        let Some(st) = stream else { continue };
        let nlen = st[3] as usize;
        let name = {
            let n = utf16le(&name_u16);
            n.chars().take(nlen).collect::<String>()
        };
        let flags = st[1];
        let first = u32::from_le_bytes(st[20..24].try_into().unwrap());
        let size = u64::from_le_bytes(st[24..32].try_into().unwrap());
        let no_chain = flags & 2 != 0;
        let is_dir = attr & 0x10 != 0;
        let child_path = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };
        if !vol.valid(first) {
            continue;
        }
        let need = size.div_ceil(vol.cbytes) as u32;
        let cl: Vec<u32> = if no_chain || deleted {
            // Apagados: a cadeia na FAT pode ter sido preservada; senão presume contíguo.
            let chain = if !no_chain { vol.chain(first) } else { vec![] };
            if chain.len() as u32 >= need.max(1) {
                chain
            } else {
                (first..first.saturating_add(need.max(1)))
                    .filter(|c| vol.valid(*c))
                    .collect()
            }
        } else {
            vol.chain(first)
        };
        if is_dir {
            if !visited.insert(first) {
                continue;
            }
            let d = vol.read(&cl, 64 << 20);
            walk(
                ctx,
                vol,
                &d,
                &child_path,
                deleted || in_deleted,
                depth + 1,
                live,
                visited,
            );
            continue;
        }
        if !(deleted || in_deleted) || size == 0 || (cl.len() as u32) < need {
            continue;
        }
        let ex = super::fat::trim_extents(vol.extents(&cl[..need as usize]), size);
        let reused =
            !vol.bitmap.is_empty() && cl[..need as usize].iter().any(|&c| vol.allocated(c));
        let (conf, note) = if reused {
            (
                Confidence::Low,
                Some("clusters já reutilizados por outros arquivos"),
            )
        } else {
            (Confidence::High, None)
        };
        let _ = overlaps(live, 0, 0);
        let f = make_found(
            ctx,
            NewFound {
                name: sanitize_name(&name),
                dir: (!path.is_empty()).then(|| path.to_string()),
                size,
                origin: Origin::Exfat,
                data: DataLoc::Extents { extents: ex },
                modified: mod_unix,
                created: cre_unix,
                accessed: acc_unix,
                base: conf,
                note,
            },
        );
        (ctx.emit)(f);
    }
    let _ = vol.base;
}
