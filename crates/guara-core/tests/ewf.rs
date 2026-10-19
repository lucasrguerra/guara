use guara_core::ewf::{write_e01, E01Options, EwfSource};
use guara_core::source::{open_path, read_vec, Source};
use std::path::PathBuf;

fn fx(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ewf").join(name)
}

fn raw() -> Vec<u8> {
    std::fs::read(fx("small.raw")).unwrap()
}

#[test]
fn reads_references_made_by_libewf() {
    let want = raw();
    for name in ["s_e6.E01", "s_e6n.E01", "s_x7.Ex01", "s_x7e.Ex01"] {
        let s = open_path(&fx(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(s.size(), want.len() as u64, "{name}");
        let got = read_vec(s.as_ref(), 0, want.len()).unwrap();
        assert!(got == want, "{name}: conteúdo difere");
        // leitura que cruza chunks e termina no fim
        let tail = read_vec(s.as_ref(), want.len() as u64 - 5000, 9000).unwrap();
        assert_eq!(tail, &want[want.len() - 5000..], "{name}");
    }
}

#[test]
fn stored_hash_matches() {
    for name in ["s_e6.E01", "s_x7.Ex01"] {
        let e = EwfSource::open(&fx(name)).unwrap();
        assert_eq!(e.verify().unwrap(), Some(true), "{name}");
    }
    let e = EwfSource::open(&fx("s_e6.E01")).unwrap();
    assert_eq!(e.info.version, 1);
    assert_eq!(e.info.fields.get("Examinador").map(String::as_str), Some("examiner"));
    assert_eq!(e.info.fields.get("Número do caso").map(String::as_str), Some("case1"));
    assert_eq!(EwfSource::open(&fx("s_x7.Ex01")).unwrap().info.version, 2);
}

#[test]
fn write_then_read_roundtrip_with_segments() {
    let mut x = 88172645463325252u64;
    let data: Vec<u8> = (0..3_000_320u32)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    let src = guara_core::source::MemSource(data.clone());
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("img");
    let opts = E01Options {
        segment_size: 1_000_000,
        case_number: "caso-1".into(),
        examiner: "Lucas".into(),
        ..Default::default()
    };
    let rep = write_e01(&src, &base, &opts, &|| false, &mut |_, _| {}).unwrap();
    assert!(dir.path().join("img.E02").exists(), "esperava mais de um segmento");
    let r = open_path(&dir.path().join("img.E01")).unwrap();
    assert_eq!(r.size(), data.len() as u64);
    assert!(read_vec(r.as_ref(), 0, data.len()).unwrap() == data);
    let e = EwfSource::open(&dir.path().join("img.E01")).unwrap();
    assert_eq!(e.info.md5.as_deref(), Some(rep.md5.as_str()));
    assert_eq!(e.verify().unwrap(), Some(true));
    assert_eq!(e.info.fields.get("Examinador").map(String::as_str), Some("Lucas"));
}

#[test]
fn compressed_output_is_smaller_and_odd_sizes_work() {
    let data = vec![7u8; 100_352];
    let src = guara_core::source::MemSource(data.clone());
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("z");
    write_e01(&src, &base, &E01Options { segment_size: 0, ..Default::default() }, &|| false, &mut |_, _| {}).unwrap();
    assert!(std::fs::metadata(dir.path().join("z.E01")).unwrap().len() < 10_000);
    let r = open_path(&dir.path().join("z.E01")).unwrap();
    assert_eq!(r.size(), 100_352);
    assert_eq!(read_vec(r.as_ref(), 0, 100_352).unwrap(), data);
}

/// Gera uma imagem para validar com ferramentas externas (ewfverify). `GUARA_E01_OUT=/caminho/base`.
#[test]
#[ignore]
fn emit_for_external_verification() {
    let Ok(base) = std::env::var("GUARA_E01_OUT") else { return };
    let src = guara_core::source::MemSource(raw());
    let opts = E01Options { segment_size: 100_000, case_number: "c1".into(), examiner: "ex".into(), ..Default::default() };
    write_e01(&src, std::path::Path::new(&base), &opts, &|| false, &mut |_, _| {}).unwrap();
}
