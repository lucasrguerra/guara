//! FAT12/16/32: entradas de diretório apagadas (inclusive nomes longos) e
//! reconstrução dos clusters pela regra "próximos clusters livres".

use super::{make_found, merge_extents, sanitize_name, utf16le, Ctx, NewFound, VolumeScan};
use crate::source::{read_vec, Source};
use crate::types::{Confidence, DataLoc, Extent, FsKind, Origin};
use std::cell::RefCell;
use std::collections::HashSet;

struct Fat<'a> {
    src: &'a dyn Source,
    kind: FsKind,
    bps: u64,
    spc: u64,
    fat_off: u64,
    root_off: u64,
    root_len: u64,
    root_cluster: u32,
    data_off: u64,
    clusters: u32,
    cache: RefCell<(u64, Vec<u8>)>,
}

const PAGE: u64 = 256 * 1024;

impl<'a> Fat<'a> {
    fn open(src: &'a dyn Source, base: u64, kind: FsKind) -> Option<Self> {
        let b = read_vec(src, base, 512).ok()?;
        if b.len() < 512 {
            return None;
        }
        let bps = u16::from_le_bytes([b[11], b[12]]) as u64;
        let spc = b[13] as u64;
        let reserved = u16::from_le_bytes([b[14], b[15]]) as u64;
        let nfats = b[16] as u64;
        let root_entries = u16::from_le_bytes([b[17], b[18]]) as u64;
        let ts16 = u16::from_le_bytes([b[19], b[20]]) as u64;
        let fat16 = u16::from_le_bytes([b[22], b[23]]) as u64;
        let ts32 = u32::from_le_bytes(b[32..36].try_into().unwrap()) as u64;
        let fat32 = u32::from_le_bytes(b[36..40].try_into().unwrap()) as u64;
        let root_cluster = u32::from_le_bytes(b[44..48].try_into().unwrap());
        let total = if ts16 != 0 { ts16 } else { ts32 };
        let fatsz = if fat16 != 0 { fat16 } else { fat32 };
        if bps == 0 || spc == 0 || fatsz == 0 {
            return None;
        }
        let root_len = root_entries * 32;
        let root_secs = root_len.div_ceil(bps);
        let data_sec = reserved + nfats * fatsz + root_secs;
        let clusters = (total.saturating_sub(data_sec) / spc) as u32;
        Some(Self {
            src,
            kind,
            bps,
            spc,
            fat_off: base + reserved * bps,
            root_off: base + (reserved + nfats * fatsz) * bps,
            root_len,
            root_cluster,
            data_off: base + data_sec * bps,
            clusters,
            cache: RefCell::new((u64::MAX, Vec::new())),
        })
    }

    fn cluster_bytes(&self) -> u64 {
        self.bps * self.spc
    }

    fn cluster_off(&self, c: u32) -> u64 {
        self.data_off + (c as u64 - 2) * self.cluster_bytes()
    }

    fn valid(&self, c: u32) -> bool {
        c >= 2 && (c as u64) < self.clusters as u64 + 2
    }

    fn fat_bytes(&self, off: u64, n: usize) -> Option<Vec<u8>> {
        let page = off / PAGE;
        let mut cache = self.cache.borrow_mut();
        if cache.0 != page {
            let v = read_vec(self.src, self.fat_off + page * PAGE, PAGE as usize).ok()?;
            *cache = (page, v);
        }
        let s = (off - page * PAGE) as usize;
        if s + n <= cache.1.len() {
            return Some(cache.1[s..s + n].to_vec());
        }
        drop(cache);
        read_vec(self.src, self.fat_off + off, n)
            .ok()
            .filter(|v| v.len() == n)
    }

    fn entry(&self, c: u32) -> Option<u32> {
        match self.kind {
            FsKind::Fat32 => {
                let v = self.fat_bytes(c as u64 * 4, 4)?;
                Some(u32::from_le_bytes(v.try_into().unwrap()) & 0x0FFF_FFFF)
            }
            FsKind::Fat16 => {
                let v = self.fat_bytes(c as u64 * 2, 2)?;
                Some(u16::from_le_bytes(v.try_into().unwrap()) as u32)
            }
            _ => {
                let v = self.fat_bytes(c as u64 * 3 / 2, 2)?;
                let raw = u16::from_le_bytes(v.try_into().unwrap());
                Some(if c % 2 == 0 { raw & 0x0FFF } else { raw >> 4 } as u32)
            }
        }
    }

    fn is_eoc(&self, v: u32) -> bool {
        match self.kind {
            FsKind::Fat32 => v >= 0x0FFF_FFF8,
            FsKind::Fat16 => v >= 0xFFF8,
            _ => v >= 0xFF8,
        }
    }

    /// Cadeia de clusters viva a partir de `start` (com limite contra ciclos).
    fn chain(&self, start: u32) -> Vec<u32> {
        let mut out = vec![];
        let mut c = start;
        let mut seen = 0u64;
        while self.valid(c) && seen <= self.clusters as u64 {
            out.push(c);
            seen += 1;
            match self.entry(c) {
                Some(n) if !self.is_eoc(n) && n != 0 => c = n,
                _ => break,
            }
        }
        out
    }

    fn clusters_to_extents(&self, cl: &[u32]) -> Vec<Extent> {
        let cb = self.cluster_bytes();
        merge_extents(
            cl.iter()
                .map(|&c| Extent::new(self.cluster_off(c), cb))
                .collect(),
        )
    }

    fn read_clusters(&self, cl: &[u32], limit: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for &c in cl {
            if let Ok(v) = read_vec(self.src, self.cluster_off(c), self.cluster_bytes() as usize) {
                out.extend_from_slice(&v);
            }
            if out.len() >= limit {
                break;
            }
        }
        out
    }
}

struct Walk<'f, 'a, 'c, 'x> {
    fat: &'f Fat<'a>,
    ctx: &'c mut Ctx<'x>,
    live: Vec<Extent>,
    visited: HashSet<u32>,
    count: u64,
}

pub fn scan(ctx: &mut Ctx, base: u64, kind: FsKind) -> VolumeScan {
    let mut res = VolumeScan::default();
    let Some(fat) = Fat::open(ctx.src, base, kind) else {
        res.warnings
            .push("FAT: setor de boot ilegível; usando apenas assinatura.".into());
        return res;
    };
    // Estruturas do sistema (reservado + FATs + raiz fixa) são "vivas".
    let mut live = vec![Extent::new(base, fat.data_off - base)];
    let mut w = Walk {
        fat: &fat,
        ctx,
        live: vec![],
        visited: HashSet::new(),
        count: 0,
    };
    if kind == FsKind::Fat32 {
        let root = fat.chain(fat.root_cluster);
        w.live.extend(fat.clusters_to_extents(&root));
        let data = fat.read_clusters(&root, 64 << 20);
        w.dir(&data, "", false, 0);
    } else {
        let data = read_vec(fat.src, fat.root_off, fat.root_len as usize).unwrap_or_default();
        w.dir(&data, "", false, 0);
    }
    live.extend(w.live);
    res.live = merge_extents(live);
    res
}

impl Walk<'_, '_, '_, '_> {
    fn dir(&mut self, data: &[u8], path: &str, in_deleted: bool, depth: u32) {
        if depth > 48 || (self.ctx.cancelled)() {
            return;
        }
        let mut lfn: Vec<(u8, [u16; 13])> = vec![];
        for e in data.chunks_exact(32) {
            if e[0] == 0x00 {
                break;
            }
            let attr = e[11];
            if attr & 0x3F == 0x0F {
                let mut ch = [0u16; 13];
                let idx = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
                for (k, &o) in idx.iter().enumerate() {
                    ch[k] = u16::from_le_bytes([e[o], e[o + 1]]);
                }
                lfn.push((e[13], ch));
                continue;
            }
            let parts = std::mem::take(&mut lfn);
            if attr & 0x08 != 0 || e[0] == b'.' {
                continue;
            }
            let deleted = e[0] == 0xE5;
            let is_dir = attr & 0x10 != 0;
            let mut short = [0u8; 11];
            short.copy_from_slice(&e[0..11]);
            let name =
                long_name(&parts, &short).unwrap_or_else(|| short_name(&short, e[12], deleted));
            let hi = u16::from_le_bytes([e[20], e[21]]) as u32;
            let lo = u16::from_le_bytes([e[26], e[27]]) as u32;
            let start = if self.fat.kind == FsKind::Fat32 {
                (hi << 16) | lo
            } else {
                lo
            };
            let size = u32::from_le_bytes(e[28..32].try_into().unwrap()) as u64;
            let mtime = super::dos_to_unix(
                u16::from_le_bytes([e[24], e[25]]),
                u16::from_le_bytes([e[22], e[23]]),
            );
            let ctime = super::dos_to_unix(
                u16::from_le_bytes([e[16], e[17]]),
                u16::from_le_bytes([e[14], e[15]]),
            );
            let atime = super::dos_to_unix(u16::from_le_bytes([e[18], e[19]]), 0);
            self.count += 1;
            if self.count % 512 == 0 {
                (self.ctx.progress)(self.count, 0);
            }
            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };

            if is_dir {
                if !self.fat.valid(start) || !self.visited.insert(start) {
                    continue;
                }
                if deleted || in_deleted {
                    // Diretório apagado: lê clusters contíguos livres enquanto parecerem entradas.
                    let cl = self.contiguous_free(start, 16);
                    let data = self
                        .fat
                        .read_clusters(&cl, 16 * self.fat.cluster_bytes() as usize);
                    self.dir(&data, &child_path, true, depth + 1);
                } else {
                    let cl = self.fat.chain(start);
                    self.live.extend(self.fat.clusters_to_extents(&cl));
                    let data = self.fat.read_clusters(&cl, 64 << 20);
                    self.dir(&data, &child_path, false, depth + 1);
                }
                continue;
            }

            if !deleted && !in_deleted {
                if self.fat.valid(start) && size > 0 {
                    let cl = self.fat.chain(start);
                    self.live.extend(self.fat.clusters_to_extents(&cl));
                }
                continue;
            }
            if size == 0 || !self.fat.valid(start) {
                continue;
            }
            self.deleted_file(&name, path, start, size, (ctime, mtime, atime));
        }
    }

    fn contiguous_free(&self, start: u32, max: usize) -> Vec<u32> {
        let mut v = vec![];
        let mut c = start;
        while v.len() < max && self.fat.valid(c) && (v.is_empty() || self.fat.entry(c) == Some(0)) {
            v.push(c);
            c += 1;
        }
        v
    }

    fn deleted_file(
        &mut self,
        name: &str,
        dir: &str,
        start: u32,
        size: u64,
        times: (Option<i64>, Option<i64>, Option<i64>),
    ) {
        let fat = self.fat;
        let cb = fat.cluster_bytes();
        let need = size.div_ceil(cb);
        if need > fat.clusters as u64 {
            return;
        }
        let start_reused = fat.entry(start).map(|v| v != 0).unwrap_or(true);
        let mut cl = Vec::with_capacity(need as usize);
        let mut c = start;
        let mut skipped = false;
        while (cl.len() as u64) < need && fat.valid(c) {
            let free = fat.entry(c) == Some(0);
            if free || (cl.is_empty() && start_reused) {
                cl.push(c);
            } else {
                skipped = true;
            }
            c += 1;
        }
        if (cl.len() as u64) < need {
            return;
        }
        let (conf, note) = if start_reused {
            (
                Confidence::Low,
                Some("cluster inicial já reutilizado por outro arquivo"),
            )
        } else if skipped {
            (
                Confidence::Medium,
                Some("arquivo possivelmente fragmentado: ordem dos clusters presumida"),
            )
        } else {
            (Confidence::High, None)
        };
        let extents = fat.clusters_to_extents(&cl);
        let nf = NewFound {
            name: sanitize_name(name),
            dir: (!dir.is_empty()).then(|| dir.to_string()),
            size,
            origin: Origin::Fat,
            data: DataLoc::Extents {
                extents: trim_extents(extents, size),
            },
            modified: times.1,
            created: times.0,
            accessed: times.2,
            base: conf,
            note,
        };
        let f = make_found(self.ctx, nf);
        (self.ctx.emit)(f);
    }
}

/// Corta a lista de extents para exatamente `size` bytes.
pub fn trim_extents(mut ex: Vec<Extent>, size: u64) -> Vec<Extent> {
    let mut left = size;
    let mut out = vec![];
    for e in ex.drain(..) {
        if left == 0 {
            break;
        }
        let take = e.len.min(left);
        out.push(Extent {
            offset: e.offset,
            len: take,
            sparse: e.sparse,
        });
        left -= take;
    }
    out
}

fn short_name(raw: &[u8; 11], case: u8, deleted: bool) -> String {
    let mut base: Vec<u8> = raw[0..8].to_vec();
    if deleted {
        base[0] = b'_';
    } else if base[0] == 0x05 {
        base[0] = 0xE5;
    }
    let ext = &raw[8..11];
    let f = |b: &[u8], lower: bool| -> String {
        let s: String = b
            .iter()
            .map(|&c| c as char)
            .collect::<String>()
            .trim_end()
            .to_string();
        if lower {
            s.to_lowercase()
        } else {
            s
        }
    };
    let b = f(&base, case & 0x08 != 0);
    let e = f(ext, case & 0x10 != 0);
    if e.is_empty() {
        b
    } else {
        format!("{b}.{e}")
    }
}

fn long_name(parts: &[(u8, [u16; 13])], short: &[u8; 11]) -> Option<String> {
    if parts.is_empty() {
        return None;
    }
    let mut sum = 0u8;
    for &b in short {
        sum = (sum >> 1).wrapping_add((sum & 1) << 7).wrapping_add(b);
    }
    // Entradas LFN de um arquivo apagado preservam o checksum do nome curto original,
    // porém o primeiro byte do nome curto virou 0xE5; tentamos as duas variantes.
    let ok = parts.iter().all(|p| p.0 == parts[0].0);
    if !ok {
        return None;
    }
    let target = parts[0].0;
    if sum != target {
        // Nome curto apagado perdeu o 1º caractere: aceita se o resto bater por tentativa.
        let mut hit = false;
        for c in 0u8..=255 {
            let mut s2 = *short;
            s2[0] = c;
            let mut s = 0u8;
            for &b in &s2 {
                s = (s >> 1).wrapping_add((s & 1) << 7).wrapping_add(b);
            }
            if s == target {
                hit = true;
                break;
            }
        }
        if !hit {
            return None;
        }
    }
    let mut u: Vec<u16> = vec![];
    for (_, ch) in parts.iter().rev() {
        u.extend_from_slice(ch);
    }
    let end = u
        .iter()
        .position(|&c| c == 0 || c == 0xFFFF)
        .unwrap_or(u.len());
    let raw: Vec<u8> = u[..end].iter().flat_map(|c| c.to_le_bytes()).collect();
    let s = utf16le(&raw);
    (!s.is_empty()).then_some(s)
}
