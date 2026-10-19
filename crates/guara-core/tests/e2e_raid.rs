//! Ponta a ponta: RAID 5 de 4 discos (um perdido) com ext4 dentro e um arquivo apagado.

mod common;
use common::*;
use guara_core::assemble::{assemble_source, Assembly};
use guara_core::scan::{self, Outcome};
use guara_core::source::{FileSource, Source};
use guara_core::*;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

fn have(t: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {t}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn data(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

const CHUNK: usize = 64 * 1024;

fn md_super(n: u32, dev: u32, data_off: u64, sectors: u64) -> Vec<u8> {
    let mut b = vec![0u8; 4096];
    b[0..4].copy_from_slice(&0xA92B_4EFCu32.to_le_bytes());
    b[4..8].copy_from_slice(&1u32.to_le_bytes());
    b[16..32].fill(0x5a);
    b[72..76].copy_from_slice(&5u32.to_le_bytes());
    b[76..80].copy_from_slice(&2u32.to_le_bytes());
    b[80..88].copy_from_slice(&sectors.to_le_bytes());
    b[88..92].copy_from_slice(&((CHUNK / 512) as u32).to_le_bytes());
    b[92..96].copy_from_slice(&n.to_le_bytes());
    b[128..136].copy_from_slice(&data_off.to_le_bytes());
    b[136..144].copy_from_slice(&sectors.to_le_bytes());
    b[160..164].copy_from_slice(&dev.to_le_bytes());
    b[200..208].copy_from_slice(&50u64.to_le_bytes());
    b[220..224].copy_from_slice(&n.to_le_bytes());
    for i in 0..n as usize {
        b[256 + i * 2..258 + i * 2].copy_from_slice(&(i as u16).to_le_bytes());
    }
    b
}

#[test]
fn deleted_file_on_degraded_raid5() {
    if !(have("mke2fs") && have("debugfs")) {
        eprintln!("SKIP: e2fsprogs ausente");
        return;
    }
    init();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("fotos")).unwrap();
    let doc = data(5, 300_000);
    std::fs::write(root.join("fotos/ferias.dat"), &doc).unwrap();
    std::fs::write(root.join("fotos/outra.dat"), data(6, 9000)).unwrap();
    let plain = dir.path().join("plain.img");
    let n = 4usize;
    let dd = n - 1;
    let rows = 40usize;
    let logical_len = rows * dd * CHUNK;
    std::fs::File::create(&plain)
        .unwrap()
        .set_len(logical_len as u64)
        .unwrap();
    assert!(Command::new("mke2fs")
        .args(["-q", "-F", "-t", "ext4", "-d"])
        .arg(&root)
        .arg(&plain)
        .status()
        .unwrap()
        .success());
    Command::new("debugfs")
        .arg("-w")
        .arg("-R")
        .arg("rm /fotos/ferias.dat")
        .arg(&plain)
        .output()
        .unwrap();
    let logical = std::fs::read(&plain).unwrap();

    // RAID 5 left-symmetric
    let data_off = 2048u64; // setores
    let mut disks = vec![vec![0u8; data_off as usize * 512 + rows * CHUNK]; n];
    for row in 0..rows {
        let pd = n - 1 - row % n;
        let mut p = vec![0u8; CHUNK];
        for d in 0..dd {
            let disk = (pd + 1 + d) % n;
            let src = &logical[(row * dd + d) * CHUNK..(row * dd + d + 1) * CHUNK];
            disks[disk][data_off as usize * 512 + row * CHUNK..][..CHUNK].copy_from_slice(src);
            for i in 0..CHUNK {
                p[i] ^= src[i];
            }
        }
        disks[pd][data_off as usize * 512 + row * CHUNK..][..CHUNK].copy_from_slice(&p);
    }
    let mut paths = vec![];
    for (i, d) in disks.iter_mut().enumerate() {
        if i == 2 {
            continue; // disco que morreu
        }
        let sb = md_super(n as u32, i as u32, data_off, (rows * CHUNK / 512) as u64);
        d[4096..8192].copy_from_slice(&sb);
        let p = dir.path().join(format!("disk{i}.img"));
        std::fs::write(&p, &d).unwrap();
        paths.push(p);
    }
    let base: Arc<dyn Source> = Arc::new(FileSource::open(&paths[0]).unwrap());
    let asm = Assembly {
        members: paths[1..]
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        ..Default::default()
    };
    let a = assemble_source(base, &asm).unwrap();
    assert!(
        a.notes.iter().any(|n| n.contains("degradado")),
        "{:?}",
        a.notes
    );
    assert_eq!(a.extra_parts.len(), 1);
    assert_eq!(a.extra_parts[0].fs, FsKind::Ext);
    let opts = ScanOptions {
        mode: ScanMode::Deep,
        extra_parts: a.extra_parts.clone(),
        covered: a.covered.clone(),
        ..Default::default()
    };
    let mut sink = Collect::default();
    let out = scan::run(a.source.as_ref(), &opts, None, &[], &|| false, &mut sink);
    assert!(matches!(out, Outcome::Completed));
    let f = sink
        .found
        .iter()
        .find(|f| f.name == "ferias.dat")
        .unwrap_or_else(|| {
            panic!(
                "{:?}",
                sink.found.iter().map(|f| &f.name).collect::<Vec<_>>()
            )
        });
    assert_eq!(f.size, 300_000);
    let dest = tempfile::tempdir().unwrap();
    let rep = recover::recover(
        a.source.as_ref(),
        &[f],
        dest.path(),
        &|| false,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert!(rep.failed.is_empty(), "{:?}", rep.failed);
    let p = &rep.recovered[0].path;
    let p = if Path::new(p).is_absolute() {
        Path::new(p).to_path_buf()
    } else {
        dest.path().join(p)
    };
    assert!(std::fs::read(p).unwrap() == doc);
}

#[test]
fn raid5_without_metadata_is_detected_and_scanned() {
    if !(have("mke2fs") && have("debugfs")) {
        return;
    }
    init();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    for d in 0..10 {
        std::fs::create_dir_all(root.join(format!("d{d}"))).unwrap();
        for f in 0..30 {
            std::fs::write(
                root.join(format!("d{d}/f{f}.txt")),
                vec![b'x'; 500 + f * 50],
            )
            .unwrap();
        }
    }
    let doc = data(9, 200_000);
    std::fs::write(root.join("d3/perdido.dat"), &doc).unwrap();
    let plain = dir.path().join("plain.img");
    let (n, chunk) = (3usize, 64 * 1024usize);
    let dd = n - 1;
    let rows = 160usize;
    std::fs::File::create(&plain)
        .unwrap()
        .set_len((rows * dd * chunk) as u64)
        .unwrap();
    assert!(Command::new("mke2fs")
        .args(["-q", "-F", "-t", "ext4", "-d"])
        .arg(&root)
        .arg(&plain)
        .status()
        .unwrap()
        .success());
    Command::new("debugfs")
        .arg("-w")
        .arg("-R")
        .arg("rm /d3/perdido.dat")
        .arg(&plain)
        .output()
        .unwrap();
    let logical = std::fs::read(&plain).unwrap();
    // left-asymmetric
    let mut disks = vec![vec![0u8; rows * chunk]; n];
    for row in 0..rows {
        let pd = n - 1 - row % n;
        let mut p = vec![0u8; chunk];
        for (k, disk) in (0..n).filter(|&i| i != pd).enumerate() {
            let s = &logical[(row * dd + k) * chunk..(row * dd + k + 1) * chunk];
            disks[disk][row * chunk..(row + 1) * chunk].copy_from_slice(s);
            for i in 0..chunk {
                p[i] ^= s[i];
            }
        }
        disks[pd][row * chunk..(row + 1) * chunk].copy_from_slice(&p);
    }
    let order = [1usize, 2, 0]; // fornecidos fora de ordem
    let mut paths = vec![];
    for (j, &i) in order.iter().enumerate() {
        let p = dir.path().join(format!("m{j}.img"));
        std::fs::write(&p, &disks[i]).unwrap();
        paths.push(p);
    }
    let base: Arc<dyn Source> = Arc::new(FileSource::open(&paths[0]).unwrap());
    let asm = Assembly {
        members: paths[1..]
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        raid: Some(guara_core::assemble::RaidSpec::default()),
        ..Default::default()
    };
    let a = assemble_source(base, &asm).unwrap();
    assert!(a.skip_primary);
    assert!(
        a.notes.iter().any(|n| n.contains("descobertos")),
        "{:?}",
        a.notes
    );
    let opts = ScanOptions {
        mode: ScanMode::Deep,
        extra_parts: a.extra_parts.clone(),
        skip_primary: true,
        ..Default::default()
    };
    let mut sink = Collect::default();
    let out = scan::run(a.source.as_ref(), &opts, None, &[], &|| false, &mut sink);
    assert!(matches!(out, Outcome::Completed));
    let f = sink
        .found
        .iter()
        .find(|f| f.name == "perdido.dat")
        .expect("perdido.dat");
    let dest = tempfile::tempdir().unwrap();
    let rep = recover::recover(
        a.source.as_ref(),
        &[f],
        dest.path(),
        &|| false,
        &mut |_, _, _| {},
    )
    .unwrap();
    let p = &rep.recovered[0].path;
    let p = if Path::new(p).is_absolute() {
        Path::new(p).to_path_buf()
    } else {
        dest.path().join(p)
    };
    assert!(std::fs::read(p).unwrap() == doc);
}
