//! Detectores estruturais: arquivos reais do sistema com lixo colado no fim (o tamanho tem de ser exato).

use guara_core::signatures::detect;
use guara_core::source::MemSource;
use guara_core::Confidence;
use std::path::Path;

fn check(path: &str, ext: &str, min_conf: Confidence) -> bool {
    let p = Path::new(path);
    if !p.exists() {
        eprintln!("SKIP {path}");
        return false;
    }
    let mut data = std::fs::read(p).unwrap();
    let real = data.len();
    // lixo depois do fim real (outro arquivo no disco)
    data.extend((0..5000u32).map(|i| (i.wrapping_mul(2654435761) >> 7) as u8 | 0x81));
    let src = MemSource(data.clone());
    let h = detect(&src, 0, &data[..128], 1 << 30).unwrap_or_else(|| panic!("{path}: não reconhecido"));
    assert_eq!(h.len as usize, real, "{path}: tamanho");
    assert_eq!(h.ext, ext, "{path}: extensão");
    assert!(h.confidence >= min_conf, "{path}: confiança {:?}", h.confidence);
    true
}

fn first(dir: &str, ext: &str) -> Option<String> {
    let out = std::process::Command::new("find").args([dir, "-iname", &format!("*.{ext}"), "-type", "f", "-size", "+2k", "-size", "-20M"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next().map(String::from)
}

#[test]
fn real_files_have_exact_lengths() {
    let mut n = 0;
    let cases: Vec<(Option<String>, &str, Confidence)> = vec![
        (first("/usr/share/cinnamon/sounds", "ogg"), "ogg", Confidence::High),
        (first("/usr/share", "gz"), "gz", Confidence::High),
        (first("/usr/share/doc", "bz2"), "bz2", Confidence::High),
        (first("/usr/lib/firmware", "zst"), "zst", Confidence::Medium),
        (first("/usr/share/texlive/texmf-dist/tex/latex/media9/players", "swf"), "swf", Confidence::Medium),
        (first("/usr/share/code/resources/app", "rtf"), "rtf", Confidence::High),
        (first("/usr/share/app-install/icons", "tiff"), "tif", Confidence::Medium),
        (Some("/usr/bin/ls".to_string()), "elf", Confidence::High),
        (first("/home/lucas/Documents/Desenvolvimento/pop-hub/public", "ico"), "ico", Confidence::High),
    ];
    for (path, ext, conf) in cases {
        if let Some(p) = path {
            if check(&p, ext, conf) {
                n += 1;
            }
        }
    }
    eprintln!("{n} arquivos reais verificados");
}

#[test]
fn webm_rar_deb_with_unknown_locations() {
    for (dir, ext, out, conf) in [
        ("/media/lucas/d9d2ba04-f925-4b9a-97d7-92c2e3bfdb78/Nextcloud/apps/firstrunwizard/img", "webm", "webm", Confidence::High),
        ("/home/lucas/Documents/Pessoal/dev/power_vision/.trabalho/libbsd", "deb", "deb", Confidence::Medium),
        ("/media/lucas/d9d2ba04-f925-4b9a-97d7-92c2e3bfdb78/war_cloud/lucasrguerra/Backups", "rar", "rar", Confidence::High),
    ] {
        if let Some(p) = first(dir, ext) {
            check(&p, out, conf);
        }
    }
}

#[test]
fn synthetic_structures() {
    // CAB, MIDI, pcap e AIFF montados à mão.
    let mut cab = b"MSCF\0\0\0\0".to_vec();
    cab.extend_from_slice(&200u32.to_le_bytes());
    cab.resize(200, 0);
    let mut d = cab.clone();
    d.extend_from_slice(&[0x99; 3000]);
    let h = detect(&MemSource(d.clone()), 0, &d[..64], 1 << 30).unwrap();
    assert_eq!((h.len, h.ext), (200, "cab"));

    let mut mid = b"MThd\0\0\0\x06\0\x01\0\x02\x01\xE0".to_vec();
    for n in [10u32, 7] {
        mid.extend_from_slice(b"MTrk");
        mid.extend_from_slice(&n.to_be_bytes());
        mid.extend(std::iter::repeat(0x41).take(n as usize));
    }
    let real = mid.len();
    let mut d = mid.clone();
    d.extend_from_slice(&[0x77; 500]);
    let h = detect(&MemSource(d.clone()), 0, &d[..64], 1 << 30).unwrap();
    assert_eq!((h.len as usize, h.ext), (real, "mid"));

    let mut pcap = vec![0xD4, 0xC3, 0xB2, 0xA1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    pcap.extend_from_slice(&65535u32.to_le_bytes());
    pcap.extend_from_slice(&1u32.to_le_bytes());
    for len in [60u32, 100, 1514] {
        pcap.extend_from_slice(&[0; 8]);
        pcap.extend_from_slice(&len.to_le_bytes());
        pcap.extend_from_slice(&len.to_le_bytes());
        pcap.extend(std::iter::repeat(0x33).take(len as usize));
    }
    let real = pcap.len();
    let mut d = pcap.clone();
    d.extend_from_slice(&[0xFF; 400]);
    let h = detect(&MemSource(d.clone()), 0, &d[..64], 1 << 30).unwrap();
    assert_eq!((h.len as usize, h.ext), (real, "pcap"));
}
