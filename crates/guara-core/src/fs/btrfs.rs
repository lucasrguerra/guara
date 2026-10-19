//! Btrfs (um dispositivo; perfis single, DUP e RAID 1): árvores atuais para nomes e mapa de
//! espaço livre, e arquivos apagados achados em nós antigos das árvores. Como o Btrfs nunca
//! sobrescreve metadados no lugar (copy-on-write), as versões anteriores dos nós continuam no
//! disco até o espaço ser reaproveitado: varremos os blocos de metadata em busca delas.
//! Extensões comprimidas (zlib, zstd, lzo) são descompactadas na recuperação.

use super::{make_found, merge_extents, Ctx, NewFound, VolumeScan};
use crate::source::{read_vec, Source};
use crate::types::{Confidence, DataLoc, Extent, Origin, Packed, PackedCodec, Part};
use std::collections::{BTreeMap, HashMap, HashSet};

const MAGIC: &[u8; 8] = b"_BHRfS_M";
const SB_OFF: u64 = 0x10000;

// chaves
const INODE_ITEM: u8 = 1;
const INODE_REF: u8 = 12;
const EXTENT_DATA: u8 = 108;
const ROOT_ITEM: u8 = 132;
const EXTENT_ITEM: u8 = 168;
const METADATA_ITEM: u8 = 169;
const CHUNK_ITEM: u8 = 228;

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

pub fn crc32c(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0x82F6_3B78
                } else {
                    c >> 1
                };
            }
            t[i as usize] = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub objectid: u64,
    pub ty: u8,
    pub offset: u64,
}

fn key_at(b: &[u8], o: usize) -> Key {
    Key {
        objectid: le64(b, o),
        ty: b.get(o + 8).copied().unwrap_or(0),
        offset: le64(b, o + 9),
    }
}

#[derive(Clone, Debug)]
struct Chunk {
    logical: u64,
    len: u64,
    ty: u64,
    /// (offset físico do primeiro stripe) por cópia
    phys: Vec<u64>,
}

#[derive(Clone, Debug)]
enum NodeKind {
    Leaf(Vec<(Key, Vec<u8>)>),
    Internal(Vec<(Key, u64)>),
}

#[derive(Clone, Debug)]
struct Node {
    bytenr: u64,
    generation: u64,
    owner: u64,
    kind: NodeKind,
}

struct Fs<'a> {
    src: &'a dyn Source,
    base: u64,
    nodesize: u64,
    sectorsize: u64,
    fsid: [u8; 16],
    chunks: Vec<Chunk>,
    root: u64,
    chunk_root: u64,
}

impl<'a> Fs<'a> {
    fn open(src: &'a dyn Source, base: u64) -> Option<Fs<'a>> {
        let sb = read_vec(src, base + SB_OFF, 4096).ok()?;
        if sb.len() < 4096 || &sb[64..72] != MAGIC {
            return None;
        }
        let csum_type = le16(&sb, 196);
        if csum_type == 0 && crc32c(&sb[32..4096]) != le32(&sb, 0) {
            return None;
        }
        let nodesize = le32(&sb, 148) as u64;
        let sectorsize = le32(&sb, 144) as u64;
        if !nodesize.is_power_of_two()
            || !(4096..=65536).contains(&nodesize)
            || !sectorsize.is_power_of_two()
        {
            return None;
        }
        let mut fsid = [0u8; 16];
        fsid.copy_from_slice(&sb[32..48]);
        let mut fs = Fs {
            src,
            base,
            nodesize,
            sectorsize,
            fsid,
            chunks: vec![],
            root: le64(&sb, 80),
            chunk_root: le64(&sb, 88),
        };
        // sys_chunk_array
        let n = le32(&sb, 160) as usize;
        let arr = sb.get(0x32B..0x32B + n.min(2048))?;
        let mut p = 0;
        while p + 17 + 48 <= arr.len() {
            let k = key_at(arr, p);
            if k.ty != CHUNK_ITEM {
                break;
            }
            let (ch, used) = parse_chunk(&arr[p + 17..], k.offset)?;
            fs.chunks.push(ch);
            p += 17 + used;
        }
        // chunk tree completa
        let mut more = vec![];
        let cr = fs.chunk_root;
        fs.walk(cr, &mut |k, d| {
            if k.ty == CHUNK_ITEM {
                if let Some((ch, _)) = parse_chunk(d, k.offset) {
                    more.push(ch);
                }
            }
        });
        for c in more {
            if !fs.chunks.iter().any(|x| x.logical == c.logical) {
                fs.chunks.push(c);
            }
        }
        fs.chunks.sort_by_key(|c| c.logical);
        Some(fs)
    }

    fn phys(&self, logical: u64) -> Option<u64> {
        let i = self
            .chunks
            .partition_point(|c| c.logical + c.len <= logical);
        let c = self.chunks.get(i)?;
        (c.logical <= logical)
            .then(|| {
                c.phys
                    .first()
                    .map(|p| self.base + p + (logical - c.logical))
            })
            .flatten()
    }

    fn read_node(&self, logical: u64) -> Option<Node> {
        let p = self.phys(logical)?;
        let raw = read_vec(self.src, p, self.nodesize as usize).ok()?;
        self.parse_node(&raw, Some(logical))
    }

    fn parse_node(&self, raw: &[u8], expect: Option<u64>) -> Option<Node> {
        if raw.len() as u64 != self.nodesize || raw[32..48] != self.fsid {
            return None;
        }
        let bytenr = le64(raw, 48);
        if expect.is_some_and(|e| e != bytenr) {
            return None;
        }
        if crc32c(&raw[32..]) != le32(raw, 0) {
            return None;
        }
        let nritems = le32(raw, 96) as usize;
        let level = raw[100];
        if nritems > (self.nodesize as usize) / 25 {
            return None;
        }
        let hdr = 101usize;
        let kind = if level == 0 {
            let mut items = Vec::with_capacity(nritems);
            for i in 0..nritems {
                let o = hdr + i * 25;
                let k = key_at(raw, o);
                let (off, size) = (le32(raw, o + 17) as usize, le32(raw, o + 21) as usize);
                let s = hdr + off;
                if s + size > raw.len() {
                    return None;
                }
                items.push((k, raw[s..s + size].to_vec()));
            }
            NodeKind::Leaf(items)
        } else {
            let mut ptrs = Vec::with_capacity(nritems);
            for i in 0..nritems {
                let o = hdr + i * 33;
                if o + 33 > raw.len() {
                    return None;
                }
                ptrs.push((key_at(raw, o), le64(raw, o + 17)));
            }
            NodeKind::Internal(ptrs)
        };
        Some(Node {
            bytenr,
            generation: le64(raw, 80),
            owner: le64(raw, 88),
            kind,
        })
    }

    /// Percorre uma árvore em ordem, chamando `f` para cada item de folha.
    fn walk(&self, root: u64, f: &mut dyn FnMut(&Key, &[u8])) {
        let mut seen = HashSet::new();
        self.walk_inner(root, f, &mut seen, &mut |_| {}, 0);
    }

    fn walk_nodes(&self, root: u64, f: &mut dyn FnMut(&Key, &[u8]), nodes: &mut dyn FnMut(&Node)) {
        let mut seen = HashSet::new();
        self.walk_inner(root, f, &mut seen, nodes, 0);
    }

    fn walk_inner(
        &self,
        at: u64,
        f: &mut dyn FnMut(&Key, &[u8]),
        seen: &mut HashSet<u64>,
        nodes: &mut dyn FnMut(&Node),
        depth: u32,
    ) {
        if depth > 10 || !seen.insert(at) {
            return;
        }
        let Some(n) = self.read_node(at) else { return };
        nodes(&n);
        match &n.kind {
            NodeKind::Leaf(items) => {
                for (k, d) in items {
                    f(k, d);
                }
            }
            NodeKind::Internal(ptrs) => {
                for (_, child) in ptrs {
                    self.walk_inner(*child, f, seen, nodes, depth + 1);
                }
            }
        }
    }
}

fn parse_chunk(d: &[u8], logical: u64) -> Option<(Chunk, usize)> {
    if d.len() < 48 {
        return None;
    }
    let len = le64(d, 0);
    let ty = le64(d, 24);
    let nstripes = le16(d, 44) as usize;
    if nstripes == 0 || nstripes > 32 || d.len() < 48 + nstripes * 32 {
        return None;
    }
    let phys = (0..nstripes).map(|i| le64(d, 48 + i * 32 + 8)).collect();
    Some((
        Chunk {
            logical,
            len,
            ty,
            phys,
        },
        48 + nstripes * 32,
    ))
}

// ---------------------------------------------------------------- inodes

#[derive(Clone, Debug, Default)]
struct InodeData {
    gen: u64,
    size: u64,
    mode: u32,
    mtime: i64,
    ctime: i64,
    atime: i64,
    has_inode: bool,
    refs: HashMap<u64, String>,
    /// offset no arquivo -> (geração do nó, item)
    extents: BTreeMap<u64, (u64, Vec<u8>)>,
}

type Files = HashMap<(u64, u64), InodeData>;

fn add_item(files: &mut Files, owner: u64, gen: u64, k: &Key, d: &[u8]) {
    match k.ty {
        INODE_ITEM if d.len() >= 160 => {
            let e = files.entry((owner, k.objectid)).or_default();
            let g = le64(d, 0);
            if !e.has_inode || g >= e.gen {
                e.has_inode = true;
                e.gen = g;
                e.size = le64(d, 16);
                e.mode = le32(d, 52);
                e.atime = le64(d, 112) as i64;
                e.ctime = le64(d, 124) as i64;
                e.mtime = le64(d, 136) as i64;
            }
        }
        INODE_REF if d.len() >= 10 => {
            let nlen = le16(d, 8) as usize;
            if let Some(n) = d.get(10..10 + nlen) {
                files
                    .entry((owner, k.objectid))
                    .or_default()
                    .refs
                    .insert(k.offset, String::from_utf8_lossy(n).to_string());
            }
        }
        EXTENT_DATA if d.len() >= 21 => {
            let e = files.entry((owner, k.objectid)).or_default();
            match e.extents.get(&k.offset) {
                Some((g, _)) if *g > gen => {}
                _ => {
                    e.extents.insert(k.offset, (gen, d.to_vec()));
                }
            }
        }
        _ => {}
    }
}

fn is_fs_tree(id: u64) -> bool {
    id == 5 || (256..(1u64 << 48)).contains(&id)
}

pub fn scan(ctx: &mut Ctx, base: u64, part_len: u64) -> VolumeScan {
    let _ = part_len;
    let mut res = VolumeScan::default();
    let Some(fs) = Fs::open(ctx.src, base) else {
        return res;
    };
    if fs
        .chunks
        .iter()
        .any(|c| c.ty & 0x1 << 3 != 0 || c.ty & 0x10 != 0 || c.ty & 0x100 != 0 || c.ty & 0x200 != 0)
    {
        res.warnings.push("Btrfs em RAID 0/10/5/6: a leitura de metadados ainda não suporta esses perfis; usando só assinaturas.".into());
        return res;
    }

    // 1) Árvores atuais: raízes das subvolumes e extents alocados.
    let mut roots: Vec<(u64, u64)> = vec![]; // (id da árvore, bytenr)
    let mut extent_root = 0u64;
    let rr = fs.root;
    fs.walk(rr, &mut |k, d| {
        if k.ty == ROOT_ITEM && d.len() >= 239 {
            let bytenr = le64(d, 176);
            if k.objectid == 2 {
                extent_root = bytenr;
            } else if is_fs_tree(k.objectid) {
                roots.push((k.objectid, bytenr));
            }
        }
    });
    let mut live_nodes: HashSet<u64> = HashSet::new();
    let mut live: Files = HashMap::new();
    for &(id, bytenr) in &roots {
        let mut tmp: Vec<(Key, Vec<u8>)> = vec![];
        let mut gens: HashMap<u64, u64> = HashMap::new();
        let mut seen_nodes: Vec<(u64, u64)> = vec![];
        {
            let mut items = |k: &Key, d: &[u8]| tmp.push((*k, d.to_vec()));
            let mut nodes = |n: &Node| {
                seen_nodes.push((n.bytenr, n.generation));
            };
            fs.walk_nodes(bytenr, &mut items, &mut nodes);
        }
        for (b, g) in seen_nodes {
            live_nodes.insert(b);
            gens.insert(b, g);
        }
        for (k, d) in &tmp {
            add_item(&mut live, id, u64::MAX, k, d);
        }
        if (ctx.cancelled)() {
            return res;
        }
    }

    // Extents alocados (árvore de extents) e mapa livre dos blocos de dados.
    let mut allocated: Vec<Extent> = vec![];
    if extent_root != 0 {
        let nodesize = fs.nodesize;
        fs.walk(extent_root, &mut |k, _| {
            if k.ty == EXTENT_ITEM {
                allocated.push(Extent::new(k.objectid, k.offset));
            } else if k.ty == METADATA_ITEM {
                allocated.push(Extent::new(k.objectid, nodesize));
            }
        });
    }
    let allocated = merge_extents(allocated);
    let mut free: Vec<Extent> = vec![];
    for c in fs.chunks.iter().filter(|c| c.ty & 0x1 != 0) {
        // blocos de dados: livres = chunk - alocado
        let mut cur = c.logical;
        let i = allocated.partition_point(|e| e.end() <= c.logical);
        for e in &allocated[i..] {
            if e.offset >= c.logical + c.len {
                break;
            }
            if e.offset > cur {
                if let Some(p) = fs.phys(cur) {
                    free.push(Extent::new(p, e.offset - cur));
                }
            }
            cur = cur.max(e.end());
        }
        if cur < c.logical + c.len {
            if let Some(p) = fs.phys(cur) {
                free.push(Extent::new(p, c.logical + c.len - cur));
            }
        }
    }
    res.free = Some(merge_extents(free));

    // 2) Varre os blocos de metadata atrás de versões antigas de folhas de árvores de arquivos.
    let mut stale: Files = HashMap::new();
    let ns = fs.nodesize as usize;
    let step = fs.sectorsize as usize;
    let md_chunks: Vec<Chunk> = fs
        .chunks
        .iter()
        .filter(|c| c.ty & 0x4 != 0)
        .cloned()
        .collect();
    let total_md: u64 = md_chunks.iter().map(|c| c.len).sum();
    let mut done_md = 0u64;
    for c in &md_chunks {
        let Some(p0) = c.phys.first().copied() else {
            continue;
        };
        let mut off = 0u64;
        let block = 8usize << 20;
        while off < c.len {
            if (ctx.cancelled)() {
                return res;
            }
            let n = ((c.len - off) as usize).min(block + ns);
            let Ok(buf) = read_vec(ctx.src, fs.base + p0 + off, n) else {
                break;
            };
            let mut i = 0usize;
            while i + ns <= buf.len() && (off + i as u64) < c.len {
                let logical = c.logical + off + i as u64;
                // cabeçalho barato antes do CRC
                if buf[i + 32..i + 48] == fs.fsid
                    && le64(&buf[i..], 48) == logical
                    && buf[i + 100] == 0
                {
                    if let Some(node) = fs.parse_node(&buf[i..i + ns], Some(logical)) {
                        if is_fs_tree(node.owner) && !live_nodes.contains(&logical) {
                            if let NodeKind::Leaf(items) = &node.kind {
                                for (k, d) in items {
                                    add_item(&mut stale, node.owner, node.generation, k, d);
                                }
                            }
                        }
                    }
                }
                i += step;
            }
            off += block as u64;
            done_md += block as u64;
            (ctx.progress)(done_md.min(total_md), total_md.max(1));
        }
    }

    // 3) Arquivos que só existem nas folhas antigas.
    let path_of = |owner: u64, mut ino: u64| -> String {
        let mut parts: Vec<String> = vec![];
        for _ in 0..64 {
            if ino == 256 {
                break;
            }
            let e = stale.get(&(owner, ino)).or_else(|| live.get(&(owner, ino)));
            match e.and_then(|e| e.refs.iter().next()) {
                Some((parent, name)) => {
                    parts.push(name.clone());
                    ino = *parent;
                }
                None => {
                    parts.push("(pasta perdida)".into());
                    break;
                }
            }
        }
        parts.reverse();
        parts.join("/")
    };
    let mut keys: Vec<&(u64, u64)> = stale.keys().collect();
    keys.sort();
    for key in keys {
        let (owner, ino) = *key;
        if live.contains_key(&(owner, ino)) {
            continue;
        }
        let e = &stale[&(owner, ino)];
        if !e.has_inode || e.mode & 0o170000 != 0o100000 || e.size == 0 || e.extents.is_empty() {
            continue;
        }
        let Some((name, parent)) = e.refs.iter().next().map(|(p, n)| (n.clone(), *p)) else {
            continue;
        };
        let dir = {
            let p = path_of(owner, parent);
            Some(if p.is_empty() {
                String::new()
            } else {
                format!("/{p}")
            })
        };
        let Some((data, any_alloc, all_ok)) = build_data(&fs, e, &allocated) else {
            continue;
        };
        let (conf, note) = if !all_ok {
            (
                Confidence::Low,
                "faltam trechos do arquivo na versão antiga da árvore".to_string(),
            )
        } else if any_alloc {
            (Confidence::Medium, "versão antiga do Btrfs; alguns blocos ainda constam como alocados (podem ser de outro arquivo)".to_string())
        } else {
            (
                Confidence::High,
                "versão antiga da árvore do Btrfs; blocos de dados livres".to_string(),
            )
        };
        let ts = |t: i64| (t > 0).then_some(t);
        let nf = NewFound {
            name,
            dir,
            size: e.size,
            origin: Origin::Btrfs,
            data,
            modified: ts(e.mtime),
            created: ts(e.ctime),
            accessed: ts(e.atime),
            base: conf,
            note: Some(&note),
        };
        let f = make_found(ctx, nf);
        (ctx.emit)(f);
    }
    res
}

/// Converte os itens EXTENT_DATA em extents físicos ou partes (inline/comprimidas).
fn build_data(fs: &Fs, e: &InodeData, allocated: &[Extent]) -> Option<(DataLoc, bool, bool)> {
    let mut parts: Vec<Part> = vec![];
    let mut pos = 0u64;
    let mut any_alloc = false;
    let mut all_ok = true;
    let mut plain_only = true;
    let mut hole = |parts: &mut Vec<Part>, len: u64| {
        if len > 0 {
            parts.push(Part::Run { offset: 0, len });
            // marcador: offset 0 + sparse tratado abaixo
        }
    };
    let _ = &mut hole;
    let mut sparse_marks: Vec<usize> = vec![];
    for (&foff, (_, d)) in &e.extents {
        if foff >= e.size {
            break;
        }
        if foff > pos {
            sparse_marks.push(parts.len());
            parts.push(Part::Run {
                offset: 0,
                len: foff - pos,
            });
            plain_only = false;
            pos = foff;
        }
        let ty = d[20];
        let comp = d[16];
        if ty == 0 {
            // inline
            let raw = &d[21..];
            let ram = le64(d, 8) as usize;
            let bytes = match comp {
                0 => raw.to_vec(),
                c => decode(c, raw, ram)?,
            };
            let len = (bytes.len() as u64).min(e.size - foff);
            parts.push(Part::Bytes {
                bytes: bytes[..len as usize].to_vec(),
            });
            plain_only = false;
            pos = foff + len;
            continue;
        }
        if d.len() < 53 {
            all_ok = false;
            break;
        }
        let (disk, disk_len, offset, num) = (le64(d, 21), le64(d, 29), le64(d, 37), le64(d, 45));
        let num = num.min(e.size.saturating_sub(foff));
        if disk == 0 || ty == 2 {
            sparse_marks.push(parts.len());
            parts.push(Part::Run {
                offset: 0,
                len: num,
            });
            plain_only = false;
        } else {
            let Some(phys) = fs.phys(disk) else {
                all_ok = false;
                break;
            };
            let overlaps = {
                let i = allocated.partition_point(|x| x.end() <= disk);
                allocated.get(i).is_some_and(|x| x.offset < disk + disk_len)
            };
            any_alloc |= overlaps;
            if comp == 0 {
                parts.push(Part::Run {
                    offset: phys + offset,
                    len: num,
                });
            } else {
                let codec = match comp {
                    1 => PackedCodec::Zlib,
                    2 => PackedCodec::Zstd,
                    3..=6 => PackedCodec::Lzo,
                    _ => {
                        all_ok = false;
                        break;
                    }
                };
                parts.push(Part::Packed(Packed {
                    offset: phys,
                    disk_len,
                    ram_len: le64(d, 8),
                    skip: offset,
                    len: num,
                    codec,
                }));
                plain_only = false;
            }
        }
        pos = foff + num;
    }
    if pos < e.size {
        all_ok = false;
    }
    if plain_only {
        let ex: Vec<Extent> = parts
            .iter()
            .filter_map(|p| {
                if let Part::Run { offset, len } = p {
                    Some(Extent::new(*offset, *len))
                } else {
                    None
                }
            })
            .collect();
        return Some((DataLoc::Extents { extents: ex }, any_alloc, all_ok));
    }
    // Buracos viram trechos esparsos: Part::Run com offset 0 não serve; usa Bytes de zeros pequenos.
    let mut out: Vec<Part> = vec![];
    for (i, p) in parts.into_iter().enumerate() {
        if sparse_marks.contains(&i) {
            if let Part::Run { len, .. } = p {
                out.push(Part::Zeros { len });
                continue;
            }
        }
        out.push(p);
    }
    Some((DataLoc::Parts { parts: out }, any_alloc, all_ok))
}

fn decode(codec: u8, raw: &[u8], ram: usize) -> Option<Vec<u8>> {
    match codec {
        1 => {
            use std::io::Read;
            let mut v = Vec::with_capacity(ram);
            flate2::read::ZlibDecoder::new(raw)
                .read_to_end(&mut v)
                .ok()?;
            Some(v)
        }
        2 => {
            use std::io::Read;
            let mut dec = ruzstd::StreamingDecoder::new(raw).ok()?;
            let mut v = Vec::with_capacity(ram);
            dec.read_to_end(&mut v).ok()?;
            Some(v)
        }
        _ => lzo_btrfs(raw, ram),
    }
}

/// Formato de extensão LZO do Btrfs: tamanho total (u32) + segmentos [tamanho (u32)][dados LZO1X]
/// de até um setor de 4 KiB descomprimido cada, sem cruzar fronteiras de setor.
pub fn lzo_btrfs(raw: &[u8], ram: usize) -> Option<Vec<u8>> {
    if raw.len() < 4 {
        return None;
    }
    let total = le32(raw, 0) as usize;
    let mut out = Vec::with_capacity(ram);
    let mut p = 4usize;
    while p + 4 <= raw.len().min(total) && out.len() < ram {
        // pula o preenchimento até o próximo setor se sobrar menos de 4 bytes
        let in_sector = 4096 - (p % 4096);
        if in_sector < 4 {
            p += in_sector;
            continue;
        }
        let seg = le32(raw, p) as usize;
        p += 4;
        let chunk = raw.get(p..p + seg)?;
        let want = (ram - out.len()).min(4096);
        out.extend(lzo1x_decompress(chunk, want)?);
        p += seg;
    }
    Some(out)
}

/// LZO1X (descompressão segura, no mesmo esquema de estados do kernel Linux).
pub fn lzo1x_decompress(src: &[u8], max_out: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(max_out);
    let mut ip = 0usize;
    let byte = |i: usize| src.get(i).copied().map(|b| b as usize);
    let lits = |out: &mut Vec<u8>, ip: &mut usize, n: usize| -> Option<()> {
        out.extend_from_slice(src.get(*ip..*ip + n)?);
        *ip += n;
        Some(())
    };
    let ext = |ip: &mut usize, base: usize| -> Option<usize> {
        let mut extra = 0usize;
        while byte(*ip)? == 0 {
            extra += 255;
            *ip += 1;
        }
        let v = base + extra + byte(*ip)?;
        *ip += 1;
        Some(v)
    };
    // estado: literais ao fim da instrução anterior (0..3) ou 4 depois de uma corrida de literais
    let mut state;
    if byte(0)? > 17 {
        let t = byte(0)? - 17;
        ip = 1;
        lits(&mut out, &mut ip, t)?;
        state = if t < 4 { t } else { 4 };
    } else {
        state = 0;
    }
    loop {
        if out.len() > max_out + 4096 {
            return None;
        }
        let t = byte(ip)?;
        ip += 1;
        if t < 16 {
            if state == 0 {
                let n = if t == 0 { ext(&mut ip, 15)? } else { t } + 3;
                lits(&mut out, &mut ip, n)?;
                state = 4;
                continue;
            }
            let h = byte(ip)?;
            ip += 1;
            let (dist, len) = if state == 4 {
                ((t >> 2) + (h << 2) + 2049, 3)
            } else {
                ((t >> 2) + (h << 2) + 1, 2)
            };
            copy_match(&mut out, dist, len)?;
            let next = t & 3;
            lits(&mut out, &mut ip, next)?;
            state = next;
        } else if t >= 64 {
            let h = byte(ip)?;
            ip += 1;
            copy_match(&mut out, 1 + ((t >> 2) & 7) + (h << 3), (t >> 5) + 1)?;
            let next = t & 3;
            lits(&mut out, &mut ip, next)?;
            state = next;
        } else if t >= 32 {
            let len = if t & 31 == 0 {
                ext(&mut ip, 31)?
            } else {
                t & 31
            } + 2;
            let b = (byte(ip)?) | (byte(ip + 1)? << 8);
            ip += 2;
            copy_match(&mut out, 1 + (b >> 2), len)?;
            let next = b & 3;
            lits(&mut out, &mut ip, next)?;
            state = next;
        } else {
            let len = if t & 7 == 0 { ext(&mut ip, 7)? } else { t & 7 } + 2;
            let b = (byte(ip)?) | (byte(ip + 1)? << 8);
            ip += 2;
            let d = ((t & 8) << 11) + (b >> 2);
            if d == 0 {
                return Some(out);
            }
            copy_match(&mut out, d + 0x4000, len)?;
            let next = b & 3;
            lits(&mut out, &mut ip, next)?;
            state = next;
        }
    }
}

fn copy_match(out: &mut Vec<u8>, dist: usize, len: usize) -> Option<()> {
    if dist == 0 || dist > out.len() {
        return None;
    }
    let start = out.len() - dist;
    for i in 0..len {
        let b = out[start + i];
        out.push(b);
    }
    Some(())
}

/// Arquivos e pastas atuais com caminho completo e tamanho (usa só as árvores vivas).
pub fn live_paths(src: &dyn Source, base: u64) -> Vec<(String, u64, bool)> {
    let Some(fs) = Fs::open(src, base) else {
        return vec![];
    };
    let mut roots: Vec<(u64, u64)> = vec![];
    fs.walk(fs.root, &mut |k, d| {
        if k.ty == ROOT_ITEM && d.len() >= 239 && is_fs_tree(k.objectid) {
            roots.push((k.objectid, le64(d, 176)));
        }
    });
    let mut files: Files = HashMap::new();
    for (id, b) in roots {
        fs.walk(b, &mut |k, d| add_item(&mut files, id, u64::MAX, k, d));
    }
    let mut out = vec![];
    for (&(owner, ino), e) in &files {
        if !e.has_inode || ino == 256 {
            continue;
        }
        let mut parts = vec![];
        let mut cur = ino;
        for _ in 0..64 {
            if cur == 256 {
                break;
            }
            match files.get(&(owner, cur)).and_then(|x| x.refs.iter().next()) {
                Some((p, n)) => {
                    parts.push(n.clone());
                    cur = *p;
                }
                None => break,
            }
        }
        parts.reverse();
        out.push((parts.join("/"), e.size, e.mode & 0o170000 == 0o040000));
    }
    out.sort();
    out
}

/// Posições das árvores principais, para ferramentas e testes que precisam editar a imagem.
pub struct TreeLocations {
    pub nodesize: u64,
    /// Endereço físico (absoluto na origem) do primeiro nó de cada árvore.
    pub root_tree: u64,
    pub fs_tree: u64,
    pub extent_tree: u64,
    /// Endereços lógicos correspondentes.
    pub root_tree_logical: u64,
    pub fs_tree_logical: u64,
    pub extent_tree_logical: u64,
    /// Converte endereço lógico em físico.
    pub chunks: Vec<(u64, u64, u64)>,
}

pub fn locate_trees(src: &dyn Source, base: u64) -> Option<TreeLocations> {
    let fs = Fs::open(src, base)?;
    let (mut fs_t, mut ext_t) = (0u64, 0u64);
    fs.walk(fs.root, &mut |k, d| {
        if k.ty == ROOT_ITEM && d.len() >= 239 {
            if k.objectid == 5 {
                fs_t = le64(d, 176);
            } else if k.objectid == 2 {
                ext_t = le64(d, 176);
            }
        }
    });
    Some(TreeLocations {
        nodesize: fs.nodesize,
        root_tree: fs.phys(fs.root)?,
        fs_tree: fs.phys(fs_t)?,
        extent_tree: fs.phys(ext_t)?,
        root_tree_logical: fs.root,
        fs_tree_logical: fs_t,
        extent_tree_logical: ext_t,
        chunks: fs
            .chunks
            .iter()
            .filter_map(|c| c.phys.first().map(|p| (c.logical, c.len, fs.base + p)))
            .collect(),
    })
}
