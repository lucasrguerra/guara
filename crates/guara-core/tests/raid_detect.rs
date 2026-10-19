//! Descoberta automática dos parâmetros de um RAID sem metadados.

use guara_core::raid::Level;
use guara_core::raid_detect;
use guara_core::source::{MemSource, Source};
use std::process::Command;
use std::sync::Arc;

fn have(t: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {t}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ext4_image(mib: u64) -> Option<Vec<u8>> {
    if !have("mke2fs") {
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    for d in 0..12 {
        std::fs::create_dir_all(root.join(format!("pasta{d}/sub"))).unwrap();
        for f in 0..40 {
            std::fs::write(
                root.join(format!("pasta{d}/arq{f}.txt")),
                vec![b'a' + (f % 26) as u8; 300 + f * 37],
            )
            .unwrap();
        }
    }
    let img = dir.path().join("e.img");
    std::fs::File::create(&img)
        .unwrap()
        .set_len(mib << 20)
        .unwrap();
    assert!(Command::new("mke2fs")
        .args(["-q", "-F", "-t", "ext4", "-b", "1024", "-d"])
        .arg(&root)
        .arg(&img)
        .status()
        .unwrap()
        .success());
    Some(std::fs::read(img).unwrap())
}

#[test]
fn finds_raid5_chunk_layout_and_shuffled_order() {
    let Some(logical) = ext4_image(24) else {
        return;
    };
    let (n, chunk) = (3usize, 32768usize);
    let dd = n - 1;
    let rows = logical.len() / (chunk * dd);
    let logical = &logical[..rows * chunk * dd];
    // right-asymmetric
    let mut disks = vec![vec![0u8; rows * chunk]; n];
    for row in 0..rows {
        let pd = row % n;
        let order: Vec<usize> = (0..n).filter(|&i| i != pd).collect();
        let mut p = vec![0u8; chunk];
        for (k, &disk) in order.iter().enumerate() {
            let s = &logical[(row * dd + k) * chunk..(row * dd + k + 1) * chunk];
            disks[disk][row * chunk..(row + 1) * chunk].copy_from_slice(s);
            for i in 0..chunk {
                p[i] ^= s[i];
            }
        }
        disks[pd][row * chunk..(row + 1) * chunk].copy_from_slice(&p);
    }
    // Entrega os discos embaralhados: fornecidos = [2, 0, 1] do array real.
    let given: Vec<Arc<dyn Source>> = [2usize, 0, 1]
        .iter()
        .map(|&i| Arc::new(MemSource(disks[i].clone())) as Arc<dyn Source>)
        .collect();
    let found = raid_detect::detect(&given, &|| false);
    assert!(!found.is_empty());
    for c in found.iter().take(6) {
        eprintln!(
            "CAND {:?} {} {:?} {:?} {:.2}",
            c.level, c.chunk, c.layout, c.order, c.score
        );
    }
    let best = &found[0];
    assert_eq!(best.level, Level::R5);
    assert_eq!(best.chunk, chunk as u64);
    // Layouts e ordens diferentes podem ser equivalentes; o que importa é o conteúdo montado.
    let r = guara_core::raid::RaidSource::new(best.params(&given)).unwrap();
    let got = guara_core::source::read_vec(&r, 0, logical.len()).unwrap();
    assert!(
        got == logical,
        "montagem escolhida ({:?}, {:?}, {:?}) não reproduz o volume",
        best.layout,
        best.order,
        best.level
    );
}

#[test]
fn finds_raid0() {
    let Some(logical) = ext4_image(24) else {
        return;
    };
    let (n, chunk) = (2usize, 65536usize);
    let rows = logical.len() / (chunk * n);
    let mut disks = vec![vec![]; n];
    for c in 0..rows * n {
        disks[c % n].extend_from_slice(&logical[c * chunk..(c + 1) * chunk]);
    }
    let given: Vec<Arc<dyn Source>> = [1usize, 0]
        .iter()
        .map(|&i| Arc::new(MemSource(disks[i].clone())) as Arc<dyn Source>)
        .collect();
    let found = raid_detect::detect(&given, &|| false);
    let best = &found[0];
    assert_eq!(best.level, Level::R0);
    assert_eq!(best.chunk, chunk as u64);
    assert_eq!(best.order, vec![1, 0]);
}
