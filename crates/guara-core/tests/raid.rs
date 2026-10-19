//! RAID por software: arrays gerados por codificadores independentes (faixa a faixa),
//! lidos de volta inteiros, degradados e com superblocos md 1.x e 0.90.

use guara_core::raid::*;
use guara_core::source::{read_vec, MemSource, Source};
use std::sync::Arc;

const C: usize = 4096;

fn data(len: usize) -> Vec<u8> {
    let mut x = 0x1234_5678_9ABC_DEF1u64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1d;
        }
        b >>= 1;
    }
    r
}

fn gpow(k: usize) -> u8 {
    (0..k).fold(1, |a, _| gmul(a, 2))
}

/// Gera as imagens dos membros de um RAID 5/6 com a paridade na posição pedida.
fn encode_parity(level: Level, layout: Layout, n: usize, logical: &[u8]) -> Vec<Vec<u8>> {
    let dd = if level == Level::R6 { n - 2 } else { n - 1 };
    let rows = logical.len() / (C * dd);
    let mut disks = vec![vec![0u8; rows * C]; n];
    for row in 0..rows {
        let r = row % n;
        let (pd, qd) = match layout {
            Layout::LeftSymmetric | Layout::LeftAsymmetric => {
                let p = n - 1 - r;
                (p, (p + 1) % n)
            }
            _ => (r, (r + 1) % n),
        };
        let sym = matches!(layout, Layout::LeftSymmetric | Layout::RightSymmetric);
        // ordem dos discos de dados
        let order: Vec<usize> = if sym {
            let start = if level == Level::R6 { qd + 1 } else { pd + 1 };
            (0..dd).map(|d| (start + d) % n).collect()
        } else {
            (0..n).filter(|&i| i != pd && !(level == Level::R6 && i == qd)).collect()
        };
        let mut p = vec![0u8; C];
        let mut q = vec![0u8; C];
        for (k, &disk) in order.iter().enumerate() {
            let src = &logical[(row * dd + k) * C..(row * dd + k + 1) * C];
            disks[disk][row * C..(row + 1) * C].copy_from_slice(src);
            for i in 0..C {
                p[i] ^= src[i];
                q[i] ^= gmul(gpow(k), src[i]);
            }
        }
        disks[pd][row * C..(row + 1) * C].copy_from_slice(&p);
        if level == Level::R6 {
            disks[qd][row * C..(row + 1) * C].copy_from_slice(&q);
        }
    }
    disks
}

fn members(imgs: &[Vec<u8>], missing: &[usize]) -> Vec<Option<Member>> {
    imgs.iter()
        .enumerate()
        .map(|(i, v)| {
            (!missing.contains(&i)).then(|| Member { src: Arc::new(MemSource(v.clone())), data_offset: 0 })
        })
        .collect()
}

fn check(level: Level, layout: Layout, n: usize, missing: &[usize]) {
    let dd = if level == Level::R6 { n - 2 } else { n - 1 };
    let logical = data(C * dd * 23);
    let imgs = encode_parity(level, layout, n, &logical);
    let r = RaidSource::new(RaidParams::simple(level, C as u64, layout, members(&imgs, missing))).unwrap();
    assert_eq!(r.size(), logical.len() as u64);
    let got = read_vec(&r, 0, logical.len()).unwrap();
    assert!(got == logical, "{level:?} {layout:?} n={n} faltando {missing:?}");
    // leitura desalinhada
    let part = read_vec(&r, 5000, 30000).unwrap();
    assert_eq!(part, &logical[5000..35000]);
}

#[test]
fn raid5_all_layouts_intact_and_degraded() {
    for layout in Layout::all_raid5() {
        for n in [3, 4, 5] {
            check(Level::R5, layout, n, &[]);
            for m in 0..n {
                check(Level::R5, layout, n, &[m]);
            }
        }
    }
}

#[test]
fn raid6_layouts_with_one_and_two_missing() {
    for layout in Layout::all_raid5() {
        for n in [4, 5, 6] {
            check(Level::R6, layout, n, &[]);
            for a in 0..n {
                check(Level::R6, layout, n, &[a]);
                for b in a + 1..n {
                    check(Level::R6, layout, n, &[a, b]);
                }
            }
        }
    }
}

#[test]
fn raid0_raid1_linear_and_raid10_near() {
    let logical = data(C * 24);
    // RAID 0 com 3 discos
    let mut imgs = vec![vec![]; 3];
    for (c, ch) in logical.chunks(C).enumerate() {
        imgs[c % 3].extend_from_slice(ch);
    }
    let r = RaidSource::new(RaidParams::simple(Level::R0, C as u64, Layout::LeftSymmetric, members(&imgs, &[]))).unwrap();
    assert_eq!(read_vec(&r, 0, logical.len()).unwrap(), logical);
    // RAID 1, um membro caído
    let m = vec![logical.clone(), logical.clone()];
    let r = RaidSource::new(RaidParams::simple(Level::R1, C as u64, Layout::LeftSymmetric, members(&m, &[0]))).unwrap();
    assert_eq!(read_vec(&r, 0, logical.len()).unwrap(), logical);
    // Linear
    let (a, b) = logical.split_at(C * 10);
    let r = RaidSource::new(RaidParams::simple(Level::Linear, C as u64, Layout::LeftSymmetric, members(&[a.to_vec(), b.to_vec()], &[]))).unwrap();
    assert_eq!(read_vec(&r, 0, logical.len()).unwrap(), logical);
    // RAID 10 near=2 com 4 discos: slots em sequência.
    let n = 4;
    let mut imgs = vec![vec![0u8; C * 12]; n];
    for (c, ch) in logical.chunks(C).enumerate() {
        for j in 0..2 {
            let slot = c * 2 + j;
            let (disk, row) = (slot % n, slot / n);
            imgs[disk][row * C..(row + 1) * C].copy_from_slice(ch);
        }
    }
    let mut p = RaidParams::simple(Level::R10, C as u64, Layout::LeftSymmetric, members(&imgs, &[1]));
    p.near = 2;
    p.far = 1;
    let r = RaidSource::new(p).unwrap();
    assert_eq!(r.size(), (C * 24) as u64);
    assert_eq!(read_vec(&r, 0, logical.len()).unwrap(), logical);
}

fn md_v1_super(uuid: u8, level: u32, layout: u32, raid_disks: u32, dev_number: u32, role: u16, data_off_sectors: u64, data_sectors: u64, events: u64) -> Vec<u8> {
    let mut b = vec![0u8; 4096];
    b[0..4].copy_from_slice(&0xA92B_4EFCu32.to_le_bytes());
    b[4..8].copy_from_slice(&1u32.to_le_bytes());
    b[16..32].fill(uuid);
    b[32..36].copy_from_slice(b"test");
    b[72..76].copy_from_slice(&level.to_le_bytes());
    b[76..80].copy_from_slice(&layout.to_le_bytes());
    b[80..88].copy_from_slice(&data_sectors.to_le_bytes());
    b[88..92].copy_from_slice(&((C / 512) as u32).to_le_bytes());
    b[92..96].copy_from_slice(&raid_disks.to_le_bytes());
    b[128..136].copy_from_slice(&data_off_sectors.to_le_bytes());
    b[136..144].copy_from_slice(&data_sectors.to_le_bytes());
    b[160..164].copy_from_slice(&dev_number.to_le_bytes());
    b[200..208].copy_from_slice(&events.to_le_bytes());
    b[220..224].copy_from_slice(&raid_disks.to_le_bytes());
    for i in 0..raid_disks as usize {
        b[256 + i * 2..258 + i * 2].copy_from_slice(&(i as u16).to_le_bytes());
    }
    let _ = role;
    b
}

#[test]
fn assembles_from_md_v12_superblocks_with_a_missing_disk() {
    let n = 4;
    let logical = data(C * 3 * 40);
    let imgs = encode_parity(Level::R5, Layout::LeftSymmetric, n, &logical);
    let data_off = 2048u64; // 1 MiB em setores
    let mut members_in = vec![];
    for (i, im) in imgs.iter().enumerate() {
        if i == 2 {
            continue; // disco perdido
        }
        let mut full = vec![0u8; 4096 + (data_off as usize) * 512 + im.len()];
        let sb = md_v1_super(7, 5, 2, n as u32, i as u32, i as u16, data_off, (im.len() / 512) as u64, 100);
        full[4096..8192].copy_from_slice(&sb);
        full[4096 + data_off as usize * 512..][..im.len()].copy_from_slice(im);
        // o superbloco 1.2 fica a 4 KiB; os dados começam em data_offset a partir do início
        let mut real = vec![0u8; data_off as usize * 512 + im.len()];
        real[4096..8192].copy_from_slice(&sb);
        real[data_off as usize * 512..].copy_from_slice(im);
        let src: Arc<dyn Source> = Arc::new(MemSource(real));
        let sup = read_md_super(src.as_ref()).expect("superbloco");
        assert_eq!(sup.version, "1.2");
        assert_eq!(sup.role, Some(i as u32));
        members_in.push((src, sup));
    }
    let arrays = assemble_md(members_in);
    assert_eq!(arrays.len(), 1);
    let a = arrays.into_iter().next().unwrap().unwrap();
    assert_eq!(a.missing, 1);
    assert_eq!(a.info.level, Level::R5);
    assert_eq!(a.source.size(), logical.len() as u64);
    assert!(read_vec(a.source.as_ref(), 0, logical.len()).unwrap() == logical);
}
