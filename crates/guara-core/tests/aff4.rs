//! AFF4: imagens de referência da Evimetry (github.com/aff4/ReferenceImages) e ida e volta.

use guara_core::aff4::{write_aff4, Aff4Options, Aff4Source};
use guara_core::source::{open_path, read_vec, MemSource, Source};
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/testdata/aff4")
}

#[test]
fn reads_reference_images_and_stored_hash_matches() {
    let mut seen = 0;
    for name in [
        "Base-Linear.aff4",
        "Base-Allocated.aff4",
        "Base-Linear-AllHashes.aff4",
    ] {
        let p = dir().join(name);
        if !p.exists() {
            eprintln!("SKIP {name}");
            continue;
        }
        let a = Aff4Source::open(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(a.size(), 268_435_456, "{name}");
        assert_eq!(
            a.verify_stream_md5().unwrap(),
            Some(true),
            "{name}: hash do fluxo"
        );
        // setor 0 legível pelo caminho genérico
        let s = open_path(&p).unwrap();
        let head = read_vec(s.as_ref(), 0, 4096).unwrap();
        assert_eq!(head.len(), 4096);
        // leitura em vãos do mapa devolve zeros
        let far = read_vec(s.as_ref(), 200 << 20, 512).unwrap();
        assert_eq!(far.len(), 512);
        eprintln!(
            "{name}: {} {:?}",
            a.info.get("Examinador").cloned().unwrap_or_default(),
            guara_core::partition::detect_fs_in(&read_vec(s.as_ref(), 0, 4096).unwrap())
                .map(|x| x.0)
        );
        seen += 1;
    }
    if seen == 0 {
        eprintln!("SKIP: execute scripts/fetch-testdata.sh");
    }
}

#[test]
fn roundtrip_with_snappy_blocks_and_odd_size() {
    let mut x = 77u64;
    let mut data: Vec<u8> = (0..5_000_001u32)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            if i % 100_000 < 60_000 {
                0
            } else {
                (x >> 24) as u8
            }
        })
        .collect();
    data.truncate(5_000_001);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.aff4");
    let opts = Aff4Options {
        case_name: "caso".into(),
        examiner: "Lucas".into(),
        notes: "teste".into(),
        ..Default::default()
    };
    let rep = write_aff4(
        &MemSource(data.clone()),
        &p,
        &opts,
        &|| false,
        &mut |_, _| {},
    )
    .unwrap();
    let a = Aff4Source::open(&p).unwrap();
    assert_eq!(a.size(), data.len() as u64);
    assert_eq!(
        a.info.get("md5").map(String::as_str),
        Some(rep.md5.as_str())
    );
    assert_eq!(a.info.get("Examinador").map(String::as_str), Some("Lucas"));
    assert_eq!(a.verify_stream_md5().unwrap(), Some(true));
    let got = read_vec(&a, 0, data.len()).unwrap();
    assert!(got == data);
    let part = read_vec(&a, 123_456, 70_000).unwrap();
    assert_eq!(part, &data[123_456..193_456]);
}
