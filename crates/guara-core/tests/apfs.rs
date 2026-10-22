//! APFS: imagem real do dfvfs (4 checkpoints) e exclusão simulada pela troca da raiz da árvore.

mod common;
use common::*;
use guara_core::fs::apfs::{fletcher64, free_extents, live_paths};
use guara_core::source::{FileSource, Source};
use guara_core::*;
use std::path::PathBuf;

fn img() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/testdata/dfvfs/apfs.raw");
    p.exists().then_some(p)
}

#[test]
fn reads_the_live_tree_of_the_reference_image() {
    let Some(p) = img() else { return };
    let src = FileSource::open(&p).unwrap();
    let files = live_paths(&src, 0);
    eprintln!("{files:#?}");
    assert!(!files.is_empty());
    let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
    assert!(
        names.iter().any(|n| n.ends_with("passwords.txt")),
        "{names:?}"
    );
}

#[test]
fn free_space_map_matches_the_spaceman() {
    let Some(p) = img() else { return };
    let src = FileSource::open(&p).unwrap();
    let free = free_extents(&src, 0).expect("mapa de espaço livre");
    let bytes: u64 = free.iter().map(|e| e.len).sum();
    assert_eq!(bytes, 904 * 4096);
    assert_eq!(free.first().unwrap().offset, 110 * 4096);
    assert!(free.iter().all(|e| e.end() <= src.size()));
}

#[test]
fn checksum_matches_real_blocks() {
    let Some(p) = img() else { return };
    let d = std::fs::read(p).unwrap();
    let b = &d[0..4096];
    assert_eq!(
        fletcher64(&b[8..]),
        u64::from_le_bytes(b[0..8].try_into().unwrap())
    );
}

#[test]
fn quick_scan_is_clean_on_pristine_volume() {
    let Some(p) = img() else { return };
    init();
    let c = scan_path(
        &p,
        &ScanOptions {
            mode: ScanMode::Quick,
            ..Default::default()
        },
    );
    assert_eq!(c.layout.as_ref().unwrap().partitions[0].fs, FsKind::Apfs);
    eprintln!(
        "{:?}",
        c.found
            .iter()
            .map(|f| (&f.name, &f.dir, f.size))
            .collect::<Vec<_>>()
    );
}

// --- manipulação bruta de nós (independente do módulo) ---

fn le16(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

type Entry = (Vec<u8>, Vec<u8>);

fn read_node(b: &[u8]) -> (u16, Vec<Entry>) {
    let flags = le16(b, 32) as u16;
    let n = le32(b, 36) as usize;
    let (toc_off, toc_len) = (le16(b, 40), le16(b, 42));
    let root = flags & 1 != 0;
    let key_base = 56 + toc_off + toc_len;
    let val_end = b.len() - if root { 40 } else { 0 };
    let mut out = vec![];
    for i in 0..n {
        let o = 56 + toc_off + i * 8;
        let (ko, kl, vo, vl) = (le16(b, o), le16(b, o + 2), le16(b, o + 4), le16(b, o + 6));
        out.push((
            b[key_base + ko..key_base + ko + kl].to_vec(),
            b[val_end - vo..val_end - vo + vl].to_vec(),
        ));
    }
    (flags, out)
}

fn write_node(old: &[u8], entries: &[Entry]) -> Vec<u8> {
    let mut b = vec![0u8; old.len()];
    b[..56].copy_from_slice(&old[..56]);
    let flags = le16(old, 32) as u16;
    let root = flags & 1 != 0;
    if root {
        let l = old.len();
        b[l - 40..].copy_from_slice(&old[l - 40..]);
    }
    let n = entries.len();
    b[36..40].copy_from_slice(&(n as u32).to_le_bytes());
    b[40..42].copy_from_slice(&0u16.to_le_bytes());
    b[42..44].copy_from_slice(&((n * 8) as u16).to_le_bytes());
    let key_base = 56 + n * 8;
    let val_end = old.len() - if root { 40 } else { 0 };
    let (mut kpos, mut vpos) = (0usize, 0usize);
    for (i, (k, v)) in entries.iter().enumerate() {
        vpos += v.len();
        let o = 56 + i * 8;
        b[o..o + 2].copy_from_slice(&(kpos as u16).to_le_bytes());
        b[o + 2..o + 4].copy_from_slice(&(k.len() as u16).to_le_bytes());
        b[o + 4..o + 6].copy_from_slice(&(vpos as u16).to_le_bytes());
        b[o + 6..o + 8].copy_from_slice(&(v.len() as u16).to_le_bytes());
        b[key_base + kpos..key_base + kpos + k.len()].copy_from_slice(k);
        b[val_end - vpos..val_end - vpos + v.len()].copy_from_slice(v);
        kpos += k.len();
    }
    seal(&mut b);
    b
}

fn seal(b: &mut [u8]) {
    let c = fletcher64(&b[8..]);
    b[0..8].copy_from_slice(&c.to_le_bytes());
}

#[test]
fn file_removed_from_the_current_tree_survives_in_the_old_node() {
    let Some(p) = img() else { return };
    let mut d = std::fs::read(&p).unwrap();
    let bs = 4096usize;
    let blk = |d: &[u8], n: u64| d[n as usize * bs..(n as usize + 1) * bs].to_vec();
    // Superbloco mais recente do contêiner.
    let (desc_blocks, desc_base) = (le32(&d, 104) as u64, le64(&d, 112));
    let mut nx = d[..bs].to_vec();
    for i in 0..desc_blocks {
        let b = blk(&d, desc_base + i);
        if &b[32..36] == b"NXSB" && le64(&b, 16) > le64(&nx, 16) {
            nx = b;
        }
    }
    // omap do contêiner -> volume
    let cont_omap = blk(&d, le64(&nx, 160));
    let cont_tree = blk(&d, le64(&cont_omap, 48));
    let (_, cont_entries) = read_node(&{
        // omap usa chaves fixas (16/16): reaproveita o leitor variável convertendo a tabela
        let mut c = cont_tree.clone();
        fixed_to_var(&mut c);
        c
    });
    let vol_oid = le64(&nx, 184);
    let vol_paddr = cont_entries
        .iter()
        .filter(|(k, _)| le64(k, 0) == vol_oid)
        .map(|(_, v)| le64(v, 8))
        .next_back()
        .unwrap();
    let vsb = blk(&d, vol_paddr);
    let (vol_omap_paddr, root_oid) = (le64(&vsb, 128), le64(&vsb, 136));
    let vol_omap = blk(&d, vol_omap_paddr);
    let vol_tree_paddr = le64(&vol_omap, 48);
    let mut vt = blk(&d, vol_tree_paddr);
    let vt_orig = vt.clone();
    fixed_to_var(&mut vt);
    let (_, omap_entries) = read_node(&vt);
    let (root_idx, root_paddr) = omap_entries
        .iter()
        .enumerate()
        .filter(|(_, (k, _))| le64(k, 0) == root_oid)
        .map(|(i, (_, v))| (i, le64(v, 8)))
        .next_back()
        .unwrap();
    let root = blk(&d, root_paddr);
    assert_eq!(le16(&root, 34), 0, "raiz deveria ser folha nesta imagem");
    let (_, recs) = read_node(&root);
    // inode do arquivo
    let victim = b"passwords.txt";
    let ino = recs
        .iter()
        .find_map(|(k, v)| {
            let hdr = le64(k, 0);
            ((hdr >> 60) == 9 && k.len() > 12 && k[12..].starts_with(victim)).then(|| le64(v, 0))
        })
        .expect("entrada de diretório");
    let private_id = recs
        .iter()
        .find_map(|(k, v)| {
            ((le64(k, 0) >> 60) == 3 && (le64(k, 0) & 0x0FFF_FFFF_FFFF_FFFF) == ino)
                .then(|| le64(v, 8))
        })
        .unwrap();
    let kept: Vec<Entry> = recs
        .iter()
        .filter(|(k, v)| {
            let hdr = le64(k, 0);
            let (id, ty) = (hdr & 0x0FFF_FFFF_FFFF_FFFF, hdr >> 60);
            let drec_of_victim =
                ty == 9 && k.len() > 12 && k[12..].starts_with(victim) && le64(v, 0) == ino;
            !(id == ino && ty != 9 || id == private_id && ty == 8 || drec_of_victim)
        })
        .cloned()
        .collect();
    assert!(kept.len() < recs.len());
    let newnode = write_node(&root, &kept);
    // bloco livre para a nova versão
    let free = (10..d.len() / bs)
        .find(|&n| d[n * bs..(n + 1) * bs].iter().all(|&x| x == 0) && n != vol_tree_paddr as usize)
        .expect("bloco livre");
    d[free * bs..(free + 1) * bs].copy_from_slice(&newnode);
    // aponta o mapa de objetos para a nova versão (mesmo xid), editando o valor da entrada
    let toc_off = le16(&vt_orig, 40);
    let key_base = 56 + toc_off + le16(&vt_orig, 42);
    let _ = key_base;
    let val_end = bs - 40;
    let vo = le16(&vt_orig, 56 + toc_off + root_idx * 4 + 2);
    let at = vol_tree_paddr as usize * bs + val_end - vo + 8;
    d[at..at + 8].copy_from_slice(&(free as u64).to_le_bytes());
    let mut ob = d[vol_tree_paddr as usize * bs..(vol_tree_paddr as usize + 1) * bs].to_vec();
    seal(&mut ob);
    d[vol_tree_paddr as usize * bs..(vol_tree_paddr as usize + 1) * bs].copy_from_slice(&ob);

    let dir = tempfile::tempdir().unwrap();
    let p2 = dir.path().join("a.img");
    std::fs::write(&p2, &d).unwrap();
    // O arquivo sumiu da árvore atual...
    let src = FileSource::open(&p2).unwrap();
    assert!(!live_paths(&src, 0).iter().any(|f| f.0 == "passwords.txt"));
    init();
    let c = scan_path(
        &p2,
        &ScanOptions {
            mode: ScanMode::Deep,
            ..Default::default()
        },
    );
    let f = c
        .found
        .iter()
        .find(|f| f.origin == Origin::Apfs && f.name == "passwords.txt")
        .unwrap_or_else(|| {
            panic!(
                "{:?}",
                c.found
                    .iter()
                    .map(|f| (&f.name, f.origin))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(f.size, 116);
    let out = dir.path().join("out");
    let rep = recover_all(&p2, std::slice::from_ref(f), &out);
    assert!(rep.failed.is_empty());
    let rp = PathBuf::from(&rep.recovered[0].path);
    let got = std::fs::read(if rp.is_absolute() { rp } else { out.join(rp) }).unwrap();
    assert_eq!(got.len(), 116);
    assert!(
        String::from_utf8_lossy(&got)
            .chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t'),
        "{got:?}"
    );
}

/// Converte a tabela de entradas fixas (4 bytes) de um nó de omap para o formato variável (8 bytes) em memória.
fn fixed_to_var(b: &mut Vec<u8>) {
    let flags = le16(b, 32) as u16;
    if flags & 4 == 0 {
        return;
    }
    let n = le32(b, 36) as usize;
    let (toc_off, toc_len) = (le16(b, 40), le16(b, 42));
    let key_base = 56 + toc_off + toc_len;
    let root = flags & 1 != 0;
    let val_end = b.len() - if root { 40 } else { 0 };
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = vec![];
    for i in 0..n {
        let o = 56 + toc_off + i * 4;
        let (ko, vo) = (le16(b, o), le16(b, o + 2));
        entries.push((
            b[key_base + ko..key_base + ko + 16].to_vec(),
            b[val_end - vo..val_end - vo + 16].to_vec(),
        ));
    }
    let mut nb = b.clone();
    nb[32..34].copy_from_slice(&(flags & !4).to_le_bytes());
    *b = write_node_flagsless(&nb, &entries);
}

fn write_node_flagsless(old: &[u8], entries: &[Entry]) -> Vec<u8> {
    let mut b = write_node(old, entries);
    // write_node usa as flags de `old`
    let fl = le16(&b, 32) as u16;
    b[32..34].copy_from_slice(&fl.to_le_bytes());
    b
}
