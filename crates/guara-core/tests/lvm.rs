//! LVM2: imagem real do conjunto de testes do dfvfs (target/testdata/dfvfs/lvm.raw).

use guara_core::lvm;
use guara_core::partition::detect_fs;
use guara_core::source::{FileSource, Source};
use std::sync::Arc;

#[test]
fn assembles_real_lvm_image() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/testdata/dfvfs/lvm.raw");
    if !p.exists() {
        eprintln!("SKIP: execute scripts/fetch-testdata.sh");
        return;
    }
    let src: Arc<dyn Source> = Arc::new(FileSource::open(&p).unwrap());
    let pv = lvm::read_pv(src.as_ref()).expect("PV");
    assert_eq!(pv.vg_name, "test_volume_group");
    let vgs = lvm::assemble_vgs(vec![(src, pv)]);
    assert_eq!(vgs.len(), 1);
    let vg = &vgs[0];
    assert!(vg.warnings.is_empty(), "{:?}", vg.warnings);
    let names: Vec<&str> = vg.lvs.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["test_logical_volume1", "test_logical_volume2"]);
    for (n, lv) in &vg.lvs {
        assert_eq!(lv.size(), 4 << 20, "{n}");
        let fs = detect_fs(lv.as_ref(), 0);
        eprintln!("{n}: {fs:?}");
        // O segundo volume da imagem de referência foi criado e nunca formatado.
        assert_eq!(fs.is_some(), n == "test_logical_volume1", "{n}");
    }
}
