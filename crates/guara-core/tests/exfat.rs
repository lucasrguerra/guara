//! exFAT: injeta um conjunto de entradas APAGADO num sistema criado por mkfs.exfat.

mod common;
use common::*;
use guara_core::*;
use std::process::Command;

#[test]
fn exfat_apagado_e_recuperado_com_nome() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("e.img");
    let ok = Command::new("truncate")
        .args(["-s", "32M"])
        .arg(&img)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        && Command::new("mkfs.exfat")
            .arg(&img)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    if !ok {
        eprintln!("SKIP: mkfs.exfat ausente");
        return;
    }
    let mut d = std::fs::read(&img).unwrap();
    let ss = 1usize << d[108];
    let cs = ss << d[109];
    let heap = u32::from_le_bytes(d[88..92].try_into().unwrap()) as usize * ss;
    let root = u32::from_le_bytes(d[96..100].try_into().unwrap()) as usize;
    let root_off = heap + (root - 2) * cs;
    // Primeira entrada livre da raiz.
    let mut slot = root_off;
    while d[slot] != 0 {
        slot += 32;
    }
    let content: Vec<u8> = (0..70_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    let first_cluster = 300u32;
    let data_off = heap + (first_cluster as usize - 2) * cs;
    d[data_off..data_off + content.len()].copy_from_slice(&content);
    let name = "relatório final.txt";
    let units: Vec<u16> = name.encode_utf16().collect();
    // 0x05 = arquivo apagado (0x85 sem o bit "em uso"), secundárias 0x40 e 0x41.
    let n_name = units.len().div_ceil(15);
    d[slot] = 0x05;
    d[slot + 1] = (1 + n_name) as u8;
    d[slot + 12..slot + 14].copy_from_slice(&0x6000u16.to_le_bytes()); // hora
    d[slot + 14..slot + 16].copy_from_slice(&((44u16 << 9) | (6 << 5) | 15).to_le_bytes()); // 2024-06-15
    let st = slot + 32;
    d[st] = 0x40;
    d[st + 1] = 0x03; // alocação possível + sem cadeia na FAT
    d[st + 3] = units.len() as u8;
    d[st + 20..st + 24].copy_from_slice(&first_cluster.to_le_bytes());
    d[st + 24..st + 32].copy_from_slice(&(content.len() as u64).to_le_bytes());
    d[st + 8..st + 16].copy_from_slice(&(content.len() as u64).to_le_bytes());
    for k in 0..n_name {
        let e = slot + 64 + k * 32;
        d[e] = 0x41;
        for (j, u) in units.iter().skip(k * 15).take(15).enumerate() {
            d[e + 2 + j * 2..e + 4 + j * 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    std::fs::write(&img, &d).unwrap();

    let res = scan_path(
        &img,
        &ScanOptions {
            mode: ScanMode::Quick,
            ..Default::default()
        },
    );
    assert!(res.warnings.is_empty(), "{:?}", res.warnings);
    assert_eq!(res.found.len(), 1);
    let f = &res.found[0];
    assert_eq!(f.name, "relatório final.txt");
    assert_eq!(f.origin, Origin::Exfat);
    assert_eq!(f.size, content.len() as u64);
    assert!(f.modified.is_some());
    let out = tempfile::tempdir().unwrap();
    recover_all(&img, &res.found, out.path());
    assert!(hashes_in(out.path()).contains_key(&sha256(&content)));
}
