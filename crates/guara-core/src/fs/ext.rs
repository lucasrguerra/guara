//! ext2/3/4: arquivos apagados com nome, por três caminhos que se completam:
//!
//! 1. inodes com `dtime` cujo mapa de extents ainda tem sobras legíveis;
//! 2. cópias antigas do inode guardadas no diário (jbd2), de antes da exclusão;
//! 3. entradas de diretório "escondidas" na folga do `rec_len` (dão o nome).
//!
//! Também devolve o mapa de blocos livres, para o carving varrer só o que está livre.

use super::{make_found, merge_extents, Ctx, NewFound, VolumeScan};
use crate::signatures;
use crate::source::{read_vec, Source};
use crate::types::{Confidence, DataLoc, Extent, Origin};
use std::collections::{HashMap, HashSet};

const EXT4_EXTENTS_FL: u32 = 0x80000;
const EXT4_INLINE_DATA_FL: u32 = 0x1000_0000;
const EXT_MAGIC: u16 = 0xF30A;
const JBD2_MAGIC: u32 = 0xC03B_3998;

fn le16(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).unwrap_or(0)
}
fn le32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
fn be32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|s| u32::from_be_bytes(s.try_into().unwrap())).unwrap_or(0)
}

#[derive(Clone, Copy, Default)]
struct Group {
    block_bitmap: u64,
    inode_bitmap: u64,
    inode_table: u64,
    flags: u16,
    itable_unused: u32,
}

#[derive(Clone)]
pub struct Inode {
    pub mode: u16,
    pub size: u64,
    pub atime: u32,
    pub ctime: u32,
    pub mtime: u32,
    pub dtime: u32,
    pub links: u16,
    pub flags: u32,
    pub block: [u8; 60],
    pub crtime: u32,
}

impl Inode {
    fn parse(b: &[u8]) -> Option<Inode> {
        if b.len() < 128 {
            return None;
        }
        let mut block = [0u8; 60];
        block.copy_from_slice(&b[40..100]);
        let mode = le16(b, 0);
        let mut size = le32(b, 4) as u64;
        if mode & 0xF000 == 0x8000 {
            size |= (le32(b, 0x6C) as u64) << 32;
        }
        Some(Inode {
            mode,
            size,
            atime: le32(b, 8),
            ctime: le32(b, 12),
            mtime: le32(b, 16),
            dtime: le32(b, 20),
            links: le16(b, 26),
            flags: le32(b, 32),
            block,
            crtime: if b.len() >= 0x98 { le32(b, 0x90) } else { 0 },
        })
    }
    fn is_reg(&self) -> bool {
        self.mode & 0xF000 == 0x8000
    }
    fn is_dir(&self) -> bool {
        self.mode & 0xF000 == 0x4000
    }
}

struct Ext<'a> {
    src: &'a dyn Source,
    base: u64,
    bs: u64,
    blocks: u64,
    first_data: u64,
    bpg: u64,
    ipg: u64,
    inode_size: u64,
    inodes: u64,
    groups: Vec<Group>,
    journal_inum: u32,
}

/// Sequência de (bloco lógico, bloco físico, quantidade, não-inicializado).
type ExtentList = Vec<(u64, u64, u64, bool)>;

impl<'a> Ext<'a> {
    fn open(src: &'a dyn Source, base: u64) -> Option<Self> {
        let sb = read_vec(src, base + 1024, 1024).ok().filter(|s| s.len() == 1024)?;
        if le16(&sb, 56) != 0xEF53 {
            return None;
        }
        let log = le32(&sb, 24);
        if log > 6 {
            return None;
        }
        let bs = 1024u64 << log;
        let incompat = le32(&sb, 96);
        let is64 = incompat & 0x80 != 0;
        let mut blocks = le32(&sb, 4) as u64;
        if is64 {
            blocks |= (le32(&sb, 336) as u64) << 32;
        }
        let first_data = le32(&sb, 20) as u64;
        let bpg = (le32(&sb, 32) as u64).max(1);
        let ipg = (le32(&sb, 40) as u64).max(1);
        let rev = le32(&sb, 76);
        let inode_size = if rev >= 1 { le16(&sb, 88) as u64 } else { 128 };
        if !(128..=4096).contains(&inode_size) {
            return None;
        }
        let desc_size = if is64 { (le16(&sb, 254) as u64).max(64) } else { 32 };
        let ngroups = blocks.saturating_sub(first_data).div_ceil(bpg);
        let gdt_block = first_data + 1;
        let gdt = read_vec(src, base + gdt_block * bs, (ngroups * desc_size) as usize).ok()?;
        if (gdt.len() as u64) < ngroups * desc_size {
            return None;
        }
        let mut groups = Vec::with_capacity(ngroups as usize);
        for g in 0..ngroups as usize {
            let d = &gdt[g * desc_size as usize..];
            let mut gr = Group {
                block_bitmap: le32(d, 0) as u64,
                inode_bitmap: le32(d, 4) as u64,
                inode_table: le32(d, 8) as u64,
                flags: le16(d, 0x12),
                itable_unused: le16(d, 0x1C) as u32,
            };
            if is64 && desc_size >= 64 {
                gr.block_bitmap |= (le32(d, 0x20) as u64) << 32;
                gr.inode_bitmap |= (le32(d, 0x24) as u64) << 32;
                gr.inode_table |= (le32(d, 0x28) as u64) << 32;
                gr.itable_unused |= (le16(d, 0x32) as u32) << 16;
            }
            groups.push(gr);
        }
        Some(Self {
            src,
            base,
            bs,
            blocks,
            first_data,
            bpg,
            ipg,
            inode_size,
            inodes: le32(&sb, 0) as u64,
            groups,
            journal_inum: le32(&sb, 224),
        })
    }

    fn read_block(&self, blk: u64) -> Option<Vec<u8>> {
        if blk >= self.blocks {
            return None;
        }
        let v = read_vec(self.src, self.base + blk * self.bs, self.bs as usize).ok()?;
        (v.len() as u64 == self.bs).then_some(v)
    }

    fn inode_bytes(&self, ino: u64) -> Option<Vec<u8>> {
        if ino == 0 || ino > self.inodes {
            return None;
        }
        let g = ((ino - 1) / self.ipg) as usize;
        let idx = (ino - 1) % self.ipg;
        let grp = self.groups.get(g)?;
        let off = self.base + grp.inode_table * self.bs + idx * self.inode_size;
        let v = read_vec(self.src, off, self.inode_size as usize).ok()?;
        (v.len() as u64 == self.inode_size).then_some(v)
    }

    fn read_inode(&self, ino: u64) -> Option<Inode> {
        Inode::parse(&self.inode_bytes(ino)?)
    }

    /// Extents de uma árvore ext4 (raiz em `node`, 60 bytes ou um bloco).
    fn walk_extent_node(&self, node: &[u8], depth_limit: u32, out: &mut ExtentList, strict: bool) -> bool {
        if node.len() < 12 || le16(node, 0) != EXT_MAGIC {
            return false;
        }
        let entries = le16(node, 2) as usize;
        let max = le16(node, 4) as usize;
        let depth = le16(node, 6) as u32;
        if entries > max || depth > depth_limit || (strict && entries == 0) {
            return false;
        }
        for i in 0..entries {
            let e = match node.get(12 + i * 12..24 + i * 12) {
                Some(e) => e,
                None => return false,
            };
            if depth == 0 {
                let lblk = le32(e, 0) as u64;
                let mut len = le16(e, 4) as u64;
                let uninit = len > 32768;
                if uninit {
                    len -= 32768;
                }
                let start = ((le16(e, 6) as u64) << 32) | le32(e, 8) as u64;
                if len == 0 || start == 0 || start + len > self.blocks {
                    return false;
                }
                out.push((lblk, start, len, uninit));
            } else {
                let child = ((le16(e, 8) as u64) << 32) | le32(e, 4) as u64;
                let Some(b) = self.read_block(child) else { return false };
                if !self.walk_extent_node(&b, depth - 1, out, true) {
                    return false;
                }
            }
        }
        true
    }

    /// Blocos de um inode ext2/3 (mapa de blocos clássico).
    fn block_map(&self, blk: &[u8; 60], max_blocks: u64) -> ExtentList {
        let mut v: Vec<u64> = vec![];
        let ptr = |i: usize| le32(blk, i * 4) as u64;
        for i in 0..12 {
            v.push(ptr(i));
        }
        let ppb = self.bs / 4;
        let ind = |b: u64, v: &mut Vec<u64>| {
            if b == 0 {
                v.extend(std::iter::repeat(0).take(ppb as usize));
            } else if let Some(d) = self.read_block(b) {
                v.extend((0..ppb as usize).map(|i| le32(&d, i * 4) as u64));
            } else {
                v.extend(std::iter::repeat(0).take(ppb as usize));
            }
        };
        if v.len() as u64 > max_blocks {
            v.truncate(max_blocks as usize);
        } else {
            ind(ptr(12), &mut v);
            if (v.len() as u64) < max_blocks {
                if ptr(13) != 0 {
                    if let Some(d) = self.read_block(ptr(13)) {
                        for i in 0..ppb as usize {
                            ind(le32(&d, i * 4) as u64, &mut v);
                            if v.len() as u64 >= max_blocks {
                                break;
                            }
                        }
                    }
                }
            }
            if (v.len() as u64) < max_blocks && ptr(14) != 0 {
                if let Some(d1) = self.read_block(ptr(14)) {
                    'o: for i in 0..ppb as usize {
                        if let Some(d2) = self.read_block(le32(&d1, i * 4) as u64) {
                            for j in 0..ppb as usize {
                                ind(le32(&d2, j * 4) as u64, &mut v);
                                if v.len() as u64 >= max_blocks {
                                    break 'o;
                                }
                            }
                        }
                    }
                }
            }
        }
        v.truncate(max_blocks as usize);
        let mut out: ExtentList = vec![];
        for (l, &p) in v.iter().enumerate() {
            if p == 0 || p >= self.blocks {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.1 + last.2 == p && last.0 + last.2 == l as u64 => last.2 += 1,
                _ => out.push((l as u64, p, 1, false)),
            }
        }
        out
    }

    /// Extents do arquivo (ordenados pelo bloco lógico), cortados em `size` bytes.
    fn file_extents(&self, ino: &Inode, size: u64) -> Vec<Extent> {
        let need = size.div_ceil(self.bs);
        let mut list: ExtentList = vec![];
        if ino.flags & EXT4_EXTENTS_FL != 0 {
            if !self.walk_extent_node(&ino.block, 5, &mut list, false) {
                return vec![];
            }
        } else if ino.flags & EXT4_INLINE_DATA_FL == 0 {
            list = self.block_map(&ino.block, need);
        }
        list.sort_by_key(|e| e.0);
        let mut out = vec![];
        let mut left = size;
        let mut cur_l = 0u64;
        for (l, p, n, uninit) in list {
            if left == 0 {
                break;
            }
            if l > cur_l {
                let hole = ((l - cur_l) * self.bs).min(left);
                out.push(Extent { offset: 0, len: hole, sparse: true });
                left -= hole;
                if left == 0 {
                    break;
                }
            }
            let take = (n * self.bs).min(left);
            out.push(Extent { offset: self.base + p * self.bs, len: take, sparse: uninit });
            left -= take;
            cur_l = l + n;
        }
        out
    }

    /// Mapa de blocos livres (bitmap), em extents absolutos.
    fn free_map(&self, ctx: &Ctx) -> (Vec<Extent>, Vec<bool>) {
        let mut free: Vec<Extent> = vec![];
        let mut allocated_all: Vec<bool> = vec![];
        let total_groups = self.groups.len() as u64;
        for (g, gr) in self.groups.iter().enumerate() {
            if (ctx.cancelled)() {
                break;
            }
            let gstart = self.first_data + g as u64 * self.bpg;
            let gblocks = self.bpg.min(self.blocks.saturating_sub(gstart));
            let mut used_bits = vec![true; gblocks as usize];
            if gr.flags & 0x2 != 0 {
                used_bits.fill(false);
                free.push(Extent::new(self.base + gstart * self.bs, gblocks * self.bs));
            } else if let Some(bm) = read_vec(self.src, self.base + gr.block_bitmap * self.bs, gblocks.div_ceil(8) as usize)
                .ok()
                .filter(|b| b.len() as u64 == gblocks.div_ceil(8))
            {
                let mut run: Option<u64> = None;
                for i in 0..gblocks {
                    let used = bm[(i / 8) as usize] >> (i % 8) & 1 == 1;
                    used_bits[i as usize] = used;
                    match (used, run) {
                        (false, None) => run = Some(i),
                        (true, Some(s)) => {
                            free.push(Extent::new(self.base + (gstart + s) * self.bs, (i - s) * self.bs));
                            run = None;
                        }
                        _ => {}
                    }
                }
                if let Some(s) = run {
                    free.push(Extent::new(self.base + (gstart + s) * self.bs, (gblocks - s) * self.bs));
                }
            }
            allocated_all.extend(used_bits);
            if g % 64 == 0 {
                (ctx.progress)(g as u64, total_groups);
            }
        }
        (merge_extents(free), allocated_all)
    }

    fn block_allocated(&self, alloc: &[bool], blk: u64) -> bool {
        blk.checked_sub(self.first_data)
            .and_then(|i| alloc.get(i as usize).copied())
            .unwrap_or(true)
    }
}

struct DirEnt {
    ino: u64,
    name: String,
    live: bool,
}

/// Lê as entradas de um bloco de diretório, inclusive as escondidas na folga.
fn parse_dir_block(b: &[u8], max_ino: u64, out: &mut Vec<DirEnt>) {
    let mut off = 0usize;
    while off + 8 <= b.len() {
        let ino = le32(b, off) as u64;
        let rec = le16(b, off + 4) as usize;
        let nlen = b[off + 6] as usize;
        if rec < 8 || rec % 4 != 0 || off + rec > b.len() || 8 + nlen > rec {
            break;
        }
        let real = (8 + nlen).div_ceil(4) * 4;
        if ino != 0 && nlen > 0 && ino <= max_ino {
            let name = String::from_utf8_lossy(&b[off + 8..off + 8 + nlen]).to_string();
            out.push(DirEnt { ino, name, live: true });
        }
        // Folga: entradas apagadas ficam coladas depois do fim "real" desta.
        let mut s = off + if ino != 0 || nlen > 0 { real } else { 8 };
        let end = off + rec;
        if ino == 0 {
            s = end; // entrada nula: sem folga confiável
        }
        while s + 8 <= end {
            let dino = le32(b, s) as u64;
            let drec = le16(b, s + 4) as usize;
            let dn = b[s + 6] as usize;
            let ok = dino != 0
                && dino <= max_ino
                && dn > 0
                && drec >= 8 + dn
                && drec % 4 == 0
                && s + drec <= end
                && b[s + 8..s + 8 + dn].iter().all(|&c| c >= 0x20 && c != b'/' && c != 0x7f);
            if !ok {
                s += 4;
                continue;
            }
            out.push(DirEnt {
                ino: dino,
                name: String::from_utf8_lossy(&b[s + 8..s + 8 + dn]).to_string(),
                live: false,
            });
            s += (8 + dn).div_ceil(4) * 4;
        }
        off += rec;
    }
}

struct Names {
    /// inode -> (pasta, nome) de entradas vivas.
    live: HashMap<u64, (String, String)>,
    /// inode -> (pasta, nome) de entradas apagadas (mais recente por varredura).
    dead: HashMap<u64, (String, String)>,
}

fn walk_dirs(ext: &Ext, ctx: &Ctx) -> Names {
    let mut names = Names { live: HashMap::new(), dead: HashMap::new() };
    let mut queue: Vec<(u64, String)> = vec![(2, String::new())];
    let mut seen: HashSet<u64> = HashSet::new();
    let mut visited = 0usize;
    while let Some((dino, path)) = queue.pop() {
        if !seen.insert(dino) || (visited % 256 == 255 && (ctx.cancelled)()) || visited > 2_000_000 {
            continue;
        }
        visited += 1;
        let Some(ino) = ext.read_inode(dino).filter(|i| i.is_dir()) else { continue };
        let mut ents = vec![];
        for e in ext.file_extents(&ino, ino.size.min(64 << 20)) {
            if e.sparse {
                continue;
            }
            let mut o = 0;
            while o < e.len {
                if let Ok(b) = read_vec(ext.src, e.offset + o, ext.bs as usize) {
                    if b.len() as u64 == ext.bs {
                        parse_dir_block(&b, ext.inodes, &mut ents);
                    }
                }
                o += ext.bs;
            }
        }
        for en in ents {
            if en.name == "." || en.name == ".." {
                continue;
            }
            if en.live {
                names.live.entry(en.ino).or_insert_with(|| (path.clone(), en.name.clone()));
                queue.push((en.ino, format!("{}/{}", path, en.name)));
            } else {
                names.dead.entry(en.ino).or_insert_with(|| (path.clone(), en.name));
            }
        }
    }
    names
}

// ---------------------------------------------------------------- diário

/// Cópias de inodes guardadas no diário: inode -> lista (ordem do diário) de registros brutos.
fn journal_inodes(ext: &Ext, ctx: &Ctx) -> HashMap<u64, Vec<Vec<u8>>> {
    let mut out: HashMap<u64, Vec<Vec<u8>>> = HashMap::new();
    if ext.journal_inum == 0 {
        return out;
    }
    let Some(jino) = ext.read_inode(ext.journal_inum as u64) else { return out };
    let jext = ext.file_extents(&jino, jino.size.min(1 << 30));
    // Blocos físicos do diário em ordem lógica.
    let mut jblocks: Vec<u64> = vec![];
    for e in &jext {
        if e.sparse {
            continue;
        }
        for k in 0..e.len / ext.bs {
            jblocks.push((e.offset - ext.base) / ext.bs + k);
        }
    }
    if jblocks.len() < 4 {
        return out;
    }
    let Some(jsb) = ext.read_block(jblocks[0]) else { return out };
    if be32(&jsb, 0) != JBD2_MAGIC {
        return out;
    }
    let jbs = be32(&jsb, 12) as u64;
    if jbs != ext.bs {
        return out;
    }
    let incompat = be32(&jsb, 40);
    let is64 = incompat & 0x2 != 0;
    let csum_v3 = incompat & 0x10 != 0;
    let csum_v2 = incompat & 0x8 != 0;
    let tag_bytes = if csum_v3 { 16 } else { 8 + if is64 { 4 } else { 0 } };
    let have_csum = csum_v2 || csum_v3;
    // Tabela de inodes: faixa de blocos de cada grupo.
    let itable_blocks = (ext.ipg * ext.inode_size).div_ceil(ext.bs);
    let mut tables: Vec<(u64, u64, u64)> = vec![]; // (início, fim, primeiro inode)
    for (g, gr) in ext.groups.iter().enumerate() {
        tables.push((gr.inode_table, gr.inode_table + itable_blocks, g as u64 * ext.ipg + 1));
    }
    let n = jblocks.len();
    let first = (be32(&jsb, 20) as usize).clamp(1, n);
    let mut seqs: Vec<(u32, usize)> = vec![]; // (sequência, índice do descritor)
    let mut idx = first;
    // Varredura física: qualquer bloco que seja descritor, em ordem do diário.
    while idx < n {
        if idx % 4096 == 4095 && (ctx.cancelled)() {
            break;
        }
        let Some(d) = ext.read_block(jblocks[idx]) else {
            idx += 1;
            continue;
        };
        if be32(&d, 0) == JBD2_MAGIC && be32(&d, 4) == 1 {
            seqs.push((be32(&d, 8), idx));
        }
        idx += 1;
    }
    seqs.sort_by_key(|s| s.0);
    for (_seq, didx) in seqs {
        let Some(d) = ext.read_block(jblocks[didx]) else { continue };
        let limit = d.len() - if have_csum { 4 } else { 0 };
        let mut off = 12usize;
        let mut data_idx = didx + 1;
        loop {
            if off + tag_bytes > limit {
                break;
            }
            let mut blk = be32(&d, off) as u64;
            let (flags, hi) = if csum_v3 {
                (be32(&d, off + 4), be32(&d, off + 8))
            } else {
                (u16::from_be_bytes([d[off + 6], d[off + 7]]) as u32, if is64 { be32(&d, off + 8) } else { 0 })
            };
            blk |= (hi as u64) << 32;
            off += tag_bytes;
            if flags & 2 == 0 {
                off += 16;
            }
            let di = if data_idx >= n { first + (data_idx - n) } else { data_idx };
            data_idx += 1;
            if flags & 4 == 0 {
                if let Some(t) = tables.iter().position(|t| blk >= t.0 && blk < t.1) {
                    if let Some(mut data) = ext.read_block(jblocks[di.min(n - 1)]) {
                        if flags & 1 != 0 {
                            data[0..4].copy_from_slice(&JBD2_MAGIC.to_be_bytes());
                        }
                        let per_block = (ext.bs / ext.inode_size) as usize;
                        let first_ino = tables[t].2 + (blk - tables[t].0) * per_block as u64;
                        for k in 0..per_block {
                            let rec = &data[k * ext.inode_size as usize..(k + 1) * ext.inode_size as usize];
                            if rec[..128].iter().any(|&x| x != 0) {
                                out.entry(first_ino + k as u64).or_default().push(rec.to_vec());
                            }
                        }
                    }
                }
            }
            if flags & 8 != 0 {
                break;
            }
        }
    }
    out
}

// -------------------------------------------------------------- varredura

pub fn scan(ctx: &mut Ctx, base: u64, part_len: u64) -> VolumeScan {
    let mut res = VolumeScan::default();
    let Some(ext) = Ext::open(ctx.src, base) else {
        return res;
    };
    let (free, alloc) = ext.free_map(ctx);
    let _ = part_len;
    res.free = Some(free);

    let names = walk_dirs(&ext, ctx);
    let journal = journal_inodes(&ext, ctx);
    let mut emitted: HashSet<u64> = HashSet::new();
    let mut seen_extents: HashSet<(u64, u64)> = HashSet::new();

    // 1) Inodes apagados na tabela atual.
    let total = ext.groups.len() as u64;
    for (g, gr) in ext.groups.iter().enumerate() {
        if g % 16 == 15 && (ctx.cancelled)() {
            return res;
        }
        if gr.flags & 0x1 != 0 {
            continue; // INODE_UNINIT
        }
        let used = ext.ipg.saturating_sub(gr.itable_unused as u64).min(ext.ipg);
        let Some(table) = read_vec(ext.src, ext.base + gr.inode_table * ext.bs, (used * ext.inode_size) as usize).ok() else {
            continue;
        };
        for i in 0..(table.len() as u64 / ext.inode_size) {
            let ino_no = g as u64 * ext.ipg + i + 1;
            let raw = &table[(i * ext.inode_size) as usize..((i + 1) * ext.inode_size) as usize];
            let Some(ino) = Inode::parse(raw) else { continue };
            if names.live.contains_key(&ino_no) || ino_no < 11 || ino.mode == 0 {
                continue;
            }
            let deleted = ino.dtime != 0 || ino.links == 0;
            if !deleted || !ino.is_reg() {
                continue;
            }
            // Melhor fonte: a cópia do diário anterior à exclusão; senão o que sobrou no inode.
            let mut cand: Option<(Inode, &str)> = None;
            if let Some(snaps) = journal.get(&ino_no) {
                for raw in snaps.iter().rev() {
                    if let Some(j) = Inode::parse(raw) {
                        if j.is_reg() && j.size > 0 && j.dtime == 0 && j.links > 0 {
                            let ex = ext.file_extents(&j, j.size);
                            if ex.iter().any(|e| !e.sparse) {
                                cand = Some((j, "diário"));
                                break;
                            }
                        }
                    }
                }
            }
            if cand.is_none() {
                cand = Some((ino.clone(), "inode"));
            }
            if let Some((c, src_kind)) = cand {
                if emit_inode(ctx, &ext, &alloc, &names, ino_no, &c, src_kind, &mut seen_extents) {
                    emitted.insert(ino_no);
                }
            }
        }
        if g % 16 == 0 {
            (ctx.progress)(g as u64, total);
        }
    }

    // 2) Inodes que só existem no diário (liberados e já reaproveitados/zerados).
    for (ino_no, snaps) in &journal {
        if emitted.contains(ino_no) || names.live.contains_key(ino_no) || *ino_no < 11 {
            continue;
        }
        let cur = ext.read_inode(*ino_no);
        // Se o inode atual está vivo e é outro arquivo, não mexe.
        if cur.as_ref().map(|c| c.dtime == 0 && c.links > 0 && c.mode != 0).unwrap_or(false) {
            continue;
        }
        for raw in snaps.iter().rev() {
            if let Some(j) = Inode::parse(raw) {
                if j.is_reg() && j.size > 0 && j.dtime == 0 && j.links > 0 {
                    if emit_inode(ctx, &ext, &alloc, &names, *ino_no, &j, "diário", &mut seen_extents) {
                        break;
                    }
                }
            }
        }
    }
    res
}

#[allow(clippy::too_many_arguments)]
fn emit_inode(
    ctx: &mut Ctx,
    ext: &Ext,
    alloc: &[bool],
    names: &Names,
    ino_no: u64,
    ino: &Inode,
    src_kind: &str,
    seen: &mut HashSet<(u64, u64)>,
) -> bool {
    let mut extents: Vec<Extent>;
    let mut size = ino.size;
    let mut estimated = false;
    if ino.flags & EXT4_EXTENTS_FL != 0 {
        let mut list: ExtentList = vec![];
        if !ext.walk_extent_node(&ino.block, 5, &mut list, true) {
            // Sobras de extent em inode truncado: cabeçalho zerado, registros ainda lá.
            list = remnant_extents(ext, &ino.block);
        }
        if list.is_empty() {
            return false;
        }
        list.sort_by_key(|e| e.0);
        if size == 0 {
            size = list.iter().map(|e| e.2).sum::<u64>() * ext.bs;
            estimated = true;
        }
        extents = layout_from_list(ext, &list, size);
    } else {
        if size == 0 {
            return false;
        }
        extents = ext.file_extents(ino, size);
    }
    if extents.is_empty() || extents.iter().all(|e| e.sparse) {
        return false;
    }
    let first = extents.iter().find(|e| !e.sparse).map(|e| (e.offset, e.len)).unwrap();
    if !seen.insert((first.0, first.1)) {
        return false;
    }
    // Quantos blocos de dados continuam livres? Reaproveitados => baixa confiança.
    let mut reused = 0u64;
    let mut total_blocks = 0u64;
    for e in extents.iter().filter(|e| !e.sparse) {
        let b0 = (e.offset - ext.base) / ext.bs;
        for k in 0..e.len.div_ceil(ext.bs) {
            total_blocks += 1;
            if ext.block_allocated(alloc, b0 + k) {
                reused += 1;
            }
        }
    }
    let mut note = Some(format!("recuperado do {src_kind} do ext4"));
    let mut conf = Confidence::High;
    if reused > 0 {
        conf = if reused * 2 > total_blocks { Confidence::Low } else { Confidence::Medium };
        note = Some(format!("{reused} de {total_blocks} blocos já foram reaproveitados por outros arquivos"));
    }
    if estimated {
        // Tamanho desconhecido: tenta achar o fim real pelo conteúdo.
        if let Some(h) = extents
            .iter()
            .find(|e| !e.sparse)
            .and_then(|e| read_vec(ext.src, e.offset, 128).ok())
            .and_then(|head| signatures::detect(ext.src, first.0, &head, size))
        {
            if h.len <= size && h.len > 0 {
                size = h.len;
                extents = super::fat::trim_extents(extents, size);
                estimated = false;
            }
        }
        if estimated {
            conf = conf.min(Confidence::Medium);
            note = Some("tamanho exato perdido na exclusão; usando todos os blocos do arquivo".into());
        }
    }
    let (dir, name) = match names.dead.get(&ino_no).or_else(|| names.live.get(&ino_no)) {
        Some((d, n)) => (Some(d.clone()), n.clone()),
        None => (None, format!("inode_{ino_no}")),
    };
    let ts = |t: u32| (t != 0).then_some(t as i64);
    let nf = NewFound {
        name,
        dir,
        size,
        origin: Origin::Ext,
        data: DataLoc::Extents { extents },
        modified: ts(ino.mtime),
        created: ts(ino.crtime),
        accessed: ts(ino.atime),
        base: conf,
        note: note.as_deref(),
    };
    let f = make_found(ctx, nf);
    (ctx.emit)(f);
    true
}

fn layout_from_list(ext: &Ext, list: &ExtentList, size: u64) -> Vec<Extent> {
    let mut out = vec![];
    let mut left = size;
    let mut cur = 0u64;
    for &(l, p, n, uninit) in list {
        if left == 0 {
            break;
        }
        if l > cur {
            let hole = ((l - cur) * ext.bs).min(left);
            out.push(Extent { offset: 0, len: hole, sparse: true });
            left -= hole;
            if left == 0 {
                break;
            }
        }
        let take = (n * ext.bs).min(left);
        out.push(Extent { offset: ext.base + p * ext.bs, len: take, sparse: uninit });
        left -= take;
        cur = l + n;
    }
    out
}

/// Lê registros de extent deixados em i_block depois que o cabeçalho foi zerado.
fn remnant_extents(ext: &Ext, block: &[u8; 60]) -> ExtentList {
    let mut out: ExtentList = vec![];
    if le16(block, 0) != EXT_MAGIC || le16(block, 6) != 0 {
        return out;
    }
    let max = (le16(block, 4) as usize).min(4);
    let mut next_l = 0u64;
    for i in 0..max {
        let e = &block[12 + i * 12..24 + i * 12];
        let lblk = le32(e, 0) as u64;
        let mut len = le16(e, 4) as u64;
        let uninit = len > 32768;
        if uninit {
            len -= 32768;
        }
        let start = ((le16(e, 6) as u64) << 32) | le32(e, 8) as u64;
        if len == 0 || start < ext.first_data || start + len > ext.blocks || lblk < next_l {
            break;
        }
        next_l = lblk + len;
        out.push((lblk, start, len, uninit));
    }
    out
}

/// Quão coerente é a estrutura do volume: fração das entradas de diretório cujo inode confere
/// com o tipo declarado. Serve para escolher parâmetros de RAID sem metadados.
pub fn consistency_score(src: &dyn Source, base: u64) -> Option<f64> {
    let ext = Ext::open(src, base)?;
    let mut queue = vec![2u64];
    let mut seen = HashSet::new();
    let (mut ok, mut total) = (0u32, 0u32);
    let (mut dots_ok, mut dots_total) = (0u32, 0u32);
    while let Some(d) = queue.pop() {
        if !seen.insert(d) || total >= 600 {
            continue;
        }
        let Some(ino) = ext.read_inode(d).filter(|i| i.is_dir()) else { continue };
        let mut ents = vec![];
        for e in ext.file_extents(&ino, ino.size.min(1 << 20)) {
            if e.sparse {
                continue;
            }
            let mut o = 0;
            while o < e.len {
                if let Ok(b) = read_vec(ext.src, e.offset + o, ext.bs as usize) {
                    if b.len() as u64 == ext.bs {
                        parse_dir_block(&b, ext.inodes, &mut ents);
                    }
                }
                o += ext.bs;
            }
        }
        // A entrada "." de um diretório aponta para ele mesmo: só confere se inode e bloco
        // vieram do lugar certo.
        dots_total += 1;
        if ents.iter().any(|e| e.live && e.name == "." && e.ino == d) {
            dots_ok += 1;
        }
        for en in ents.iter().filter(|e| e.live && e.name != "." && e.name != "..") {
            total += 1;
            if let Some(i) = ext.read_inode(en.ino) {
                if i.mode != 0 && i.links > 0 {
                    ok += 1;
                    if i.is_dir() {
                        queue.push(en.ino);
                    }
                }
            }
            if total >= 600 {
                break;
            }
        }
    }
    if total == 0 {
        return None;
    }
    let a = ok as f64 / total as f64;
    let b = if dots_total > 0 { dots_ok as f64 / dots_total as f64 } else { 0.0 };
    Some(1.0 + 5.0 * a + 5.0 * b)
}
