//! XFS (v4 e v5): mapa de espaço livre pela árvore de blocos livres de cada grupo de alocação e
//! arquivos apagados. Ao liberar um inode, o XFS zera o modo e o tamanho mas não apaga a lista de
//! extents que ficou dentro do inode; e a entrada de diretório liberada perde só os 4 primeiros
//! bytes (o número do inode), preservando o nome. Juntamos as duas sobras.

use super::{make_found, merge_extents, Ctx, NewFound, VolumeScan};
use crate::signatures;
use crate::source::{read_vec, Source};
use crate::types::{Confidence, DataLoc, Extent, Origin};
use std::collections::{HashMap, HashSet};

fn be16(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .unwrap_or(0)
}
fn be32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4)
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}
fn be64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8)
        .map(|s| u64::from_be_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}

struct Xfs<'a> {
    src: &'a dyn Source,
    base: u64,
    bs: u64,
    agblocks: u64,
    agcount: u64,
    inodesize: u64,
    inopblog: u32,
    agblklog: u32,
    sectsize: u64,
    v5: bool,
    ftype: bool,
    root_ino: u64,
    dblocks: u64,
}

impl<'a> Xfs<'a> {
    fn open(src: &'a dyn Source, base: u64) -> Option<Xfs<'a>> {
        let sb = read_vec(src, base, 512).ok()?;
        if sb.len() < 512 || &sb[0..4] != b"XFSB" {
            return None;
        }
        let ver = be16(&sb, 100) & 0xF;
        let bs = be32(&sb, 4) as u64;
        let agblocks = be32(&sb, 84) as u64;
        let agcount = be32(&sb, 88) as u64;
        let inodesize = be16(&sb, 104) as u64;
        if !(512..=65536).contains(&bs)
            || !bs.is_power_of_two()
            || agblocks == 0
            || agcount == 0
            || !(256..=2048).contains(&inodesize)
        {
            return None;
        }
        let incompat = be32(&sb, 216);
        let features2 = be32(&sb, 200);
        let v5 = ver == 5;
        Some(Xfs {
            src,
            base,
            bs,
            agblocks,
            agcount,
            inodesize,
            inopblog: sb[123] as u32,
            agblklog: sb[124] as u32,
            sectsize: be16(&sb, 102) as u64,
            v5,
            ftype: if v5 {
                incompat & 1 != 0
            } else {
                features2 & 0x200 != 0
            },
            root_ino: be64(&sb, 56),
            dblocks: be64(&sb, 8),
        })
    }

    fn ag_off(&self, agno: u64) -> u64 {
        self.base + agno * self.agblocks * self.bs
    }

    /// Endereço físico de um bloco "fsblock" (agno << agblklog | agbno).
    fn fsb_off(&self, fsb: u64) -> Option<u64> {
        let (agno, agbno) = (fsb >> self.agblklog, fsb & ((1 << self.agblklog) - 1));
        (agno < self.agcount && agbno < self.agblocks).then(|| self.ag_off(agno) + agbno * self.bs)
    }

    fn ino_off(&self, ino: u64) -> Option<u64> {
        let shift = self.agblklog + self.inopblog;
        let agno = ino >> shift;
        let agbno = (ino >> self.inopblog) & ((1 << self.agblklog) - 1);
        let idx = ino & ((1 << self.inopblog) - 1);
        (agno < self.agcount && agbno < self.agblocks)
            .then(|| self.ag_off(agno) + agbno * self.bs + idx * self.inodesize)
    }

    fn read_ino(&self, ino: u64) -> Option<Vec<u8>> {
        let b = read_vec(self.src, self.ino_off(ino)?, self.inodesize as usize).ok()?;
        (b.len() as u64 == self.inodesize && &b[0..2] == b"IN").then_some(b)
    }

    fn core_size(&self, ino: &[u8]) -> usize {
        if ino[4] >= 3 {
            176
        } else {
            100
        }
    }

    /// Extents de um inode com a lista no próprio inode (formato 2) ou em árvore (formato 3).
    fn extents_of(&self, ino: &[u8], max_blocks: u64) -> Vec<(u64, u64, u64)> {
        let fmt = ino[5];
        let start = self.core_size(ino);
        let fork_end = if ino[82] != 0 {
            start.max(100 + ino[82] as usize * 8)
        } else {
            self.inodesize as usize
        };
        let lit = &ino[start..fork_end.min(ino.len())];
        let mut out = vec![];
        match fmt {
            2 => {
                let n = (be32(ino, 76) as usize).min(lit.len() / 16);
                for i in 0..n {
                    out.push(unpack_extent(&lit[i * 16..i * 16 + 16]));
                }
            }
            3 => {
                // raiz da árvore de extents no inode: nível, nregistros, chaves e ponteiros
                let level = be16(lit, 0) as usize;
                let nrecs = be16(lit, 2) as usize;
                let maxrecs = (lit.len() - 4) / 16;
                let ptrs = 4 + maxrecs * 8;
                for i in 0..nrecs.min(maxrecs) {
                    let fsb = be64(lit, ptrs + i * 8);
                    self.walk_bmbt(fsb, level - 1, &mut out, 0);
                }
            }
            _ => {}
        }
        out.retain(|e| e.2 > 0 && e.2 <= max_blocks);
        out
    }

    fn walk_bmbt(&self, fsb: u64, level: usize, out: &mut Vec<(u64, u64, u64)>, depth: u32) {
        if depth > 6 || out.len() > 1_000_000 {
            return;
        }
        let Some(off) = self.fsb_off(fsb) else { return };
        let Ok(b) = read_vec(self.src, off, self.bs as usize) else {
            return;
        };
        if b.len() < 24 || !(&b[0..4] == b"BMAP" || &b[0..4] == b"BMA3") {
            return;
        }
        let hdr = if self.v5 { 72 } else { 24 };
        let numrecs = be16(&b, 6) as usize;
        if level == 0 {
            for i in 0..numrecs {
                if let Some(r) = b.get(hdr + i * 16..hdr + i * 16 + 16) {
                    out.push(unpack_extent(r));
                }
            }
        } else {
            let maxrecs = (self.bs as usize - hdr) / 16;
            let ptrs = hdr + maxrecs * 8;
            for i in 0..numrecs {
                self.walk_bmbt(be64(&b, ptrs + i * 8), level - 1, out, depth + 1);
            }
        }
    }
}

/// Registro de extent empacotado em 128 bits: flag(1) | início no arquivo(54) | bloco(52) | quantidade(21).
fn unpack_extent(r: &[u8]) -> (u64, u64, u64) {
    let hi = u64::from_be_bytes(r[0..8].try_into().unwrap());
    let lo = u64::from_be_bytes(r[8..16].try_into().unwrap());
    let startoff = (hi >> 9) & ((1 << 54) - 1);
    let startblock = ((hi & 0x1FF) << 43) | (lo >> 21);
    let count = lo & 0x1F_FFFF;
    (startoff, startblock, count)
}

/// Percorre as folhas de uma árvore B+ curta, entregando cada registro de 16 (inobt) ou 8 (bnobt) bytes.
fn walk_short_btree(
    x: &Xfs,
    agno: u64,
    root: u64,
    levels: u32,
    rec: usize,
    key_size: usize,
    f: &mut dyn FnMut(&[u8]),
) {
    let mut cur = root;
    // desce pela esquerda até a folha
    for lvl in (1..levels).rev() {
        let Ok(b) = read_vec(x.src, x.ag_off(agno) + cur * x.bs, x.bs as usize) else {
            return;
        };
        if b.len() < 24 {
            return;
        }
        let hdr = if x.v5 { 56 } else { 16 };
        let maxrecs = (x.bs as usize - hdr) / (key_size + 4);
        let ptrs = hdr + maxrecs * key_size;
        let _ = lvl;
        cur = be32(&b, ptrs) as u64;
    }
    let mut seen = HashSet::new();
    while cur != 0 && cur != u32::MAX as u64 && seen.insert(cur) {
        let Ok(b) = read_vec(x.src, x.ag_off(agno) + cur * x.bs, x.bs as usize) else {
            return;
        };
        if b.len() < 24 {
            return;
        }
        let hdr = if x.v5 { 56 } else { 16 };
        let n = be16(&b, 6) as usize;
        for i in 0..n {
            if let Some(r) = b.get(hdr + i * rec..hdr + (i + 1) * rec) {
                f(r);
            }
        }
        cur = be32(&b, 12) as u64; // rightsib
    }
}

pub fn scan(ctx: &mut Ctx, base: u64, part_len: u64) -> VolumeScan {
    let _ = part_len;
    let mut res = VolumeScan::default();
    let Some(x) = Xfs::open(ctx.src, base) else {
        return res;
    };

    // Espaço livre por grupo e inodes livres.
    let mut free: Vec<Extent> = vec![];
    let mut free_set: Vec<(u64, u64)> = vec![]; // (fsblock inicial, quantidade) para checar blocos
    let mut chunks: Vec<(u64, u64, u64)> = vec![]; // (agno, startino, máscara livre)
    for agno in 0..x.agcount {
        if (ctx.cancelled)() {
            return res;
        }
        let agf = read_vec(ctx.src, x.ag_off(agno) + x.sectsize, 64).unwrap_or_default();
        if agf.len() >= 64 && &agf[0..4] == b"XAGF" {
            let (root, levels) = (be32(&agf, 16) as u64, be32(&agf, 28));
            walk_short_btree(&x, agno, root, levels, 8, 8, &mut |r| {
                let (s, c) = (be32(r, 0) as u64, be32(r, 4) as u64);
                free.push(Extent::new(x.ag_off(agno) + s * x.bs, c * x.bs));
                free_set.push(((agno << x.agblklog) | s, c));
            });
        }
        let agi = read_vec(ctx.src, x.ag_off(agno) + 2 * x.sectsize, 64).unwrap_or_default();
        if agi.len() >= 64 && &agi[0..4] == b"XAGI" {
            let (root, levels) = (be32(&agi, 20) as u64, be32(&agi, 24));
            walk_short_btree(&x, agno, root, levels, 16, 4, &mut |r| {
                chunks.push((agno, be32(r, 0) as u64, be64(r, 8)));
            });
        }
        (ctx.progress)(agno, x.agcount);
    }
    free.sort_by_key(|e| e.offset);
    let free = merge_extents(free);
    res.free = Some(free.clone());
    free_set.sort();
    let is_free = |fsb: u64, count: u64| -> bool {
        let i = free_set.partition_point(|&(s, c)| s + c <= fsb);
        free_set
            .get(i)
            .is_some_and(|&(s, c)| s <= fsb && s + c >= fsb + count)
    };

    // Nomes: entradas de diretório liberadas (guardam o nome e os 4 bytes baixos do inode).
    let (live_dirs, stale_names) = collect_names(&x, ctx);

    // Inodes livres com a lista de extents ainda dentro do inode.
    let max_blocks = x.dblocks;
    let mut seen_first: HashSet<u64> = HashSet::new();
    for (agno, startino, mask) in &chunks {
        if (ctx.cancelled)() {
            return res;
        }
        for i in 0..64u64 {
            if mask >> i & 1 == 0 {
                continue; // em uso
            }
            let ino = (agno << (x.agblklog + x.inopblog)) | (startino + i);
            let Some(raw) = x.read_ino(ino) else { continue };
            let mode = be16(&raw, 2);
            if mode != 0 && be32(&raw, 16) != 0 {
                continue;
            }
            // Só inodes que foram arquivos: o formato de extents continua gravado e há sobras.
            if raw[5] != 2 {
                continue;
            }
            let start = x.core_size(&raw);
            let lit = &raw[start..];
            // lê registros plausíveis em sequência
            let mut ext: Vec<(u64, u64, u64)> = vec![];
            let mut next_off = 0u64;
            for k in 0..lit.len() / 16 {
                let e = unpack_extent(&lit[k * 16..k * 16 + 16]);
                let valid = e.2 > 0
                    && e.2 < 1 << 21
                    && e.0 == next_off
                    && x.fsb_off(e.1).is_some()
                    && (e.1 & ((1 << x.agblklog) - 1)) + e.2 <= x.agblocks;
                if !valid {
                    break;
                }
                next_off = e.0 + e.2;
                ext.push(e);
            }
            if ext.is_empty() {
                continue;
            }
            let total_blocks: u64 = ext.iter().map(|e| e.2).sum();
            if total_blocks > max_blocks {
                continue;
            }
            let all_free = ext.iter().all(|e| is_free(e.1, e.2));
            let extents: Vec<Extent> = ext
                .iter()
                .filter_map(|e| x.fsb_off(e.1).map(|o| Extent::new(o, e.2 * x.bs)))
                .collect();
            if extents.is_empty() || !seen_first.insert(extents[0].offset) {
                continue;
            }
            let mut size = total_blocks * x.bs;
            let mut conf = if all_free {
                Confidence::High
            } else {
                Confidence::Low
            };
            let mut note = if all_free {
                "inode XFS liberado; lista de extents recuperada do próprio inode".to_string()
            } else {
                "blocos do arquivo já reaproveitados".to_string()
            };
            // tamanho exato: tenta achar o fim real pelo conteúdo
            let mut est = true;
            if let Some(h) = read_vec(ctx.src, extents[0].offset, 128)
                .ok()
                .and_then(|head| signatures::detect(ctx.src, extents[0].offset, &head, size))
            {
                if h.len > 0 && h.len <= size {
                    size = h.len;
                    est = false;
                }
            }
            if est {
                conf = conf.min(Confidence::Medium);
                note.push_str("; tamanho exato perdido: usando todos os blocos");
            }
            let extents = super::fat::trim_extents(extents, size);
            let low = ino & 0xFFFF_FFFF;
            let (dir, name) = match stale_names.get(&low) {
                Some((parent, n)) => (
                    Some(live_dirs.get(parent).cloned().unwrap_or_default()),
                    n.clone(),
                ),
                None => (None, format!("inode_{ino}")),
            };
            let ts = |s: u32| (s != 0).then_some(s as i64);
            let nf = NewFound {
                name,
                dir,
                size,
                origin: Origin::Xfs,
                data: DataLoc::Extents { extents },
                modified: ts(be32(&raw, 40)),
                created: if raw[4] >= 3 {
                    ts(be32(&raw, 144))
                } else {
                    None
                },
                accessed: ts(be32(&raw, 32)),
                base: conf,
                note: Some(&note),
            };
            let f = make_found(ctx, nf);
            (ctx.emit)(f);
        }
    }
    res
}

/// Percorre os diretórios vivos; devolve (caminho de cada diretório por inode, nome liberado por inode baixo).
fn collect_names(x: &Xfs, ctx: &Ctx) -> (HashMap<u64, String>, HashMap<u64, (u64, String)>) {
    let mut paths: HashMap<u64, String> = HashMap::new();
    let mut stale: HashMap<u64, (u64, String)> = HashMap::new();
    let mut queue: Vec<(u64, String)> = vec![(x.root_ino, String::new())];
    let mut seen: HashSet<u64> = HashSet::new();
    while let Some((dino, path)) = queue.pop() {
        if !seen.insert(dino) || (ctx.cancelled)() || seen.len() > 200_000 {
            continue;
        }
        let Some(raw) = x.read_ino(dino) else {
            continue;
        };
        if be16(&raw, 2) & 0o170000 != 0o040000 {
            continue;
        }
        paths.insert(dino, path.clone());
        let start = x.core_size(&raw);
        match raw[5] {
            1 => {
                // pasta curta guardada no inode
                let lit = &raw[start..];
                let count = lit[0] as usize;
                let i8 = lit[1] != 0;
                let isz = if i8 { 8 } else { 4 };
                let mut p = 2 + isz;
                for _ in 0..count.max(lit[1] as usize) {
                    if p + 3 > lit.len() {
                        break;
                    }
                    let nl = lit[p] as usize;
                    p += 3;
                    let name =
                        String::from_utf8_lossy(lit.get(p..p + nl).unwrap_or(&[])).to_string();
                    p += nl + if x.ftype { 1 } else { 0 };
                    let ino = if i8 {
                        be64(lit, p)
                    } else {
                        be32(lit, p) as u64
                    };
                    p += isz;
                    queue.push((ino, format!("{path}/{name}")));
                }
            }
            2 | 3 => {
                let size = be64(&raw, 56).min(64 << 20);
                let ex = x.extents_of(&raw, 1 << 20);
                let _ = size;
                for (off, fsb, cnt) in ex {
                    // blocos de dados de diretório ficam antes do offset de 32 GiB (depois vêm folhas e índice)
                    if off * x.bs >= 32u64 << 30 {
                        continue;
                    }
                    for k in 0..cnt.min(256) {
                        let Some(o) = x.fsb_off(fsb + k) else {
                            continue;
                        };
                        let Ok(b) = read_vec(x.src, o, x.bs as usize) else {
                            continue;
                        };
                        parse_dir_block(x, &b, dino, &path, &mut queue, &mut stale);
                    }
                }
            }
            _ => {}
        }
    }
    (paths, stale)
}

fn parse_dir_block(
    x: &Xfs,
    b: &[u8],
    dino: u64,
    path: &str,
    queue: &mut Vec<(u64, String)>,
    stale: &mut HashMap<u64, (u64, String)>,
) {
    let magic = &b[0..4];
    let hdr = match magic {
        b"XD2B" | b"XD2D" => 16,
        b"XDB3" | b"XDD3" => 64,
        _ => return,
    };
    let is_block = magic == b"XD2B" || magic == b"XDB3";
    // em blocos únicos, o final tem a tabela de folhas
    let end = if is_block {
        let cnt = be32(b, b.len() - 8) as usize;
        b.len().saturating_sub(8 + cnt * 8)
    } else {
        b.len()
    };
    let mut p = hdr;
    while p + 8 <= end {
        if be16(b, p) == 0xFFFF {
            let len = be16(b, p + 2) as usize;
            if len < 8 || p + len > end {
                break;
            }
            // entrada liberada: sobraram (inode baixo, nome); outras ainda podem estar dentro da região
            scan_free_region(x, &b[p..p + len], dino, stale);
            p += len;
            continue;
        }
        let ino = be64(b, p);
        let nl = b[p + 8] as usize;
        if nl == 0 {
            break;
        }
        let ft = if x.ftype { 1 } else { 0 };
        let size = (8 + 1 + nl + ft + 2).div_ceil(8) * 8;
        if p + size > end {
            break;
        }
        let name = String::from_utf8_lossy(&b[p + 9..p + 9 + nl]).to_string();
        if name != "." && name != ".." {
            queue.push((ino, format!("{path}/{name}")));
        }
        p += size;
    }
}

fn scan_free_region(x: &Xfs, r: &[u8], dir_ino: u64, stale: &mut HashMap<u64, (u64, String)>) {
    // 1) a primeira entrada perdeu só os 4 primeiros bytes do número do inode
    let try_at = |p: usize, low: u64, stale: &mut HashMap<u64, (u64, String)>| -> Option<usize> {
        let nl = *r.get(p + 8)? as usize;
        let name = r.get(p + 9..p + 9 + nl)?;
        if nl == 0 || name.iter().any(|&c| c < 0x20 || c == b'/' || c == 0x7F) {
            return None;
        }
        stale
            .entry(low)
            .or_insert((dir_ino, String::from_utf8_lossy(name).to_string()));
        Some((8 + 1 + nl + if x.ftype { 1 } else { 0 } + 2).div_ceil(8) * 8)
    };
    if r.len() >= 16 {
        let low = be32(r, 4) as u64;
        if let Some(sz) = try_at(0, low, stale) {
            // entradas seguintes dentro da mesma região, com o inode inteiro
            let mut p = sz;
            while p + 16 <= r.len() {
                let ino = be64(r, p);
                if ino == 0 || ino > 1 << 56 {
                    p += 8;
                    continue;
                }
                match try_at(p, ino & 0xFFFF_FFFF, stale) {
                    Some(s) => p += s,
                    None => p += 8,
                }
            }
        }
    }
}
