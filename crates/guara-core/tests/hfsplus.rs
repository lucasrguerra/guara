//! HFS+: imagem real do dfvfs (target/testdata/dfvfs/hfsplus.raw) e exclusão simulada de registros.

mod common;
use common::*;
use guara_core::*;

fn image() -> Option<Vec<u8>> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/testdata/dfvfs/hfsplus.raw");
    p.exists().then(|| std::fs::read(p).unwrap())
}

fn scan_bytes(bytes: &[u8]) -> Collect {
    init();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("h.img");
    std::fs::write(&p, bytes).unwrap();
    scan_path(&p, &ScanOptions { mode: ScanMode::Quick, ..Default::default() })
}

#[test]
fn pristine_volume_has_no_deleted_files_and_is_recognized() {
    let Some(img) = image() else {
        eprintln!("SKIP: execute scripts/fetch-testdata.sh");
        return;
    };
    let c = scan_bytes(&img);
    assert_eq!(c.layout.as_ref().unwrap().partitions[0].fs, FsKind::HfsPlus);
    assert!(c.found.is_empty(), "{:?}", c.found.iter().map(|f| &f.name).collect::<Vec<_>>());
}

#[test]
fn record_left_in_leaf_slack_is_recovered() {
    let Some(mut img) = image() else { return };
    let bs = 4096usize;
    let node = 186 * bs + bs; // catálogo começa no bloco 186; nó 1 é a única folha
    // Remove os 5 últimos registros como o driver faz ao apagar no fim do nó: só diminui a contagem.
    img[node + 10..node + 12].copy_from_slice(&21u16.to_be_bytes());
    // Libera o bloco de dados (bitmap de alocação no bloco 1).
    let blk = 278usize;
    img[bs + blk / 8] &= !(0x80 >> (blk % 8));
    let c = scan_bytes(&img);
    let f = c.found.iter().find(|f| f.name == "fseventsd-uuid").unwrap_or_else(|| panic!("{:?}", c.found.iter().map(|f| &f.name).collect::<Vec<_>>()));
    assert_eq!(f.origin, Origin::Hfs);
    assert_eq!(f.size, 36);
    assert_eq!(f.dir.as_deref(), Some("/.fseventsd"));
    assert!(f.confidence >= Confidence::Medium);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("h.img");
    std::fs::write(&p, &img).unwrap();
    let out = dir.path().join("out");
    let rep = recover_all(&p, std::slice::from_ref(f), &out);
    assert!(rep.failed.is_empty());
    let rp = std::path::PathBuf::from(&rep.recovered[0].path);
    let got = std::fs::read(if rp.is_absolute() { rp } else { out.join(rp) }).unwrap();
    assert_eq!(got, &img[blk * bs..blk * bs + 36]);
}
