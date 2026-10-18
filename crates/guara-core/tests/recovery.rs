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
