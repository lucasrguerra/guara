//! HFS+ / HFSX (inclusive volumes embutidos num wrapper HFS clássico).
//!
//! Lê o catálogo (árvore B+) para nomes e caminhos, o bitmap de alocação para o mapa de
//! espaço livre e procura arquivos apagados em três lugares: sobras dentro dos nós folha do
//! catálogo, cópias antigas de nós no diário (journal) e registros de arquivo "soltos".

use super::{make_found, merge_extents, Ctx, NewFound, VolumeScan};
use crate::source::{read_vec, Source};
use crate::types::{Confidence, DataLoc, Extent, Origin};
use std::collections::{HashMap, HashSet};

const HFS_EPOCH: i64 = 2_082_844_800;

fn be16(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]])).unwrap_or(0)
}
fn be32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|s| u32::from_be_bytes(s.try_into().unwrap())).unwrap_or(0)
}
fn be64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8).map(|s| u64::from_be_bytes(s.try_into().unwrap())).unwrap_or(0)
}

#[derive(Clone, Debug, Default)]
struct Fork {
    size: u64,
    blocks: u32,
    extents: Vec<(u32, u32)>,
}

fn parse_fork(b: &[u8]) -> Fork {
    let mut f = Fork { size: be64(b, 0), blocks: be32(b, 12), extents: vec![] };
    for i in 0..8 {
        let (s, c) = (be32(b, 16 + i * 8), be32(b, 20 + i * 8));
        if c != 0 {
            f.extents.push((s, c));
        }
    }
    f
}

/// Posição do volume HFS+ dentro da partição (wrapper HFS clássico) ou `None` se não for HFS+.
pub fn locate(src: &dyn Source, base: u64) -> Option<u64> {
    let h = read_vec(src, base + 1024, 512).ok()?;
    if h.len() < 512 {
        return None;
    }
    match &h[0..2] {
        b"H+" | b"HX" => Some(base),
        b"BD" if &h[124..126] == b"H+" => {
            let alblk = be32(&h, 20) as u64;
            let alblst = be16(&h, 28) as u64;
            let start = be16(&h, 126) as u64;
            Some(base + alblst * 512 + start * alblk)
        }
        _ => None,
    }
}

struct Vol<'a> {
    src: &'a dyn Source,
    base: u64,
    bs: u64,
    total: u64,
    catalog: Fork,
    extents_file: Fork,
    alloc: Fork,
    next_cnid: u32,
    journal_info: u32,
}

impl<'a> Vol<'a> {
    fn open(src: &'a dyn Source, base: u64) -> Option<Vol<'a>> {
        let h = read_vec(src, base + 1024, 512).ok()?;
        if h.len() < 512 || !(&h[0..2] == b"H+" || &h[0..2] == b"HX") {
            return None;
        }
        let bs = be32(&h, 40) as u64;
        if bs < 512 || !bs.is_power_of_two() {
            return None;
        }
        Some(Vol {
            src,
            base,
            bs,
            total: be32(&h, 44) as u64,
            alloc: parse_fork(&h[112..192]),
            extents_file: parse_fork(&h[192..272]),
            catalog: parse_fork(&h[272..352]),
            next_cnid: be32(&h, 64),
            journal_info: be32(&h, 12),
        })
    }

    fn block_off(&self, b: u64) -> u64 {
        self.base + b * self.bs
    }

    /// Todas as extensões de um fork (inclusive as do arquivo de overflow).
    fn fork_extents(&self, f: &Fork, file_id: u32, fork_type: u8) -> Vec<Extent> {
        let mut ex: Vec<(u32, u32)> = f.extents.clone();
        let mut have: u64 = ex.iter().map(|e| e.1 as u64).sum();
        if have < f.blocks as u64 && file_id >= 2 {
            ex.extend(self.overflow_extents(file_id, fork_type, have as u32));
            have = ex.iter().map(|e| e.1 as u64).sum();
            let _ = have;
        }
        merge_extents(ex.into_iter().map(|(s, c)| Extent::new(self.block_off(s as u64), c as u64 * self.bs)).collect::<Vec<_>>())
            .into_iter()
            .collect()
    }

    /// Extensões além das oito iniciais, procuradas no arquivo de overflow (árvore B+).
    fn overflow_extents(&self, file_id: u32, fork_type: u8, mut start: u32) -> Vec<(u32, u32)> {
        let mut out = vec![];
        let node_size = match self.tree_node_size(&self.extents_file) {
            Some(n) => n,
            None => return out,
        };
        let total_nodes = self.extents_file.size / node_size as u64;
        for n in 1..total_nodes.min(100_000) {
            let Some(node) = self.read_tree_node(&self.extents_file, n as u32, node_size) else { continue };
            if node[8] != 0xFF {
                continue;
            }
            for r in records(&node) {
                // chave: keyLength u16, forkType u8, pad u8, fileID u32, startBlock u32
                if r.len() < 12 + 64 || be16(r, 0) != 10 {
                    continue;
                }
                if r[2] == fork_type && be32(r, 4) == file_id && be32(r, 8) == start {
                    let mut got = 0;
                    for i in 0..8 {
                        let (s, c) = (be32(r, 12 + i * 8), be32(r, 16 + i * 8));
                        if c != 0 {
                            out.push((s, c));
                            got += c;
                        }
                    }
                    start += got;
                }
            }
        }
        out
    }

    fn tree_node_size(&self, f: &Fork) -> Option<usize> {
        let first = self.read_fork_bytes(f, 0, 64)?;
        let ns = be16(&first, 32) as usize;
        (512..=65536).contains(&ns).then_some(ns)
    }

    fn read_tree_node(&self, f: &Fork, n: u32, ns: usize) -> Option<Vec<u8>> {
        self.read_fork_bytes(f, n as u64 * ns as u64, ns)
    }

    /// Lê bytes de um fork que tem só extensões iniciais ou overflow já resolvido (para árvores).
    fn read_fork_bytes(&self, f: &Fork, off: u64, len: usize) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        let mut pos = 0u64;
        for &(s, c) in &f.extents {
            let elen = c as u64 * self.bs;
            if off + out.len() as u64 >= pos && off + (out.len() as u64) < pos + elen {
                let within = off + out.len() as u64 - pos;
                let n = ((elen - within) as usize).min(len - out.len());
                out.extend(read_vec(self.src, self.block_off(s as u64) + within, n).ok()?);
                if out.len() == len {
                    return Some(out);
                }
            }
            pos += elen;
        }
        (out.len() == len).then_some(out)
    }
}

/// Registros de um nó B+ (fatias, da chave em diante).
fn records(node: &[u8]) -> Vec<&[u8]> {
    let n = be16(node, 10) as usize;
    let mut offs = vec![];
    for i in 0..=n {
        let p = node.len().saturating_sub(2 * (i + 1));
        offs.push(be16(node, p) as usize);
    }
    let mut out = vec![];
    for i in 0..n {
        let (a, b) = (offs[i], offs[i + 1]);
        if a >= 14 && b > a && b <= node.len() {
            out.push(&node[a..b]);
        }
    }
    out
}

fn utf16be(b: &[u8]) -> String {
    let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&u).replace('/', "_").replace(':', "/")
}

struct CatFile {
    parent: u32,
    name: String,
    id: u32,
    data: Fork,
    created: u32,
    modified: u32,
    accessed: u32,
    stale: bool,
}

fn date(d: u32) -> Option<i64> {
    (d != 0).then(|| d as i64 - HFS_EPOCH)
}

/// Decodifica um registro do catálogo (chave + dados). `max_id` descarta lixo.
fn decode_record(r: &[u8], max_id: u32, files: &mut Vec<CatFile>, threads: &mut HashMap<u32, (u32, String)>, folders: &mut HashMap<(u32, String), u32>, stale: bool) {
    let klen = be16(r, 0) as usize;
    if klen < 6 || klen + 2 > r.len() {
        return;
    }
    let parent = be32(r, 2);
    let nlen = be16(r, 6) as usize;
    if nlen > 255 || klen != 6 + nlen * 2 && klen != 6 {
        return;
    }
    let name = utf16be(r.get(8..8 + nlen * 2).unwrap_or(&[]));
    let mut d = 2 + klen;
    if d % 2 == 1 {
        d += 1;
    }
    let Some(rec) = r.get(d..) else { return };
    match be16(rec, 0) {
        1 if rec.len() >= 88 => {
            let id = be32(rec, 8);
            if id >= 2 && id < max_id.max(16) {
                folders.entry((parent, name)).or_insert(id);
            }
        }
        3 if rec.len() >= 10 => {
            // thread de pasta: chave = (id da pasta, ""); dados = (pai, nome)
            let pn = be16(rec, 8) as usize;
            if pn <= 255 {
                threads.entry(parent).or_insert((be32(rec, 4), utf16be(rec.get(10..10 + pn * 2).unwrap_or(&[]))));
            }
        }
        2 if rec.len() >= 248 => {
            let id = be32(rec, 8);
            if id < 16 || id >= max_id.max(16) {
                return;
            }
            let data = parse_fork(&rec[88..168]);
            files.push(CatFile {
                parent,
                name,
                id,
                data,
                created: be32(rec, 12),
                modified: be32(rec, 16),
                accessed: be32(rec, 24),
                stale,
            });
        }
        4 if rec.len() >= 10 => {
            let pn = be16(rec, 8) as usize;
            if pn <= 255 {
                threads.entry(parent).or_insert((be32(rec, 4), utf16be(rec.get(10..10 + pn * 2).unwrap_or(&[]))));
            }
        }
        _ => {}
    }
}

/// Procura registros de arquivo nas sobras de um nó (entre os dados e a tabela de offsets).
fn scan_slack(node: &[u8], max_id: u32, files: &mut Vec<CatFile>, threads: &mut HashMap<u32, (u32, String)>, folders: &mut HashMap<(u32, String), u32>) {
    let n = be16(node, 10) as usize;
    let table = node.len().saturating_sub(2 * (n + 1));
    let used_end = (be16(node, table) as usize).max(14);
    let mut p = used_end + (used_end % 2);
    while p + 40 < table {
        let klen = be16(node, p) as usize;
        let nlen = be16(node, p + 6) as usize;
        if klen == 6 + nlen * 2 && (1..=255).contains(&nlen) && p + 2 + klen + 2 < table {
            let parent = be32(node, p + 2);
            if parent >= 1 && parent < max_id.max(16) {
                let d = (p + 2 + klen + 1) & !1;
                let rt = be16(node, d);
                if matches!(rt, 1 | 2 | 3 | 4) {
                    let end = (d + 248).min(table);
                    decode_record(&node[p..end], max_id, files, threads, folders, true);
                    p = d + if rt == 2 { 248 } else { 16 };
                    continue;
                }
            }
        }
        p += 2;
    }
}

pub fn scan(ctx: &mut Ctx, base: u64, part_len: u64) -> VolumeScan {
    let mut res = VolumeScan::default();
    let _ = part_len;
    let Some(vbase) = locate(ctx.src, base) else { return res };
    let Some(vol) = Vol::open(ctx.src, vbase) else { return res };

    // Mapa de espaço livre.
    if let Some(bm) = vol.read_fork_all(&vol.alloc, (vol.total / 8 + 1) as usize) {
        let mut free = vec![];
        let mut run: Option<u64> = None;
        for b in 0..vol.total {
            let used = bm.get((b / 8) as usize).map(|x| x >> (7 - (b % 8)) & 1 == 1).unwrap_or(true);
            match (used, run) {
                (false, None) => run = Some(b),
                (true, Some(s)) => {
                    free.push(Extent::new(vol.block_off(s), (b - s) * vol.bs));
                    run = None;
                }
                _ => {}
            }
        }
        if let Some(s) = run {
            free.push(Extent::new(vol.block_off(s), (vol.total - s) * vol.bs));
        }
        res.free = Some(merge_extents(free));
    }

    let Some(ns) = vol.tree_node_size(&vol.catalog) else {
        res.warnings.push("HFS+: catálogo ilegível.".into());
        return res;
    };
    let Some(hdr) = vol.read_tree_node(&vol.catalog, 0, ns) else { return res };
    let first_leaf = be32(&hdr, 24);
    let max_id = vol.next_cnid.max(16);
    let mut files: Vec<CatFile> = vec![];
    let mut threads: HashMap<u32, (u32, String)> = HashMap::new();
    let mut folders: HashMap<(u32, String), u32> = HashMap::new();
    let mut live_ids: HashSet<u32> = HashSet::new();

    let mut n = first_leaf;
    let mut guard = 0;
    while n != 0 && guard < 5_000_000 {
        guard += 1;
        if (ctx.cancelled)() {
            return res;
        }
        let Some(node) = vol.read_tree_node(&vol.catalog, n, ns) else { break };
        if node[8] != 0xFF {
            break;
        }
        let mut live_here: Vec<CatFile> = vec![];
        for r in records(&node) {
            decode_record(r, max_id, &mut live_here, &mut threads, &mut folders, false);
        }
        for f in &live_here {
            live_ids.insert(f.id);
        }
        files.extend(live_here);
        scan_slack(&node, max_id, &mut files, &mut threads, &mut folders);
        n = be32(&node, 0);
    }

    // Diário: cópias antigas de nós folha do catálogo.
    if vol.journal_info != 0 {
        scan_journal(&vol, ns, max_id, &mut files, &mut threads, &mut folders, ctx);
    }

    // Caminho de cada pasta a partir dos threads.
    let path_of = |parent: u32| -> String {
        let mut parts = vec![];
        let mut cur = parent;
        for _ in 0..64 {
            if cur <= 2 {
                break;
            }
            match threads.get(&cur) {
                Some((p, name)) => {
                    parts.push(name.clone());
                    cur = *p;
                }
                None => break,
            }
        }
        parts.reverse();
        format!("/{}", parts.join("/"))
    };

    let free_set = res.free.clone().unwrap_or_default();
    let mut seen: HashSet<(u32, u64)> = HashSet::new();
    for f in files.iter().filter(|f| f.stale && !live_ids.contains(&f.id)) {
        if f.data.size == 0 || f.data.extents.is_empty() {
            continue;
        }
        let ex = vol.fork_extents(&f.data, f.id, 0);
        let total_len: u64 = ex.iter().map(|e| e.len).sum();
        if ex.is_empty() || total_len < f.data.size || ex.iter().any(|e| e.end() > vol.base + vol.total * vol.bs) {
            continue;
        }
        if !seen.insert((f.id, ex[0].offset)) {
            continue;
        }
        // Mantém só os bytes do arquivo.
        let extents = super::fat::trim_extents(ex, f.data.size);
        // Blocos já reaproveitados?
        let reused = extents.iter().filter(|e| !covered_by(&free_set, e)).count();
        let (conf, note) = if reused == 0 {
            (Confidence::High, "registro apagado do catálogo HFS+; blocos ainda livres".to_string())
        } else {
            (Confidence::Low, "alguns blocos do arquivo já foram reaproveitados".to_string())
        };
        let nf = NewFound {
            name: f.name.clone(),
            dir: Some(path_of(f.parent)),
            size: f.data.size,
            origin: Origin::Hfs,
            data: DataLoc::Extents { extents },
            modified: date(f.modified),
            created: date(f.created),
            accessed: date(f.accessed),
            base: conf,
            note: Some(&note),
        };
        let found = make_found(ctx, nf);
        (ctx.emit)(found);
    }
    res
}

fn covered_by(free: &[Extent], e: &Extent) -> bool {
    let i = free.partition_point(|f| f.end() <= e.offset);
    free.get(i).map(|f| f.offset <= e.offset && f.end() >= e.end()).unwrap_or(false)
}

impl Vol<'_> {
    fn read_fork_all(&self, f: &Fork, max: usize) -> Option<Vec<u8>> {
        let mut out = vec![];
        for &(s, c) in &f.extents {
            let n = (c as u64 * self.bs) as usize;
            out.extend(read_vec(self.src, self.block_off(s as u64), n.min(max.saturating_sub(out.len()))).ok()?);
            if out.len() >= max {
                break;
            }
        }
        Some(out)
    }
}

/// Procura nós folha do catálogo nas transações guardadas no diário.
fn scan_journal(vol: &Vol, ns: usize, max_id: u32, files: &mut Vec<CatFile>, threads: &mut HashMap<u32, (u32, String)>, folders: &mut HashMap<(u32, String), u32>, ctx: &Ctx) {
    let Some(info) = read_vec(vol.src, vol.block_off(vol.journal_info as u64), 52).ok().filter(|b| b.len() == 52) else { return };
    let (joff, jsize) = (be64(&info, 36), be64(&info, 44));
    if jsize == 0 || jsize > 1 << 30 || vol.base + joff + jsize > vol.src.size() {
        return;
    }
    let mut pos = 0u64;
    let chunk = 4 << 20;
    while pos < jsize {
        if (ctx.cancelled)() {
            return;
        }
        let n = chunk.min(jsize - pos) as usize;
        let Ok(buf) = read_vec(vol.src, vol.base + joff + pos, n + ns) else { break };
        let mut i = 0;
        while i + ns <= buf.len().min(n + ns) && i < n {
            let nd = &buf[i..i + ns];
            // nó folha do catálogo: kind = -1, altura 1, registros plausíveis
            if nd[8] == 0xFF && nd[9] == 1 && be16(nd, 10) > 0 && (be16(nd, 10) as usize) < ns / 8 {
                let table = ns - 2 * (be16(nd, 10) as usize + 1);
                if (be16(nd, ns - 2) as usize) == 14 && be16(nd, table) as usize > 14 && (be16(nd, table) as usize) <= table {
                    for r in records(nd) {
                        decode_record(r, max_id, files, threads, folders, true);
                    }
                    scan_slack(nd, max_id, files, threads, folders);
                }
            }
            i += 512;
        }
        pos += n as u64;
    }
}
