//! Reconstrução do moov de MP4 H.264: arquivos sintéticos e, se existir, um vídeo real.

use guara_core::mp4rebuild;
use guara_core::source::MemSource;

struct BitW {
    v: Vec<u8>,
    n: usize,
}
impl BitW {
    fn new() -> Self {
        Self { v: vec![], n: 0 }
    }
    fn bit(&mut self, b: u32) {
        if self.n % 8 == 0 {
            self.v.push(0);
        }
        if b != 0 {
            *self.v.last_mut().unwrap() |= 0x80 >> (self.n % 8);
        }
        self.n += 1;
    }
    fn u(&mut self, val: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.bit((val >> i) & 1);
        }
    }
    fn ue(&mut self, v: u32) {
        let x = v + 1;
        let len = 32 - x.leading_zeros();
        for _ in 0..len - 1 {
            self.bit(0);
        }
        self.u(x, len);
    }
}

const SPS: &str = "67f4000d90d9b28283f602d41818190000030001000003003c8f142996";
const PPS: &str = "68ebec448440";

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

/// Cabeçalho de fatia H.264 conforme o SPS acima (log2_max_frame_num e log2_max_poc_lsb lidos do SPS).
fn slice(
    idr: bool,
    frame_num: u32,
    poc_lsb: u32,
    payload: usize,
    seed: u8,
    sps: &mguara::Sps,
) -> Vec<u8> {
    let mut b = BitW::new();
    b.ue(0); // first_mb_in_slice
    b.ue(if idr { 7 } else { 5 }); // slice_type
    b.ue(0); // pps
    b.u(frame_num, sps.log2_max_frame_num);
    if idr {
        b.ue(0);
    }
    b.u(poc_lsb, sps.log2_max_poc_lsb);
    b.u(0x55, 8);
    let mut nal = vec![if idr { 0x65 } else { 0x41 }];
    nal.extend_from_slice(&b.v);
    nal.extend((0..payload).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed) | 0x10));
    nal
}

mod mguara {
    pub use guara_core::mp4rebuild::Sps264 as Sps;
}

fn nal(out: &mut Vec<u8>, n: &[u8]) {
    out.extend_from_slice(&(n.len() as u32).to_be_bytes());
    out.extend_from_slice(n);
}

/// Monta um MP4 sem moov: ftyp, free, mdat. Devolve (arquivo, tamanhos esperados, sincronos, ordem de exibição).
fn synthetic(gops: usize) -> (Vec<u8>, Vec<usize>, Vec<bool>) {
    synthetic_opt(gops, true)
}

fn synthetic_opt(gops: usize, inband: bool) -> (Vec<u8>, Vec<usize>, Vec<bool>) {
    let sps_nal = hex(SPS);
    let sps = guara_core::mp4rebuild::parse_sps264(&sps_nal).expect("SPS");
    assert_eq!((sps.width, sps.height), (320, 240));
    let mut mdat = vec![];
    let mut sizes = vec![];
    let mut sync = vec![];
    // GOP: I P B B P B B  (ordem de decodificação) com POC de exibição 0 6 2 4 12 8 10 (em unidades de 2)
    let pocs = [0u32, 6, 2, 4, 12, 8, 10];
    let mut frame_num = 0;
    for g in 0..gops {
        for (i, &p) in pocs.iter().enumerate() {
            let start = mdat.len();
            nal(&mut mdat, &[0x09, 0x10]);
            if i == 0 && inband {
                nal(&mut mdat, &sps_nal);
                nal(&mut mdat, &hex(PPS));
            }
            if i == 0 {
                frame_num = 0;
            }
            let s = slice(
                i == 0,
                frame_num % 16,
                p % 64,
                600 + i * 90 + g * 7,
                (g * 7 + i) as u8,
                &sps,
            );
            nal(&mut mdat, &s);
            if p % 4 == 0 || i == 1 || i == 4 {
                frame_num += 1;
            }
            sizes.push(mdat.len() - start);
            sync.push(i == 0);
        }
    }
    let mut f = vec![];
    f.extend_from_slice(&[
        0, 0, 0, 24, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 2, 0, b'i', b's', b'o',
        b'm', b'a', b'v', b'c', b'1',
    ]);
    f.extend_from_slice(&[0, 0, 0, 8, b'f', b'r', b'e', b'e']);
    f.extend_from_slice(&((8 + mdat.len()) as u32).to_be_bytes());
    f.extend_from_slice(b"mdat");
    f.extend_from_slice(&mdat);
    (f, sizes, sync)
}

fn assemble(file: &[u8], r: &mp4rebuild::Rebuilt) -> Vec<u8> {
    let mut out = file[..r.pre_len as usize].to_vec();
    out.extend_from_slice(&r.mdat_header());
    out.extend_from_slice(&file[r.payload.0 as usize..r.payload.1 as usize]);
    out.extend_from_slice(&r.moov);
    out
}

#[test]
fn rebuilds_synthetic_stream_with_b_frames_and_garbage_tail() {
    let (mut file, sizes, sync) = synthetic(6);
    let real_len = file.len();
    // Lixo depois do fim real (outro arquivo no disco) e tamanho do mdat declarado como "até o fim".
    file.extend((0..20_000u32).map(|i| (i.wrapping_mul(2654435761) >> 11) as u8 | 0x80));
    let n = file.len() as u32 - 32;
    file[32..36].copy_from_slice(&n.to_be_bytes());
    let src = MemSource(file.clone());
    let r = mp4rebuild::rebuild(&src, 0, file.len() as u64, &[]).expect("reconstrução");
    assert_eq!(r.frames, sizes.len());
    assert_eq!((r.width, r.height), (320, 240));
    assert_eq!(
        r.payload.1 as usize, real_len,
        "o fim do vídeo foi detectado"
    );
    let out = assemble(&file, &r);
    let size = out.len() as u64;
    let mut mp4 =
        mp4::Mp4Reader::read_header(std::io::Cursor::new(out), size).expect("moov válido");
    let track_id = *mp4.tracks().keys().next().unwrap();
    let t = &mp4.tracks()[&track_id];
    assert_eq!(t.sample_count() as usize, sizes.len());
    for i in 0..sizes.len() {
        let s = mp4.read_sample(track_id, i as u32 + 1).unwrap().unwrap();
        assert_eq!(s.bytes.len(), sizes[i], "tamanho da amostra {}", i + 1);
        assert_eq!(s.is_sync, sync[i], "sincronia da amostra {}", i + 1);
    }
    // Ordem de exibição: pts = dts + deslocamento deve crescer na ordem do POC.
    let mut pts: Vec<(i64, usize)> = vec![];
    for i in 0..sizes.len() {
        let s = mp4.read_sample(track_id, i as u32 + 1).unwrap().unwrap();
        pts.push((s.start_time as i64 + s.rendering_offset as i64, i));
    }
    let pocs = [0u32, 6, 2, 4, 12, 8, 10];
    for g in 0..6 {
        let mut order: Vec<usize> = (0..7).collect();
        order.sort_by_key(|&k| pts[g * 7 + k].0);
        let mut expect: Vec<usize> = (0..7).collect();
        expect.sort_by_key(|&k| pocs[k]);
        assert_eq!(order, expect, "ordem de exibição do GOP {g}");
    }
}

#[test]
fn rebuilds_real_h264_file_like_the_original_index() {
    let Ok(path) = std::env::var("GUARA_MP4_SAMPLE") else {
        eprintln!(
            "SKIP: defina GUARA_MP4_SAMPLE com um MP4 H.264 para comparar com o índice original"
        );
        return;
    };
    let orig = std::fs::read(&path).unwrap();
    // Corta o moov original.
    let mut p = 0usize;
    let mut cut = orig.len();
    while p + 8 <= orig.len() {
        let sz = u32::from_be_bytes(orig[p..p + 4].try_into().unwrap()) as usize;
        if &orig[p + 4..p + 8] == b"moov" {
            cut = p;
            break;
        }
        p += sz;
    }
    let file = orig[..cut].to_vec();
    let src = MemSource(file.clone());
    let r = mp4rebuild::rebuild(&src, 0, file.len() as u64, &[]).expect("reconstrução");
    let out = assemble(&file, &r);
    let osize = orig.len() as u64;
    let mut a = mp4::Mp4Reader::read_header(std::io::Cursor::new(orig), osize).unwrap();
    let rsize = out.len() as u64;
    let mut b = mp4::Mp4Reader::read_header(std::io::Cursor::new(out), rsize).unwrap();
    let ta = *a.tracks().keys().next().unwrap();
    let tb = *b.tracks().keys().next().unwrap();
    assert_eq!(
        a.tracks()[&ta].sample_count(),
        b.tracks()[&tb].sample_count()
    );
    for i in 1..=a.tracks()[&ta].sample_count() {
        let x = a.read_sample(ta, i).unwrap().unwrap();
        let y = b.read_sample(tb, i).unwrap().unwrap();
        assert_eq!(x.bytes, y.bytes, "amostra {i}");
        assert_eq!(x.is_sync, y.is_sync, "sincronia {i}");
    }
    eprintln!("{} quadros, {:.2} fps", r.frames, r.fps);
}

#[test]
fn rebuilds_real_video_with_interleaved_audio() {
    // GUARA_MP4_BIG: vídeo real com áudio; o teste corta o moov e os primeiros 40 MB do mdat bastam.
    let Ok(path) = std::env::var("GUARA_MP4_BIG") else {
        return;
    };
    use std::io::Read;
    let mut f = std::fs::File::open(&path).unwrap();
    let total = f.metadata().unwrap().len();
    let mut head = vec![0u8; 1 << 20];
    f.read_exact(&mut head).unwrap();
    // átomos no começo
    let mut p = 0usize;
    let (mut ftyp_end, mut mdat_at) = (0usize, None);
    while p + 8 <= head.len() {
        let sz = u32::from_be_bytes(head[p..p + 4].try_into().unwrap()) as usize;
        match &head[p + 4..p + 8] {
            b"ftyp" => ftyp_end = p + sz,
            b"mdat" => {
                mdat_at = Some(p);
                break;
            }
            _ => {}
        }
        if sz == 0 {
            break;
        }
        p += sz;
    }
    let Some(mp) = mdat_at else {
        eprintln!("SKIP: mdat fora do primeiro MiB (moov no começo é grande)");
        return;
    };
    // Reexecuta com a leitura completa do começo para montar: ftyp + mdat (40 MB).
    use std::io::Seek;
    let mut f = std::fs::File::open(&path).unwrap();
    f.seek(std::io::SeekFrom::Start(mp as u64)).unwrap();
    let mut mdat = vec![0u8; 40 << 20];
    f.read_exact(&mut mdat).unwrap();
    let mut file = head[..ftyp_end].to_vec();
    file.extend_from_slice(&mdat);
    // O tamanho do mdat declarado vai muito além dos 40 MB que temos (arquivo cortado).
    let src = MemSource(file.clone());
    // Sem SPS/PPS no mdat, é preciso um vídeo de referência da mesma câmera (aqui, o próprio original).
    let cfg = mp4rebuild::config_from_mp4(&MemSource(head.clone()), 0, head.len() as u64)
        .expect("avcC do original");
    assert!(
        mp4rebuild::rebuild(&src, 0, file.len() as u64, &[]).is_none(),
        "sem referência não há como montar o avcC"
    );
    let r = mp4rebuild::rebuild(&src, 0, file.len() as u64, &[cfg]).expect("reconstrução");
    let out = assemble(&file, &r);
    let osize = out.len() as u64;
    let mut b = mp4::Mp4Reader::read_header(std::io::Cursor::new(out), osize).unwrap();
    let tb = *b.tracks().keys().next().unwrap();
    // Original (índice completo) para comparar as primeiras amostras.
    let f = std::fs::File::open(&path).unwrap();
    let mut a = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), total).unwrap();
    let vid = a
        .tracks()
        .values()
        .find(|t| {
            t.track_type()
                .map(|x| x == mp4::TrackType::Video)
                .unwrap_or(false)
        })
        .unwrap()
        .track_id();
    eprintln!(
        "reconstruído: {} quadros, {} bytes pulados (áudio), {}x{}",
        r.frames, r.skipped_bytes, r.width, r.height
    );
    let sizes_a: Vec<usize> = (1..=60)
        .map(|i| a.read_sample(vid, i).unwrap().unwrap().bytes.len())
        .collect();
    let sizes_b: Vec<usize> = (1..=60)
        .map(|i| b.read_sample(tb, i).unwrap().unwrap().bytes.len())
        .collect();
    eprintln!("ORIG {:?}\nMINE {:?}", &sizes_a[40..60], &sizes_b[40..60]);
    let check = (r.frames as u32 - 1).min(400);
    for i in 1..=check {
        let x = a.read_sample(vid, i).unwrap().unwrap();
        let y = b.read_sample(tb, i).unwrap().unwrap();
        assert_eq!(x.bytes.len(), y.bytes.len(), "tamanho da amostra {i}");
        assert_eq!(x.is_sync, y.is_sync, "sincronia {i}");
    }
}

mod common;

#[test]
fn deep_scan_rebuilds_a_video_without_moov_using_a_neighbour_as_reference() {
    use guara_core::{ScanMode, ScanOptions};
    common::init();
    // Arquivo íntegro (com moov) da "mesma câmera" e um segundo que perdeu o moov e não traz SPS/PPS.
    let (good_raw, _, _) = synthetic_opt(8, true);
    let r =
        mp4rebuild::rebuild(&MemSource(good_raw.clone()), 0, good_raw.len() as u64, &[]).unwrap();
    let good = assemble(&good_raw, &r);
    let (broken, sizes, _) = synthetic_opt(10, false);
    let mut disk = vec![0u8; 8192];
    let good_off = disk.len();
    disk.extend_from_slice(&good);
    disk.resize(disk.len().div_ceil(4096) * 4096 + 8192, 0);
    let broken_off = disk.len();
    disk.extend_from_slice(&broken);
    disk.extend((0..30_000u32).map(|i| (i.wrapping_mul(2654435761) >> 9) as u8 | 0x80));
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("v.img");
    std::fs::write(&img, &disk).unwrap();
    let c = common::scan_path(
        &img,
        &ScanOptions {
            mode: ScanMode::Deep,
            ..Default::default()
        },
    );
    let vids: Vec<_> = c.found.iter().filter(|f| f.ext == "mp4").collect();
    assert_eq!(
        vids.len(),
        2,
        "{:?}",
        vids.iter()
            .map(|f| (f.first_offset(), f.size, f.confidence, &f.note))
            .collect::<Vec<_>>()
    );
    let fb = vids
        .iter()
        .find(|f| matches!(f.data, guara_core::DataLoc::Parts { .. }))
        .expect("vídeo quebrado");
    let _ = broken_off;
    assert!(
        fb.note.as_deref().unwrap_or("").contains("reconstruído"),
        "{:?}",
        fb.note
    );
    let _ = good_off;
    let out = dir.path().join("out");
    let rep = common::recover_all(&img, std::slice::from_ref(*fb), &out);
    assert!(rep.failed.is_empty());
    let p = std::path::PathBuf::from(&rep.recovered[0].path);
    let bytes = std::fs::read(if p.is_absolute() { p } else { out.join(p) }).unwrap();
    let len = bytes.len() as u64;
    let mut m = mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), len).expect("MP4 válido");
    let tid = *m.tracks().keys().next().unwrap();
    assert_eq!(m.tracks()[&tid].sample_count() as usize, sizes.len());
    let first = m.read_sample(tid, 1).unwrap().unwrap();
    assert_eq!(first.bytes.len(), sizes[0]);
}
