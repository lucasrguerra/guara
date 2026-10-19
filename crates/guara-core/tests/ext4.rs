//! ext4: arquivos apagados com nome, a partir de imagens criadas com mke2fs/debugfs.

mod common;
use common::*;
use guara_core::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn debugfs(img: &Path, cmds: &[&str]) -> String {
    let mut out = String::new();
    for c in cmds {
        let o = Command::new("debugfs")
            .arg("-w")
            .arg("-R")
            .arg(c)
            .arg(img)
            .output()
            .unwrap();
        out.push_str(&String::from_utf8_lossy(&o.stdout));
        out.push_str(&String::from_utf8_lossy(&o.stderr));
    }
    out
}

fn inode_of(img: &Path, path: &str) -> u64 {
    let o = debugfs(img, &[&format!("stat {path}")]);
    let l = o
        .lines()
        .find(|l| l.starts_with("Inode:"))
        .unwrap_or_else(|| panic!("{o}"));
    l.split_whitespace().nth(1).unwrap().parse().unwrap()
}

fn content(seed: u32, len: usize) -> Vec<u8> {
    let mut x = 88172645463325252u64 ^ seed as u64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

struct Img {
    _dir: tempfile::TempDir,
    img: PathBuf,
    files: Vec<(&'static str, Vec<u8>)>,
}

fn build(opts: &[&str]) -> Option<Img> {
    if !have("mke2fs") || !have("debugfs") {
        eprintln!("SKIP: e2fsprogs ausente");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("docs/sub")).unwrap();
    std::fs::create_dir_all(root.join("fotos")).unwrap();
    let files = vec![
        ("docs/relatorio.dat", content(1, 150_000)),
        ("docs/sub/contrato.dat", content(2, 9_000)),
        ("fotos/viagem.dat", content(3, 700_000)),
        ("fotos/fica.dat", content(4, 30_000)),
    ];
    for (p, c) in &files {
        std::fs::write(root.join(p), c).unwrap();
    }
    let img = dir.path().join("e.img");
    let f = std::fs::File::create(&img).unwrap();
    f.set_len(24 << 20).unwrap();
    let st = Command::new("mke2fs")
        .args(["-q", "-F", "-d"])
        .arg(&root)
        .args(opts)
        .arg(&img)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    Some(Img {
        _dir: dir,
        img,
        files,
    })
}

fn deep(img: &Path) -> common::Collect {
    init();
    let opts = ScanOptions {
        mode: ScanMode::Deep,
        ..ScanOptions::default()
    };
    scan_path(img, &opts)
}

fn recovered(img: &Path, f: &Found) -> Vec<u8> {
    let d = tempfile::tempdir().unwrap();
    let rep = recover_all(img, std::slice::from_ref(f), d.path());
    assert!(rep.failed.is_empty(), "{:?}", rep.failed);
    let p = &rep.recovered[0].path;
    let p = if Path::new(p).is_absolute() {
        PathBuf::from(p)
    } else {
        d.path().join(p)
    };
    std::fs::read(p).unwrap()
}

#[test]
fn deleted_by_debugfs_keeps_extents_and_name() {
    let Some(i) = build(&["-t", "ext4"]) else {
        return;
    };
    debugfs(&i.img, &["rm /docs/relatorio.dat", "rm /fotos/viagem.dat"]);
    let c = deep(&i.img);
    for (p, data) in i
        .files
        .iter()
        .filter(|f| f.0 == "docs/relatorio.dat" || f.0 == "fotos/viagem.dat")
    {
        let name = p.rsplit('/').next().unwrap();
        let f = c
            .found
            .iter()
            .find(|f| f.name == name && f.origin == Origin::Ext)
            .unwrap_or_else(|| {
                panic!(
                    "{name} não encontrado: {:?}",
                    c.found
                        .iter()
                        .map(|f| (&f.name, f.origin))
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(f.size, data.len() as u64);
        assert_eq!(
            f.dir.as_deref(),
            Some(&format!("/{}", p.rsplit_once('/').unwrap().0)[..])
        );
        assert!(recovered(&i.img, f) == *data, "{name}");
    }
    // O arquivo que não foi apagado não aparece.
    assert!(!c
        .found
        .iter()
        .any(|f| f.name == "fica.dat" && f.origin == Origin::Ext));
}

#[test]
fn truncated_inode_recovered_by_remnant_extents() {
    // Simula o que o kernel faz ao apagar: tamanho zerado e contagem de extents zerada.
    let Some(i) = build(&["-t", "ext4"]) else {
        return;
    };
    let ino = inode_of(&i.img, "/docs/sub/contrato.dat");
    debugfs(
        &i.img,
        &[
            "rm /docs/sub/contrato.dat",
            &format!("sif <{ino}> size 0"),
            &format!("sif <{ino}> block[0] 0xF30A"),
        ],
    );
    let c = deep(&i.img);
    let f = c
        .found
        .iter()
        .find(|f| f.name == "contrato.dat")
        .expect("contrato.dat");
    assert_eq!(f.origin, Origin::Ext);
    // Tamanho exato é perdido: recupera todos os blocos (9000 bytes arredondados para 3 blocos de 4K).
    assert_eq!(f.size, 12288);
    let got = recovered(&i.img, f);
    assert_eq!(&got[..9000], &i.files[1].1[..]);
}

#[test]
fn journal_copy_restores_exact_size() {
    if !have("debugfs") {
        return;
    }
    let Some(i) = build(&["-t", "ext4", "-O", "has_journal"]) else {
        return;
    };
    // Bloco da tabela de inodes que contém o inode do arquivo.
    let ino = inode_of(&i.img, "/fotos/viagem.dat");
    let o = debugfs(&i.img, &["stats"]);
    let _ = o;
    let blk_line = debugfs(&i.img, &[&format!("imap <{ino}>")]);
    // "Inode 12 is part of block group 0\n\tlocated at block 1058, offset 0x0300"
    let blk: u64 = blk_line
        .split("located at block ")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| panic!("imap: {blk_line}"));
    let bs = 4096u64;
    let raw = std::fs::read(&i.img).unwrap();
    let old = &raw[(blk * bs) as usize..((blk + 1) * bs) as usize];
    let bin = i.img.with_extension("blk");
    std::fs::write(&bin, old).unwrap();
    let script = i.img.with_extension("cmds");
    std::fs::write(
        &script,
        format!("jo\njw -b {blk} {}\njc\njc\n", bin.display()),
    )
    .unwrap();
    let o = Command::new("debugfs")
        .arg("-w")
        .arg("-f")
        .arg(&script)
        .arg(&i.img)
        .output()
        .unwrap();
    let j = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    if j.contains("Unknown request") || j.contains("not supported") {
        eprintln!("SKIP: debugfs sem suporte a escrita no diário: {j}");
        return;
    }
    // Exclui e zera como o kernel faria.
    debugfs(
        &i.img,
        &[
            "rm /fotos/viagem.dat",
            &format!("sif <{ino}> size 0"),
            &format!("sif <{ino}> block[0] 0xF30A"),
        ],
    );
    let c = deep(&i.img);
    let f = c
        .found
        .iter()
        .find(|f| f.name == "viagem.dat")
        .unwrap_or_else(|| {
            panic!(
                "sem viagem.dat: {:?}",
                c.found.iter().map(|f| &f.name).collect::<Vec<_>>()
            )
        });
    assert_eq!(f.size, 700_000, "{:?}", f.note);
    assert!(recovered(&i.img, f) == i.files[2].1);
}

#[test]
fn ext2_blockmap_deleted_is_not_invented() {
    // ext2 zera os ponteiros ao apagar; sem diário não há o que recuperar por metadados,
    // mas a varredura não pode falhar nem listar o arquivo que continua existindo.
    let Some(i) = build(&["-t", "ext2"]) else {
        return;
    };
    let c = deep(&i.img);
    assert!(!c.found.iter().any(|f| f.origin == Origin::Ext));
}
