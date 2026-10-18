//! Recuperação de ponta a ponta sobre imagens reais (FAT32, NTFS, ext4).

mod common;
use common::*;
use guara_core::*;

fn check(fs: &str, only_carvable: bool) {
    let Some(dir) = fixtures() else { return };
    let img = dir.join(format!("{fs}.img"));
    let before = sha256_file(&img);
    let res = scan_path(&img, &ScanOptions::default());
    let out = tempfile::tempdir().unwrap();
    let rep = recover_all(&img, &res.found, out.path());
    assert!(rep.failed.is_empty(), "falhas: {:?}", rep.failed);
    let got = hashes_in(out.path());
    for (hash, path) in expected(&dir, fs) {
        let carvable = [".jpg", ".png", ".pdf", ".docx", ".db"]
            .iter()
            .any(|e| path.ends_with(e));
        if only_carvable && !carvable {
            continue;
        }
        assert!(
            got.contains_key(&hash),
            "[{fs}] não recuperado bit a bit: {path}"
        );
    }
    // Os dois métodos juntos não podem listar o mesmo conteúdo duas vezes.
    let mut seen = std::collections::HashSet::new();
    for f in &res.found {
        if let Some(o) = f.first_offset() {
            assert!(seen.insert(o), "[{fs}] achado duplicado no offset {o}");
        }
    }
    assert_eq!(before, sha256_file(&img), "a origem foi modificada!");
}

#[test]
fn fat32_recupera_apagados_com_nome_e_pasta() {
    check("fat32", false);
    let Some(dir) = fixtures() else { return };
    let res = scan_path(&dir.join("fat32.img"), &ScanOptions::default());
    assert!(res
        .found
        .iter()
        .any(|f| f.dir.as_deref() == Some("Documentos/Trabalho") && f.name == "relatorio.pdf"));
    assert!(
        res.found.iter().all(|f| f.name != "mantida.jpg"),
        "arquivo existente não deve ser listado"
    );
}

#[test]
fn ntfs_recupera_apagados() {
    check("ntfs", false);
}

#[test]
fn ext4_recupera_por_assinatura() {
    check("ext4", true);
}

#[test]
fn varredura_rapida_nao_faz_carving() {
    let Some(dir) = fixtures() else { return };
    let res = scan_path(
        &dir.join("ext4.img"),
        &ScanOptions {
            mode: ScanMode::Quick,
            ..Default::default()
        },
    );
    assert!(res.found.is_empty());
    let res = scan_path(
        &dir.join("ntfs.img"),
        &ScanOptions {
            mode: ScanMode::Quick,
            ..Default::default()
        },
    );
    assert_eq!(
        res.found
            .iter()
            .filter(|f| f.origin == Origin::Ntfs)
            .count(),
        7
    );
}

/// Regressão: mkfs.vfat -F 32 em mídia pequena (< 65525 clusters) é FAT32 pelo BPB.
#[test]
fn fat32_pequeno_com_poucos_clusters_e_reconhecido() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("p.img");
    let ok = Command::new("truncate")
        .args(["-s", "32M"])
        .arg(&img)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        && Command::new("mkfs.vfat")
            .args(["-F", "32"])
            .arg(&img)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    let file = dir.path().join("arquivo.txt");
    std::fs::write(&file, b"conteudo de teste\n".repeat(500)).unwrap();
    let ok = ok
        && Command::new("mcopy")
            .arg("-i")
            .arg(&img)
            .arg(&file)
            .arg("::/")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        && Command::new("mdel")
            .arg("-i")
            .arg(&img)
            .arg("::/arquivo.txt")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    if !ok {
        eprintln!("SKIP: mtools/dosfstools ausentes");
        return;
    }
    let res = scan_path(
        &img,
        &ScanOptions {
            mode: ScanMode::Quick,
            ..Default::default()
        },
    );
    let layout = res.layout.unwrap();
    assert_eq!(layout.partitions[0].fs, FsKind::Fat32);
    assert_eq!(res.found.len(), 1);
    let out = tempfile::tempdir().unwrap();
    recover_all(&img, &res.found, out.path());
    assert!(hashes_in(out.path()).contains_key(&sha256(&b"conteudo de teste\n".repeat(500))));
}
