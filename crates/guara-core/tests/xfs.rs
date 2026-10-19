//! XFS: imagem real do mkfs.xfs + exclusão simulada (inode liberado e entrada de diretório
//! liberada, como o kernel deixa) e a imagem de referência do dfvfs.

mod common;
use common::*;
use guara_core::*;
use std::path::PathBuf;
use std::process::Command;

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

fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}

fn unpack(r: &[u8]) -> (u64, u64, u64) {
    let hi = be64(r, 0);
    let lo = be64(r, 8);
    (
        (hi >> 9) & ((1 << 54) - 1),
        ((hi & 0x1FF) << 43) | (lo >> 21),
        lo & 0x1F_FFFF,
    )
}

#[test]
fn deleted_file_is_recovered_from_free_inode_and_stale_directory_entry() {
    if !(have("mkfs.xfs") && have("xfs_db")) {
        eprintln!("SKIP: xfsprogs ausente");
        return;
    }
    init();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src/docs");
    std::fs::create_dir_all(&src).unwrap();
    let doc = data(5, 150_000);
    std::fs::write(src.join("relatorio.dat"), &doc).unwrap();
    for i in 1..=40 {
        std::fs::write(src.join(format!("a_longa_{i}.txt")), format!("arquivo {i}")).unwrap();
    }
    let mut proto = String::from("/dev/null\n0 0\nd--755 0 0\ndocs d--755 0 0\n");
    let mut names: Vec<_> = std::fs::read_dir(&src)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    for n in &names {
        proto.push_str(&format!("  {n} ---644 0 0 {}\n", src.join(n).display()));
    }
    proto.push_str("  $\n$\n");
    let pf = dir.path().join("proto");
    std::fs::write(&pf, proto).unwrap();
    let img = dir.path().join("x.img");
    std::fs::File::create(&img)
        .unwrap()
        .set_len(320 << 20)
        .unwrap();
    let o = Command::new("mkfs.xfs")
        .args(["-q", "-f", "-p"])
        .arg(&pf)
        .arg(&img)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    let db = |cmds: &[&str]| -> String {
        let mut c = Command::new("xfs_db");
        c.arg("-r");
        for x in cmds {
            c.args(["-c", x]);
        }
        c.arg(&img);
        String::from_utf8_lossy(&c.output().unwrap().stdout).to_string()
    };
    let nc = db(&["blockget -n", "ncheck"]);
    let ino: u64 = nc
        .lines()
        .find(|l| l.ends_with("docs/relatorio.dat"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let conv = db(&[&format!("convert ino {ino} byte")]);
    let ino_off = usize::from_str_radix(
        conv.split("0x")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .trim_end_matches(')'),
        16,
    )
    .unwrap();
    let sbv = db(&["sb 0", "p agblocks agblklog inopblog"]);
    let num = |k: &str| -> u64 {
        sbv.lines()
            .find(|l| l.starts_with(k))
            .unwrap()
            .split('=')
            .nth(1)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    };
    let (agblocks, agblklog, inopblog) = (num("agblocks"), num("agblklog"), num("inopblog"));

    let mut b = std::fs::read(&img).unwrap();
    let bs = 4096usize;
    // extents do arquivo (no inode, formato 3)
    assert_eq!(b[ino_off + 5], 2);
    let nex = be32(&b[ino_off..], 76) as usize;
    let exts: Vec<_> = (0..nex)
        .map(|i| unpack(&b[ino_off + 176 + i * 16..]))
        .collect();
    assert!(exts.iter().map(|e| e.2).sum::<u64>() * 4096 >= 150_000);
    // 1) libera o inode como o kernel: modo 0, links 0, tamanho 0, 0 extents
    b[ino_off + 2..ino_off + 4].fill(0);
    b[ino_off + 16..ino_off + 20].fill(0);
    b[ino_off + 56..ino_off + 64].fill(0);
    b[ino_off + 76..ino_off + 80].fill(0);
    // 2) marca livre na árvore de inodes (AGI -> raiz folha)
    let agno = (ino >> (agblklog + inopblog)) as usize;
    let ag_off = agno * agblocks as usize * bs;
    let agi = &b[ag_off + 2 * 512..ag_off + 2 * 512 + 64];
    let (iroot, ilevel) = (be32(agi, 20) as usize, be32(agi, 24));
    assert_eq!(ilevel, 1);
    let blk = ag_off + iroot * bs;
    let n = be16(&b[blk..], 6) as usize;
    let agino = ino & ((1 << (agblklog + inopblog)) - 1);
    let mut done = false;
    for i in 0..n {
        let r = blk + 56 + i * 16;
        let start = be32(&b[r..], 0) as u64;
        if agino >= start && agino < start + 64 {
            let mask = be64(&b[r..], 8) | (1u64 << (agino - start));
            b[r + 8..r + 16].copy_from_slice(&mask.to_be_bytes());
            let fc = be32(&b[r..], 4) + 1;
            b[r + 4..r + 8].copy_from_slice(&fc.to_be_bytes());
            done = true;
        }
    }
    assert!(done);
    // 3) devolve os blocos à árvore de espaço livre (folha única)
    let agf = &b[ag_off + 512..ag_off + 512 + 64].to_vec();
    let (broot, blevel) = (be32(agf, 16) as usize, be32(agf, 28));
    assert_eq!(blevel, 1);
    let bb = ag_off + broot * bs;
    let nrec = be16(&b[bb..], 6) as usize;
    let mut recs: Vec<(u32, u32)> = (0..nrec)
        .map(|i| {
            (
                be32(&b[bb + 56 + i * 8..], 0),
                be32(&b[bb + 56 + i * 8..], 4),
            )
        })
        .collect();
    for e in &exts {
        recs.push(((e.1 & ((1 << agblklog) - 1)) as u32, e.2 as u32));
    }
    recs.sort();
    let mut merged: Vec<(u32, u32)> = vec![];
    for r in recs {
        match merged.last_mut() {
            Some(l) if l.0 + l.1 == r.0 => l.1 += r.1,
            _ => merged.push(r),
        }
    }
    for (i, r) in merged.iter().enumerate() {
        b[bb + 56 + i * 8..bb + 56 + i * 8 + 4].copy_from_slice(&r.0.to_be_bytes());
        b[bb + 56 + i * 8 + 4..bb + 56 + i * 8 + 8].copy_from_slice(&r.1.to_be_bytes());
    }
    b[bb + 6..bb + 8].copy_from_slice(&(merged.len() as u16).to_be_bytes());
    // 4) entrada de diretório liberada: perde os 4 primeiros bytes do inode (viram marca + tamanho)
    let name = b"relatorio.dat";
    let pos = (0..b.len() - 20)
        .find(|&p| {
            &b[p + 9..p + 9 + name.len()] == name
                && b[p + 8] as usize == name.len()
                && be64(&b[p..], 0) == ino
        })
        .expect("entrada do diretório");
    let size = (8 + 1 + name.len() + 1 + 2usize).div_ceil(8) * 8;
    b[pos..pos + 2].copy_from_slice(&[0xFF, 0xFF]);
    b[pos + 2..pos + 4].copy_from_slice(&(size as u16).to_be_bytes());
    std::fs::write(&img, &b).unwrap();

    let c = scan_path(
        &img,
        &ScanOptions {
            mode: ScanMode::Deep,
            ..Default::default()
        },
    );
    let f = c
        .found
        .iter()
        .find(|f| f.origin == Origin::Xfs)
        .unwrap_or_else(|| {
            panic!(
                "nada do XFS: {:?}",
                c.found
                    .iter()
                    .map(|f| (&f.name, f.origin))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(f.name, "relatorio.dat");
    assert_eq!(f.dir.as_deref(), Some("/docs"));
    assert!(f.confidence >= Confidence::Medium, "{:?}", f.note);
    let out = dir.path().join("out");
    let rep = recover_all(&img, std::slice::from_ref(f), &out);
    assert!(rep.failed.is_empty());
    let rp = PathBuf::from(&rep.recovered[0].path);
    let got = std::fs::read(if rp.is_absolute() { rp } else { out.join(rp) }).unwrap();
    // O tamanho exato se perde na exclusão: o arquivo vem com os blocos inteiros (150000 bytes + preenchimento).
    assert!(got.starts_with(&doc), "conteúdo inicial difere");
    assert_eq!(got.len() % 4096, 0);
    // Só um arquivo apagado.
    assert_eq!(
        c.found.iter().filter(|f| f.origin == Origin::Xfs).count(),
        1
    );
}

#[test]
fn reference_image_is_recognized() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/testdata/dfvfs/xfs.raw");
    if !p.exists() {
        return;
    }
    init();
    let c = scan_path(
        &p,
        &ScanOptions {
            mode: ScanMode::Quick,
            ..Default::default()
        },
    );
    let l = c.layout.unwrap();
    assert_eq!(l.partitions[0].fs, FsKind::Xfs);
}
