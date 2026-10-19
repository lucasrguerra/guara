use guara_core::defrag::*;
use guara_core::source::MemSource;
use guara_core::types::Extent;

fn photo(w: u16, h: u16, seed: u64, shift: i32) -> Vec<u8> {
    photo_n(w, h, seed, shift, 31)
}

fn photo_n(w: u16, h: u16, seed: u64, shift: i32, nmask: u64) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    let mut x = seed | 1;
    for yy in 0..h as i32 {
        for xx in 0..w as i32 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let noise = ((x >> 40) & nmask) as i32;
            let fx = xx as f32 / w as f32;
            let fy = yy as f32 / h as f32;
            let wave = ((fx * 9.0 + seed as f32 * 0.37).sin() * (fy * 7.0 + 1.3).cos() * 40.0) as i32 + ((fx * 31.0).sin() * 8.0) as i32;
            let base = 110 + shift + wave + (xx * 40 / w as i32);
            let blob = if ((xx - 300).pow(2) + (yy - 200).pow(2)) < 5000 { 60 } else { 0 };
            let v = (base + blob + noise).clamp(0, 255) as u8;
            rgb.extend_from_slice(&[v, v.saturating_sub(10), (v as i32 / 2 + noise) as u8]);
        }
    }
    let mut out = vec![];
    jpeg_encoder::Encoder::new(&mut out, 92).encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb).unwrap();
    out
}

fn noise(n: usize, seed: u64) -> Vec<u8> {
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

fn pad(v: &mut Vec<u8>) {
    v.resize(v.len().div_ceil(4096) * 4096, 0);
}

#[test]
fn rebuilds_a_jpeg_split_around_another_one() {
    let a = photo(1024, 768, 7, 0);
    let b = photo(800, 600, 99, 40);
    let split = 14 * 4096;
    assert!(a.len() > split + 3 * 4096, "A pequena: {}", a.len());
    let mut disk: Vec<u8> = vec![0u8; 64 * 4096];
    let a_off = disk.len() as u64;
    disk.extend_from_slice(&a[..split]);
    let b_off = disk.len() as u64;
    let mut bb = b.clone();
    pad(&mut bb);
    disk.extend_from_slice(&bb);
    disk.extend(noise(20 * 4096, 5)); // lixo
    let a2_off = disk.len() as u64;
    let mut rest = a[split..].to_vec();
    pad(&mut rest);
    disk.extend_from_slice(&rest);
    disk.extend(noise(10 * 4096, 6));
    let src = MemSource(disk.clone());

    let mut idx = ClusterIndex::new(disk.len() as u64);
    idx.scan_chunk(0, &disk);
    // O fragmento correto precisa estar marcado.
    assert!(idx.get(a2_off));
    eprintln!("TRUE a2_off={a2_off}");
    let pool = Pool { index: &idx, used: &[Extent::new(b_off, bb.len() as u64)] };
    match analyze_jpeg(&src, a_off, 4 << 20, &pool, true) {
        Analysis::Rebuilt { extents, size } => {
            assert_eq!(size as usize, a.len(), "tamanho");
            assert_eq!(extents.len(), 2);
            assert_eq!(extents[0], Extent::new(a_off, split as u64));
            assert_eq!(extents[1].offset, a2_off);
            let mut got = disk[a_off as usize..a_off as usize + split].to_vec();
            got.extend_from_slice(&disk[a2_off as usize..a2_off as usize + (size as usize - split)]);
            assert!(got == a, "conteúdo recomposto difere");
        }
        other => panic!("esperava Rebuilt, veio {other:?}"),
    }
    // O outro JPEG, íntegro, continua íntegro.
    let pool2 = Pool { index: &idx, used: &[] };
    match analyze_jpeg(&src, b_off, 4 << 20, &pool2, true) {
        Analysis::Intact { len } => assert_eq!(len as usize, b.len()),
        other => panic!("B deveria estar íntegro: {other:?}"),
    }
}

#[test]
fn rebuilds_three_fragments_among_many_decoys() {
    let a = photo_n(2048, 1536, 21, 10, 3);
    let n = a.len();
    let (s1, s2) = (10 * 4096, 10 * 4096 + (n - 10 * 4096) / 2 / 4096 * 4096);
    assert!(n > s2 + 4 * 4096);
    let mut disk: Vec<u8> = vec![0u8; 32 * 4096];
    let a_off = disk.len() as u64;
    disk.extend_from_slice(&a[..s1]);
    let mut decoys = vec![];
    for k in 0..6u64 {
        let mut d = photo_n(1600, 1200, 100 + k, (k as i32) * 17, 3);
        pad(&mut d);
        decoys.push((disk.len() as u64, d.len() as u64));
        disk.extend_from_slice(&d);
        if k == 1 {
            // segundo fragmento de A no meio
            let off2 = disk.len() as u64;
            disk.extend_from_slice(&a[s1..s2]);
            decoys.push((off2, (s2 - s1) as u64));
        }
        if k == 4 {
            let mut r = a[s2..].to_vec();
            pad(&mut r);
            disk.extend_from_slice(&r);
        }
    }
    let src = MemSource(disk.clone());
    let mut idx = ClusterIndex::new(disk.len() as u64);
    idx.scan_chunk(0, &disk);
    eprintln!("TRUE frag2={:?}", decoys[2]);
    let used: Vec<Extent> = decoys
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 2) // o fragmento do meio de A não é de outro arquivo
        .map(|(_, (o, l))| Extent::new(*o, *l))
        .collect();
    let pool = Pool { index: &idx, used: &used };
    match analyze_jpeg(&src, a_off, 4 << 20, &pool, true) {
        Analysis::Rebuilt { extents, size } => {
            assert_eq!(size as usize, n);
            let mut got = vec![];
            for e in &extents {
                got.extend_from_slice(&disk[e.offset as usize..(e.offset + e.len) as usize]);
            }
            assert!(got == a, "conteúdo difere ({} fragmentos)", extents.len());
        }
        other => panic!("{other:?}"),
    }
}

mod common;

#[test]
fn deep_scan_recovers_fragmented_jpeg_end_to_end() {
    use guara_core::{recover, ScanMode, ScanOptions};
    common::init();
    let a = photo_n(2048, 1536, 5, 0, 3);
    let n = a.len();
    let split = 12 * 4096;
    let mut disk: Vec<u8> = vec![0u8; 16 * 4096];
    let a_off = disk.len();
    disk.extend_from_slice(&a[..split]);
    let mut b = photo_n(1600, 1200, 77, 30, 3);
    pad(&mut b);
    let b_len = b.len();
    disk.extend_from_slice(&b);
    disk.extend(noise(8 * 4096, 3));
    let mut rest = a[split..].to_vec();
    pad(&mut rest);
    disk.extend_from_slice(&rest);
    disk.extend(noise(8 * 4096, 4));
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("d.img");
    std::fs::write(&img, &disk).unwrap();
    let c = common::scan_path(&img, &ScanOptions { mode: ScanMode::Deep, ..Default::default() });
    let jpgs: Vec<_> = c.found.iter().filter(|f| f.ext == "jpg").collect();
    assert_eq!(jpgs.len(), 2, "{:?}", jpgs.iter().map(|f| (f.size, f.first_offset(), f.confidence)).collect::<Vec<_>>());
    let fa = jpgs.iter().find(|f| f.first_offset() == Some(a_off as u64)).unwrap();
    assert_eq!(fa.size as usize, n, "{:?}", fa.note);
    let out = dir.path().join("out");
    let rep = common::recover_all(&img, std::slice::from_ref(*fa), &out);
    let p = std::path::PathBuf::from(&rep.recovered[0].path);
    let got = std::fs::read(if p.is_absolute() { p } else { out.join(p) }).unwrap();
    assert!(got == a, "JPEG recuperado difere do original");
    let fb = jpgs.iter().find(|f| f.first_offset() != Some(a_off as u64)).unwrap();
    assert_eq!(fb.size as usize, photo_n(1600, 1200, 77, 30, 3).len());
    let _ = (b_len, recover::RecoverReport::default());
}
