//! APFS (volumes sem cifragem): lê o contêiner, o mapa de objetos e a árvore de arquivos de cada
//! volume; arquivos apagados vêm de duas fontes: as árvores dos instantâneos (snapshots) e os nós
//! antigos da árvore (copy-on-write) que ainda estão no disco. Volumes cifrados (FileVault) são
//! detectados e deixados para a varredura por assinatura.

use super::{make_found, merge_extents, Ctx, NewFound, VolumeScan};
use crate::source::{read_vec, Source};
use crate::types::{Confidence, DataLoc, Extent, Origin};
use std::collections::{BTreeMap, HashMap, HashSet};

fn le16(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .unwrap_or(0)
}
fn le32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}
fn le64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}

const OBJ_TYPE_MASK: u32 = 0xFFFF;
const T_BTREE_ROOT: u32 = 2;
const T_BTREE_NODE: u32 = 3;
const T_SPACEMAN: u32 = 5;
const T_CIB: u32 = 7;
const T_CHECKPOINT_MAP: u32 = 0x0C;
const SUBTYPE_FSTREE: u32 = 0x0E;

// tipos de registro da árvore de arquivos
const R_SNAP_METADATA: u8 = 1;
const R_INODE: u8 = 3;
const R_FILE_EXTENT: u8 = 8;
const R_DIR_REC: u8 = 9;

/// Soma de verificação Fletcher-64 do APFS (cobre o bloco menos os 8 bytes iniciais).
pub fn fletcher64(data: &[u8]) -> u64 {
    let (mut s1, mut s2) = (0u64, 0u64);
    for w in data.chunks_exact(4) {
        s1 = (s1 + u32::from_le_bytes(w.try_into().unwrap()) as u64) % 0xFFFF_FFFF;
        s2 = (s2 + s1) % 0xFFFF_FFFF;
    }
    let c1 = 0xFFFF_FFFF - ((s1 + s2) % 0xFFFF_FFFF);
    let c2 = 0xFFFF_FFFF - ((s1 + c1) % 0xFFFF_FFFF);
    (c2 << 32) | c1
}

struct Apfs<'a> {
    src: &'a dyn Source,
    base: u64,
    bs: u64,
    blocks: u64,
}

struct BNode {
    flags: u16,
    level: u16,
    /// (chave, valor) por entrada
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    oid: u64,
    xid: u64,
}

impl<'a> Apfs<'a> {
    fn block(&self, paddr: u64) -> Option<Vec<u8>> {
        if paddr >= self.blocks {
            return None;
        }
        let b = read_vec(self.src, self.base + paddr * self.bs, self.bs as usize).ok()?;
        (b.len() as u64 == self.bs).then_some(b)
    }

    fn checked_block(&self, paddr: u64) -> Option<Vec<u8>> {
        let b = self.block(paddr)?;
        (fletcher64(&b[8..]) == le64(&b, 0)).then_some(b)
    }

    fn parse_node(&self, b: &[u8], fixed_k: usize, fixed_v: usize) -> Option<BNode> {
        let flags = le16(b, 32);
        let level = le16(b, 34);
        let n = le32(b, 36) as usize;
        let toc_off = le16(b, 40) as usize;
        let toc_len = le16(b, 42) as usize;
        let is_fixed = flags & 4 != 0;
        let is_root = flags & 1 != 0;
        let key_base = 56 + toc_off + toc_len;
        let val_end = b.len() - if is_root { 40 } else { 0 };
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let (koff, klen, voff, vlen);
            if is_fixed {
                let o = 56 + toc_off + i * 4;
                koff = le16(b, o) as usize;
                voff = le16(b, o + 2) as usize;
                klen = fixed_k;
                vlen = if level == 0 { fixed_v } else { 8 };
            } else {
                let o = 56 + toc_off + i * 8;
                koff = le16(b, o) as usize;
                klen = le16(b, o + 2) as usize;
                voff = le16(b, o + 4) as usize;
                vlen = le16(b, o + 6) as usize;
            }
            let ks = key_base + koff;
            let k = b.get(ks..ks + klen)?.to_vec();
            let v = if voff == 0xFFFF {
                vec![]
            } else {
                b.get(val_end.checked_sub(voff)?..)?.get(..vlen)?.to_vec()
            };
            entries.push((k, v));
        }
        Some(BNode {
            flags,
            level,
            entries,
            oid: le64(b, 8),
            xid: le64(b, 16),
        })
    }

    /// Todas as entradas de uma árvore física (mapa de objetos): filhos são endereços físicos.
    fn walk_physical(
        &self,
        root: u64,
        fixed_k: usize,
        fixed_v: usize,
        f: &mut dyn FnMut(&[u8], &[u8]),
    ) {
        let mut stack = vec![(root, 0u32)];
        let mut seen = HashSet::new();
        while let Some((p, depth)) = stack.pop() {
            if depth > 8 || !seen.insert(p) {
                continue;
            }
            let Some(b) = self.checked_block(p) else {
                continue;
            };
            let Some(n) = self.parse_node(&b, fixed_k, fixed_v) else {
                continue;
            };
            if n.level == 0 {
                for (k, v) in &n.entries {
                    f(k, v);
                }
            } else {
                for (_, v) in n.entries.iter().rev() {
                    stack.push((le64(v, 0), depth + 1));
                }
            }
        }
    }

    /// Mapa de objetos: oid -> [(xid, paddr)] ordenado por xid.
    fn omap(&self, omap_paddr: u64) -> HashMap<u64, Vec<(u64, u64)>> {
        let mut m: HashMap<u64, Vec<(u64, u64)>> = HashMap::new();
        let Some(ob) = self.checked_block(omap_paddr) else {
            return m;
        };
        let tree = le64(&ob, 48);
        self.walk_physical(tree, 16, 16, &mut |k, v| {
            m.entry(le64(k, 0))
                .or_default()
                .push((le64(k, 8), le64(v, 8)));
        });
        for v in m.values_mut() {
            v.sort();
        }
        m
    }

    /// Endereço físico de um objeto ephemeral (spaceman), achado no mapa de checkpoint mais novo.
    fn ephemeral_paddr(&self, nx: &[u8], oid: u64) -> Option<u64> {
        let desc_blocks = le32(nx, 104) as u64;
        let desc_base = le64(nx, 112);
        let max_entries = (self.bs as usize).saturating_sub(40) / 40;
        let mut best: Option<(u64, u64)> = None;
        for i in 0..desc_blocks {
            let Some(b) = self.checked_block(desc_base + i) else {
                continue;
            };
            if le32(&b, 24) & OBJ_TYPE_MASK != T_CHECKPOINT_MAP {
                continue;
            }
            let xid = le64(&b, 16);
            if best.is_some_and(|(x, _)| x >= xid) {
                continue;
            }
            let count = (le32(&b, 36) as usize).min(max_entries);
            for j in 0..count {
                let o = 40 + j * 40;
                if le64(&b, o + 24) == oid {
                    best = Some((xid, le64(&b, o + 32)));
                    break;
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// Mapa de espaço livre do contêiner: percorre o spaceman, os chunk-info blocks e os bitmaps
    /// (bit 1 = bloco em uso).
    fn free_map(&self, nx: &[u8]) -> Option<Vec<Extent>> {
        let sm_oid = le64(nx, 152);
        if sm_oid == 0 {
            return None;
        }
        let sm = self.checked_block(self.ephemeral_paddr(nx, sm_oid)?)?;
        if le32(&sm, 24) & OBJ_TYPE_MASK != T_SPACEMAN {
            return None;
        }
        // Com CABs (contêineres grandes) os endereços dos CIBs não ficam no spaceman.
        if le32(&sm, 68) != 0 {
            return None;
        }
        let cib_count = le32(&sm, 64) as usize;
        let addr_off = le32(&sm, 80) as usize;
        let end = addr_off.checked_add(cib_count.checked_mul(8)?)?;
        if cib_count == 0 || end > sm.len() {
            return None;
        }
        let per_cib = (self.bs as usize).saturating_sub(40) / 32;
        let mut free = vec![];
        for ci in 0..cib_count {
            let cp = le64(&sm, addr_off + ci * 8);
            let Some(cib) = self.checked_block(cp) else {
                continue;
            };
            if le32(&cib, 24) & OBJ_TYPE_MASK != T_CIB {
                continue;
            }
            let count = (le32(&cib, 36) as usize).min(per_cib);
            for j in 0..count {
                let o = 40 + j * 32;
                let addr = le64(&cib, o + 8);
                let n = le32(&cib, o + 16) as u64;
                let bm = le64(&cib, o + 24);
                if n == 0 || addr >= self.blocks {
                    continue;
                }
                let n = n.min(self.blocks - addr);
                if bm == 0 {
                    free.push(Extent::new(self.base + addr * self.bs, n * self.bs));
                    continue;
                }
                let Some(b) = self.block(bm) else {
                    continue;
                };
                let mut run: Option<u64> = None;
                for k in 0..n {
                    let used = b
                        .get((k / 8) as usize)
                        .map(|x| x >> (k % 8) & 1 == 1)
                        .unwrap_or(true);
                    match (used, run) {
                        (false, None) => run = Some(k),
                        (true, Some(s)) => {
                            free.push(Extent::new(
                                self.base + (addr + s) * self.bs,
                                (k - s) * self.bs,
                            ));
                            run = None;
                        }
                        _ => {}
                    }
                }
                if let Some(s) = run {
                    free.push(Extent::new(
                        self.base + (addr + s) * self.bs,
                        (n - s) * self.bs,
                    ));
                }
            }
        }
        Some(merge_extents(free))
    }
}

fn lookup(m: &HashMap<u64, Vec<(u64, u64)>>, oid: u64, max_xid: u64) -> Option<u64> {
    m.get(&oid)?
        .iter()
        .rev()
        .find(|(x, _)| *x <= max_xid)
        .map(|(_, p)| *p)
}

// ---------------------------------------------------------------- registros

#[derive(Clone, Debug, Default)]
struct Inode {
    parent: u64,
    private_id: u64,
    mtime: i64,
    ctime: i64,
    atime: i64,
    mode: u16,
    size: u64,
    name: Option<String>,
    has: bool,
    compressed: bool,
}

#[derive(Clone, Debug, Default)]
struct FileRec {
    inode: Inode,
    names: BTreeMap<u64, String>, // pai -> nome
}

type Files = HashMap<u64, FileRec>;
type Extents = HashMap<u64, BTreeMap<u64, (u64, u64, u64)>>; // private_id -> logical -> (len, paddr, crypto)

fn ns(t: u64) -> i64 {
    (t / 1_000_000_000) as i64
}

fn add_record(files: &mut Files, extents: &mut Extents, k: &[u8], v: &[u8]) {
    let hdr = le64(k, 0);
    let (id, ty) = (hdr & 0x0FFF_FFFF_FFFF_FFFF, (hdr >> 60) as u8);
    match ty {
        R_INODE if v.len() >= 92 => {
            let e = files.entry(id).or_default();
            let mut ino = Inode {
                parent: le64(v, 0),
                private_id: le64(v, 8),
                ctime: ns(le64(v, 16)), // criação
                mtime: ns(le64(v, 24)),
                atime: ns(le64(v, 40)),
                mode: le16(v, 80),
                size: le64(v, 84),
                has: true,
                ..Default::default()
            };
            let flags = le64(v, 48);
            ino.compressed = le32(v, 68) & 0x20 != 0;
            let _ = flags;
            // campos estendidos
            let xf = &v[92..];
            if xf.len() >= 4 {
                let n = le16(xf, 0) as usize;
                let mut p = 4 + n * 4;
                for i in 0..n {
                    let (xt, xs) = (xf[4 + i * 4], le16(xf, 6 + i * 4) as usize);
                    let data = xf.get(p..p + xs).unwrap_or(&[]);
                    match xt {
                        4 => {
                            ino.name = Some(
                                String::from_utf8_lossy(
                                    data.split(|&c| c == 0).next().unwrap_or(&[]),
                                )
                                .to_string(),
                            )
                        }
                        8 if data.len() >= 8 => ino.size = le64(data, 0),
                        _ => {}
                    }
                    p += xs.div_ceil(8) * 8;
                }
            }
            e.inode = ino;
        }
        R_DIR_REC if k.len() > 12 && v.len() >= 18 => {
            let nl = (le32(k, 8) & 0x3FF) as usize;
            if let Some(name) = k.get(12..12 + nl) {
                let name = String::from_utf8_lossy(name.split(|&c| c == 0).next().unwrap_or(&[]))
                    .to_string();
                files.entry(le64(v, 0)).or_default().names.insert(id, name);
            }
        }
        R_FILE_EXTENT if k.len() >= 16 && v.len() >= 24 => {
            let len = le64(v, 0) & 0x00FF_FFFF_FFFF_FFFF;
            extents
                .entry(id)
                .or_default()
                .insert(le64(k, 8), (len, le64(v, 8), le64(v, 16)));
        }
        _ => {}
    }
}

impl Apfs<'_> {
    /// Percorre a árvore de arquivos a partir de uma raiz virtual.
    fn walk_fstree(
        &self,
        root_oid: u64,
        omap: &HashMap<u64, Vec<(u64, u64)>>,
        xid: u64,
        f: &mut dyn FnMut(&[u8], &[u8], u64),
    ) {
        let Some(rp) = lookup(omap, root_oid, xid) else {
            return;
        };
        let mut stack = vec![(rp, 0u32)];
        let mut seen = HashSet::new();
        while let Some((p, depth)) = stack.pop() {
            if depth > 10 || !seen.insert(p) {
                continue;
            }
            let Some(b) = self.checked_block(p) else {
                continue;
            };
            let Some(n) = self.parse_node(&b, 16, 16) else {
                continue;
            };
            if n.level == 0 {
                for (k, v) in &n.entries {
                    f(k, v, p);
                }
            } else {
                for (_, v) in n.entries.iter().rev() {
                    if let Some(cp) = lookup(omap, le64(v, 0), xid) {
                        stack.push((cp, depth + 1));
                    }
                }
            }
        }
    }
}

fn path_of(files: &Files, mut id: u64) -> String {
    let mut parts = vec![];
    for _ in 0..64 {
        if id == 2 || id == 1 {
            break;
        }
        match files
            .get(&id)
            .and_then(|f| f.names.iter().next().map(|(p, n)| (*p, n.clone())))
        {
            Some((p, n)) => {
                parts.push(n);
                id = p;
            }
            None => break,
        }
    }
    parts.reverse();
    parts.join("/")
}

struct Volume {
    name: String,
    omap_paddr: u64,
    root_oid: u64,
    snap_tree_oid: u64,
    encrypted: bool,
}

fn parse_volume(b: &[u8]) -> Option<Volume> {
    if &b[32..36] != b"APSB" {
        return None;
    }
    let name = String::from_utf8_lossy(&b[704..960])
        .split('\0')
        .next()
        .unwrap_or("")
        .to_string();
    Some(Volume {
        name,
        omap_paddr: le64(b, 128),
        root_oid: le64(b, 136),
        snap_tree_oid: le64(b, 152),
        encrypted: le64(b, 264) & 1 == 0,
    })
}

fn open_container<'a>(src: &'a dyn Source, base: u64) -> Option<(Apfs<'a>, Vec<u8>)> {
    let b0 = read_vec(src, base, 4096).ok()?;
    if b0.len() < 4096 || &b0[32..36] != b"NXSB" {
        return None;
    }
    let bs = le32(&b0, 36) as u64;
    let blocks = le64(&b0, 40);
    if !(4096..=65536).contains(&bs) || !bs.is_power_of_two() {
        return None;
    }
    let a = Apfs {
        src,
        base,
        bs,
        blocks,
    };
    // O superbloco mais recente está na área de descritores de checkpoint.
    let desc_blocks = le32(&b0, 104) as u64;
    let desc_base = le64(&b0, 112);
    let mut best = b0.clone();
    for i in 0..desc_blocks {
        if let Some(b) = a.checked_block(desc_base as u64 + i) {
            if &b[32..36] == b"NXSB" && le64(&b, 16) > le64(&best, 16) {
                best = b;
            }
        }
    }
    Some((a, best))
}

pub fn scan(ctx: &mut Ctx, base: u64, part_len: u64) -> VolumeScan {
    let _ = part_len;
    let mut res = VolumeScan::default();
    let Some((a, nx)) = open_container(ctx.src, base) else {
        return res;
    };
    let Some(cont_omap_block) = a.checked_block(le64(&nx, 160)) else {
        return res;
    };
    let _ = cont_omap_block;
    res.free = a.free_map(&nx);
    if res.free.is_none() {
        res.warnings.push(
            "APFS: não foi possível ler o mapa de espaço livre; a varredura profunda usará todo o volume."
                .into(),
        );
    }
    let container_omap = a.omap(le64(&nx, 160));
    let max_fs = le32(&nx, 180).min(100) as usize;
    let mut vols: Vec<Volume> = vec![];
    for i in 0..max_fs {
        let oid = le64(&nx, 184 + i * 8);
        if oid == 0 {
            continue;
        }
        if let Some(p) = lookup(&container_omap, oid, u64::MAX) {
            if let Some(b) = a.checked_block(p) {
                if let Some(v) = parse_volume(&b) {
                    vols.push(v);
                }
            }
        }
    }
    let nvols = vols.len();
    for v in &vols {
        if v.encrypted {
            res.warnings.push(format!("APFS: o volume \"{}\" está cifrado (FileVault); os arquivos só são legíveis com a chave, então use a varredura por assinatura apenas se o volume já estiver aberto.", v.name));
            continue;
        }
        scan_volume(ctx, &a, v, nvols == 1, &mut res);
        if (ctx.cancelled)() {
            return res;
        }
    }
    res
}

fn scan_volume(ctx: &mut Ctx, a: &Apfs, v: &Volume, allow_stale_scan: bool, res: &mut VolumeScan) {
    let omap = a.omap(v.omap_paddr);
    let current_xid = u64::MAX;
    // Árvore atual.
    let mut live: Files = HashMap::new();
    let mut live_ext: Extents = HashMap::new();
    let mut live_nodes: HashSet<u64> = HashSet::new();
    a.walk_fstree(v.root_oid, &omap, current_xid, &mut |k, val, node| {
        live_nodes.insert(node);
        add_record(&mut live, &mut live_ext, k, val);
    });
    // Instantâneos.
    let mut stale: Files = HashMap::new();
    let mut stale_ext: Extents = HashMap::new();
    if v.snap_tree_oid != 0 {
        if let Some(sp) = lookup(&omap, v.snap_tree_oid, current_xid).or(Some(v.snap_tree_oid)) {
            let mut snaps: Vec<(u64, u64)> = vec![]; // (xid, sblock paddr)
            let mut stack = vec![sp];
            let mut seen = HashSet::new();
            while let Some(p) = stack.pop() {
                if !seen.insert(p) {
                    continue;
                }
                let Some(b) = a.checked_block(p) else {
                    continue;
                };
                let Some(n) = a.parse_node(&b, 0, 0) else {
                    continue;
                };
                if n.level == 0 {
                    for (k, val) in &n.entries {
                        let hdr = le64(k, 0);
                        if (hdr >> 60) as u8 == R_SNAP_METADATA && val.len() >= 40 {
                            snaps.push((hdr & 0x0FFF_FFFF_FFFF_FFFF, le64(val, 8)));
                        }
                    }
                } else {
                    for (_, val) in &n.entries {
                        // árvore de metadados de snapshot: filhos físicos ou virtuais
                        let c = le64(val, 0);
                        stack.push(lookup(&omap, c, current_xid).unwrap_or(c));
                    }
                }
            }
            for (xid, sblock) in snaps {
                let Some(sb) = a.checked_block(sblock) else {
                    continue;
                };
                let Some(sv) = parse_volume(&sb) else {
                    continue;
                };
                a.walk_fstree(sv.root_oid, &omap, xid, &mut |k, val, _| {
                    add_record(&mut stale, &mut stale_ext, k, val);
                });
            }
        }
    }
    // Nós antigos da árvore de arquivos.
    if allow_stale_scan {
        let total = a.blocks;
        for p in 0..total {
            if p % 4096 == 0 {
                if (ctx.cancelled)() {
                    return;
                }
                (ctx.progress)(p, total);
            }
            let Some(b) = a.block(p) else { continue };
            let ty = le32(&b, 24) & OBJ_TYPE_MASK;
            if (ty != T_BTREE_NODE && ty != T_BTREE_ROOT)
                || le32(&b, 28) != SUBTYPE_FSTREE
                || le16(&b, 34) != 0
            {
                continue;
            }
            if live_nodes.contains(&p) || fletcher64(&b[8..]) != le64(&b, 0) {
                continue;
            }
            if let Some(n) = a.parse_node(&b, 16, 16) {
                let _ = (n.oid, n.xid, n.flags);
                for (k, val) in &n.entries {
                    add_record(&mut stale, &mut stale_ext, k, val);
                }
            }
        }
    }
    // Arquivos que só existem nas versões antigas.
    let mut ids: Vec<&u64> = stale.keys().collect();
    ids.sort();
    let mut seen_first: HashSet<u64> = HashSet::new();
    for id in ids {
        if live.get(id).is_some_and(|f| f.inode.has) {
            continue;
        }
        let rec = &stale[id];
        let ino = &rec.inode;
        if !ino.has || ino.mode & 0o170000 != 0o100000 || ino.size == 0 {
            continue;
        }
        let Some(ex) = stale_ext.get(&ino.private_id) else {
            continue;
        };
        if ex.values().any(|e| e.2 != 0) {
            continue; // extensão cifrada
        }
        // nome e pasta: da entrada de diretório, senão do próprio inode
        let (name, parent) = match rec.names.iter().next() {
            Some((p, n)) => (n.clone(), *p),
            None => match &ino.name {
                Some(n) => (n.clone(), ino.parent),
                None => (format!("arquivo_{id}"), ino.parent),
            },
        };
        let merged: Files = stale
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .chain(live.iter().map(|(k, v)| (*k, v.clone())))
            .collect();
        let dir = Some(format!("/{}", path_of(&merged, parent)).replace("//", "/"));
        let mut extents: Vec<Extent> = vec![];
        let mut pos = 0u64;
        let mut ok = true;
        for (&lo, &(len, paddr, _)) in ex {
            if lo > pos {
                extents.push(Extent {
                    offset: 0,
                    len: lo - pos,
                    sparse: true,
                });
            }
            if paddr == 0 {
                extents.push(Extent {
                    offset: 0,
                    len,
                    sparse: true,
                });
            } else if paddr < a.blocks {
                extents.push(Extent::new(a.base + paddr * a.bs, len));
            } else {
                ok = false;
            }
            pos = lo + len;
        }
        if !ok || pos < ino.size.min(1) {
            continue;
        }
        let extents = super::fat::trim_extents(extents, ino.size);
        let Some(first) = extents.iter().find(|e| !e.sparse) else {
            continue;
        };
        if !seen_first.insert(first.offset) {
            continue;
        }
        let note = if ino.compressed {
            "arquivo comprimido pelo macOS (decmpfs): o conteúdo pode estar incompleto"
        } else {
            "versão antiga da árvore ou instantâneo do APFS"
        };
        let ts = |t: i64| (t > 0).then_some(t);
        let nf = NewFound {
            name,
            dir,
            size: ino.size,
            origin: Origin::Apfs,
            data: DataLoc::Extents { extents },
            modified: ts(ino.mtime),
            created: ts(ino.ctime),
            accessed: ts(ino.atime),
            base: if ino.compressed {
                Confidence::Low
            } else {
                Confidence::Medium
            },
            note: Some(note),
        };
        let f = make_found(ctx, nf);
        (ctx.emit)(f);
    }
    let _ = res;
}

/// Trechos livres (absolutos) do contêiner, segundo o spaceman.
pub fn free_extents(src: &dyn Source, base: u64) -> Option<Vec<Extent>> {
    let (a, nx) = open_container(src, base)?;
    a.free_map(&nx)
}

/// Arquivos e pastas atuais do primeiro volume sem cifragem (caminho, tamanho, é pasta).
pub fn live_paths(src: &dyn Source, base: u64) -> Vec<(String, u64, bool)> {
    let Some((a, nx)) = open_container(src, base) else {
        return vec![];
    };
    let cont = a.omap(le64(&nx, 160));
    let mut out = vec![];
    for i in 0..le32(&nx, 180).min(100) as usize {
        let oid = le64(&nx, 184 + i * 8);
        let Some(p) = lookup(&cont, oid, u64::MAX) else {
            continue;
        };
        let Some(vb) = a.checked_block(p) else {
            continue;
        };
        let Some(v) = parse_volume(&vb) else { continue };
        if v.encrypted {
            continue;
        }
        let omap = a.omap(v.omap_paddr);
        let mut files: Files = HashMap::new();
        let mut ext: Extents = HashMap::new();
        a.walk_fstree(v.root_oid, &omap, u64::MAX, &mut |k, val, _| {
            add_record(&mut files, &mut ext, k, val)
        });
        for (&id, f) in &files {
            if !f.inode.has || id <= 2 {
                continue;
            }
            out.push((
                path_of(&files, id),
                f.inode.size,
                f.inode.mode & 0o170000 == 0o040000,
            ));
        }
    }
    out.sort();
    out
}
