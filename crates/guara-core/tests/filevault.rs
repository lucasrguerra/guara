//! FileVault 2 (Core Storage): imagem de referência do cryptsetup (senha heslo123).

use guara_core::crypt::{filevault, Secret, UnlockError};
use guara_core::hashing::MultiHasher;
use guara_core::source::{FileSource, Source};
use std::sync::Arc;

#[test]
fn opens_reference_image_and_matches_known_hash() {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/testdata/fvault2-images/small");
    if !p.exists() {
        eprintln!("SKIP: execute scripts/fetch-testdata.sh");
        return;
    }
    let src: Arc<dyn Source> = Arc::new(FileSource::open(&p).unwrap());
    assert!(matches!(
        filevault::unlock(src.clone(), &[Secret::Password("errada".into())]),
        Err(UnlockError::WrongSecret)
    ));
    let u = filevault::unlock(src, &[Secret::Password("heslo123".into())]).unwrap();
    assert_eq!(u.source.size(), 167_772_160);
    assert_eq!(
        guara_core::crypt::to_hex(u.volume_key.as_ref().unwrap()),
        "20734d3389212774d7610c29d732880 9".replace(' ', "")
    );
    let mut h = MultiHasher::new();
    let mut off = 0;
    let mut buf = vec![0u8; 1 << 20];
    while off < u.source.size() {
        let n = u.source.read_at(off, &mut buf).unwrap();
        h.update(&buf[..n]);
        off += n as u64;
    }
    assert_eq!(
        h.finalize().sha256,
        "2c662e36c0f7e2f5583e6a939bbcbdc660805692d0fccaa45ad4052beb3b8e18"
    );
}
