//! Discos com tabela de partições, espaço não particionado, partições perdidas e cifragem.

mod common;
use common::*;
use guara_core::*;
use std::io::Write;

const MIB: u64 = 1024 * 1024;

fn mbr(parts: &[(u8, u64, u64)]) -> Vec<u8> {
    let mut s = vec![0u8; 512];
    for (i, (ty, start, len)) in parts.iter().enumerate() {
        let e = &mut s[446 + i * 16..462 + i * 16];
        e[4] = *ty;
        e[8..12].copy_from_slice(&((start / 512) as u32).to_le_bytes());
        e[12..16].copy_from_slice(&((len / 512) as u32).to_le_bytes());
    }
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

fn png(extra: usize) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let chunk = |ty: &[u8; 4], data: &[u8]| {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(ty);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0, 0, 0, 0]);
        c
    };
    v.extend(chunk(b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 2, 0, 0, 0]));
    v.extend(chunk(
        b"IDAT",
        &(0..extra).map(|i| (i * 31 % 251) as u8).collect::<Vec<_>>(),
    ));
    v.extend(chunk(b"IEND", &[]));
    v
}

/// Disco: [1 MiB vazio][partição FAT32 do fixture][8 MiB não particionado com um PNG]
#[test]
fn particao_mbr_e_espaco_nao_particionado() {
    let Some(dir) = fixtures() else { return };
    let fat = std::fs::read(dir.join("fat32.img")).unwrap();
    let plen = fat.len() as u64;
    let gap_start = MIB + plen;
    let mut disk = vec![0u8; (gap_start + 8 * MIB) as usize];
    disk[..512].copy_from_slice(&mbr(&[(0x0C, MIB, plen)]));
    disk[MIB as usize..(MIB + plen) as usize].copy_from_slice(&fat);
    let lost_png = png(40_000);
    let at = (gap_start + 3 * MIB) as usize;
    disk[at..at + lost_png.len()].copy_from_slice(&lost_png);
    let path = tempfile::NamedTempFile::new().unwrap();
    path.as_file().write_all(&disk).unwrap();

    let res = scan_path(path.path(), &ScanOptions::default());
    let layout = res.layout.unwrap();
    assert_eq!(layout.table, "mbr");
    assert_eq!(layout.partitions.len(), 1);
    assert_eq!(layout.partitions[0].fs, FsKind::Fat32);
    assert_eq!(layout.partitions[0].start, MIB);
    assert!(
        layout.gaps.iter().any(|g| g.start == gap_start),
        "gap: {:?}",
        layout.gaps
    );
    // Metadados da partição (offsets corrigidos pela posição da partição)...
    let fat_found: Vec<_> = res
        .found
        .iter()
        .filter(|f| f.origin == Origin::Fat)
        .collect();
    assert_eq!(fat_found.len(), 7);
    // ... e o PNG no espaço não particionado.
    let carved: Vec<_> = res
        .found
        .iter()
        .filter(|f| f.origin == Origin::Carved && f.ext == "png")
        .collect();
    assert_eq!(carved.len(), 1);
    assert_eq!(carved[0].first_offset(), Some(gap_start + 3 * MIB));
    let out = tempfile::tempdir().unwrap();
    recover_all(path.path(), &res.found, out.path());
    assert!(hashes_in(out.path()).contains_key(&sha256(&lost_png)));
    for (h, p) in expected(&dir, "fat32") {
        assert!(hashes_in(out.path()).contains_key(&h), "faltou {p}");
    }
}

#[test]
fn particao_perdida_e_encontrada_sem_tabela() {
    let Some(dir) = fixtures() else { return };
    let ntfs = std::fs::read(dir.join("ntfs.img")).unwrap();
    let off = 5 * MIB;
    let mut disk = vec![0u8; (off as usize) + ntfs.len() + MIB as usize * 2];
    disk[off as usize..off as usize + ntfs.len()].copy_from_slice(&ntfs);
    let path = tempfile::NamedTempFile::new().unwrap();
    path.as_file().write_all(&disk).unwrap();
    let res = scan_path(path.path(), &ScanOptions::default());
    let layout = res.layout.unwrap();
    let lost: Vec<_> = layout.partitions.iter().filter(|p| p.lost).collect();
    assert_eq!(lost.len(), 1, "{layout:?}");
    assert_eq!(lost[0].fs, FsKind::Ntfs);
    assert_eq!(lost[0].start, off);
    assert_eq!(
        res.found
            .iter()
            .filter(|f| f.origin == Origin::Ntfs)
            .count(),
        7
    );
}

#[test]
fn volume_cifrado_e_detectado_e_nao_varrido() {
    let mut disk = vec![0u8; 8 * MIB as usize];
    disk[..512].copy_from_slice(&mbr(&[(0x07, MIB, 4 * MIB), (0x83, 5 * MIB, 2 * MIB)]));
    // BitLocker na partição 1, LUKS na 2.
    disk[MIB as usize + 3..MIB as usize + 11].copy_from_slice(b"-FVE-FS-");
    let l = 5 * MIB as usize;
    disk[l..l + 6].copy_from_slice(b"LUKS\xBA\xBE");
    // Um PNG "dentro" do volume cifrado não deve ser tocado.
    let p = png(10_000);
    let at = (2 * MIB) as usize;
    disk[at..at + p.len()].copy_from_slice(&p);
    let path = tempfile::NamedTempFile::new().unwrap();
    path.as_file().write_all(&disk).unwrap();
    let res = scan_path(path.path(), &ScanOptions::default());
    let layout = res.layout.unwrap();
    assert_eq!(layout.partitions[0].encrypted.as_deref(), Some("BitLocker"));
    assert_eq!(layout.partitions[1].encrypted.as_deref(), Some("LUKS"));
    assert!(res.warnings.iter().any(|w| w.contains("BitLocker")));
    assert!(
        res.found.is_empty(),
        "não deve varrer volume cifrado: {:?}",
        res.found.len()
    );
}

#[test]
fn gpt_e_lido() {
    // GPT mínimo com 1 partição de dados.
    let mut disk = vec![0u8; 8 * MIB as usize];
    disk[..512].copy_from_slice(&mbr(&[(0xEE, 512, 8 * MIB - 512)]));
    let mut h = vec![0u8; 512];
    h[0..8].copy_from_slice(b"EFI PART");
    h[72..80].copy_from_slice(&2u64.to_le_bytes());
    h[80..84].copy_from_slice(&128u32.to_le_bytes());
    h[84..88].copy_from_slice(&128u32.to_le_bytes());
    disk[512..1024].copy_from_slice(&h);
    let e = &mut disk[1024..1024 + 128];
    e[0..16].copy_from_slice(&[
        0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99,
        0xC7,
    ]);
    e[32..40].copy_from_slice(&(MIB / 512).to_le_bytes());
    e[40..48].copy_from_slice(&((5 * MIB) / 512 - 1).to_le_bytes());
    let name: Vec<u8> = "Dados"
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    e[56..56 + name.len()].copy_from_slice(&name);
    let path = tempfile::NamedTempFile::new().unwrap();
    path.as_file().write_all(&disk).unwrap();
    let src = source::open_path(path.path()).unwrap();
    let l = partition::analyze(src.as_ref(), false);
    assert_eq!(l.table, "gpt");
    assert_eq!(l.partitions.len(), 1);
    assert_eq!(l.partitions[0].start, MIB);
    assert_eq!(l.partitions[0].label.as_deref(), Some("Dados"));
}

#[test]
fn imagem_forense_e_identica_e_origem_intacta() {
    let Some(dir) = fixtures() else { return };
    let src_path = dir.join("fat32.img");
    let src = source::open_path(&src_path).unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("copia.img");
    let rep = image::create_image(src.as_ref(), &dest, &|| false, &mut |_, _| {}).unwrap();
    assert!(rep.bad_ranges.is_empty());
    assert_eq!(rep.sha256, sha256_file(&src_path));
    assert_eq!(sha256_file(&dest), sha256_file(&src_path));
    // Não sobrescreve arquivo existente.
    assert!(image::create_image(src.as_ref(), &dest, &|| false, &mut |_, _| {}).is_err());
}

#[test]
fn pausa_e_retoma_encontra_o_mesmo() {
    let Some(dir) = fixtures() else { return };
    let path = dir.join("ext4.img");
    let full = scan_path(&path, &ScanOptions::default());
    let src = source::open_path(&path).unwrap();
    // Cancela logo na 3ª consulta, retoma do checkpoint.
    let n = std::sync::atomic::AtomicUsize::new(0);
    let cancel = || n.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 2;
    let mut sink = Collect::default();
    let out = scan::run(
        src.as_ref(),
        &ScanOptions::default(),
        None,
        &[],
        &cancel,
        &mut sink,
    );
    let scan::Outcome::Interrupted(cp) = out else {
        panic!("deveria ter pausado")
    };
    let before = sink.found.clone();
    let out = scan::run(
        src.as_ref(),
        &ScanOptions::default(),
        Some(cp),
        &before,
        &|| false,
        &mut sink,
    );
    assert!(matches!(out, scan::Outcome::Completed));
    let mut a: Vec<_> = full
        .found
        .iter()
        .map(|f| (f.first_offset(), f.size))
        .collect();
    let mut b: Vec<_> = sink
        .found
        .iter()
        .map(|f| (f.first_offset(), f.size))
        .collect();
    a.sort();
    b.sort();
    assert_eq!(a, b);
}

/// Streaming de pré-visualização: Range devolve exatamente os bytes do arquivo recuperado.
#[test]
fn file_slice_com_range_bate_com_o_arquivo_recuperado() {
    use std::io::Read;
    let Some(dir) = fixtures() else { return };
    let path = dir.join("ntfs.img");
    let eng = service::Engine::new();
    let job = eng
        .start_scan(
            path.to_str().unwrap(),
            ScanOptions {
                mode: ScanMode::Quick,
                ..Default::default()
            },
        )
        .unwrap();
    loop {
        let st = eng.status(&job).unwrap();
        if st["status"] != "running" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let res = eng
        .results(
            &job,
            &service::Query {
                query: Some("relatorio.pdf".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let id = res["items"][0]["id"].as_u64().unwrap();
    let size = res["items"][0]["size"].as_u64().unwrap();
    let want = expected(&dir, "ntfs")
        .into_iter()
        .find(|(_, p)| p.ends_with("relatorio.pdf"))
        .unwrap()
        .0;

    let full = eng.file_slice(&job, id, None).unwrap();
    assert_eq!((full.status(), full.len()), (200, size));
    let mut all = vec![];
    full.reader().read_to_end(&mut all).unwrap();
    assert_eq!(sha256(&all), want);

    let part = eng.file_slice(&job, id, Some("bytes=100-199")).unwrap();
    assert_eq!((part.status(), part.len()), (206, 100));
    assert_eq!(part.read_all(1 << 20).unwrap(), all[100..200]);
    let tail = eng.file_slice(&job, id, Some("bytes=-50")).unwrap();
    assert_eq!(tail.read_all(1 << 20).unwrap(), all[all.len() - 50..]);
    assert_eq!(
        eng.file_slice(&job, id, Some(&format!("bytes={size}-")))
            .err(),
        Some(416)
    );
    assert_eq!(eng.file_slice(&job, 999_999, None).err(), Some(404));
}

#[test]
fn configuracoes_de_tema_persistem() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("GUARA_CONFIG_DIR", dir.path());
    assert_eq!(settings::load().theme, "auto");
    settings::save(&settings::Settings {
        theme: "dark".into(),
    })
    .unwrap();
    assert_eq!(settings::load().theme, "dark");
}

fn wait_done(eng: &service::Engine, job: &str) {
    for _ in 0..500 {
        if eng.status(job).unwrap()["status"] != "running" {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("job não terminou");
}

/// Gera uma sessão no formato da v0.1.0 (sem os campos que vieram depois) a partir de uma
/// varredura real da imagem, com o alvo informado.
fn sessao_formato_v0_1_0(img: &std::path::Path, alvo: &str) -> tempfile::NamedTempFile {
    let eng = service::Engine::new();
    let job = eng
        .start_scan(img.to_str().unwrap(), ScanOptions::default())
        .unwrap();
    wait_done(&eng, &job);
    let tmp = tempfile::NamedTempFile::new().unwrap();
    eng.save_session(&job, tmp.path().to_str().unwrap())
        .unwrap();
    let mut v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.path()).unwrap()).unwrap();
    let o = v.as_object_mut().unwrap();
    o.remove("recoveries");
    o.remove("started_unix");
    o.insert("target".into(), alvo.into());
    for f in o["found"].as_array_mut().unwrap() {
        let f = f.as_object_mut().unwrap();
        for k in ["created", "accessed", "taken", "camera", "px", "gps", "sig"] {
            f.remove(k);
        }
    }
    std::fs::write(tmp.path(), serde_json::to_vec(&v).unwrap()).unwrap();
    tmp
}

/// Uma sessão gravada pela v0.1.0 continua abrindo nas versões novas.
#[test]
fn sessao_da_v0_1_0_abre_e_funciona() {
    let Some(dir) = fixtures() else { return };
    let img = dir.join("ntfs.img");
    let tmp = sessao_formato_v0_1_0(&img, img.to_str().unwrap());
    let eng = service::Engine::new();
    let job = eng.load_session(tmp.path().to_str().unwrap()).unwrap();
    wait_done(&eng, &job);
    let st = eng.status(&job).unwrap();
    assert_eq!(st["status"], "done");
    assert_eq!(st["found"], 7);
    assert_eq!(st["source"], true);
    let res = eng.results(&job, &service::Query::default()).unwrap();
    assert_eq!(res["total"], 7);
    // recuperar a partir da sessão antiga continua dando o mesmo conteúdo
    let out = tempfile::tempdir().unwrap();
    let rj = eng
        .start_recover(
            &job,
            None,
            Some(service::Query::default()),
            out.path().to_str().unwrap(),
        )
        .unwrap();
    wait_done(&eng, &rj);
    let got = hashes_in(out.path());
    for (h, p) in expected(&dir, "ntfs") {
        assert!(got.contains_key(&h), "faltou {p}");
    }
}

/// Origem indisponível não impede abrir a sessão; reapontar a origem religa tudo.
#[test]
fn sessao_abre_sem_a_origem_e_pode_ser_reconectada() {
    let Some(dir) = fixtures() else { return };
    let img = dir.join("ntfs.img");
    let tmp = sessao_formato_v0_1_0(&img, "/dev/disco-que-nao-existe");
    let eng = service::Engine::new();
    let job = eng.load_session(tmp.path().to_str().unwrap()).unwrap();
    let st = eng.status(&job).unwrap();
    assert_eq!(st["source"], false);
    assert_eq!(st["found"], 7);
    assert!(st["warnings"][0]
        .as_str()
        .unwrap()
        .contains("não está disponível"));
    // navegar e exportar funcionam
    assert_eq!(
        eng.results(&job, &service::Query::default()).unwrap()["total"],
        7
    );
    let csv = tempfile::NamedTempFile::new().unwrap();
    assert_eq!(
        eng.export_csv(&job, csv.path().to_str().unwrap(), None)
            .unwrap(),
        7
    );
    // pré-visualização, retomada e recuperação avisam com clareza
    assert_eq!(eng.thumb(&job, 1, 128).err(), Some(503));
    assert!(eng
        .resume(&job)
        .unwrap_err()
        .to_string()
        .contains("não está disponível"));
    let out = tempfile::tempdir().unwrap();
    let e = eng
        .start_recover(&job, None, None, out.path().to_str().unwrap())
        .unwrap_err();
    assert!(e.to_string().contains("não está disponível"), "{e}");
    // reconecta
    eng.relink(&job, img.to_str().unwrap()).unwrap();
    let st = eng.status(&job).unwrap();
    assert_eq!(st["source"], true);
    assert!(st["warnings"].as_array().unwrap().is_empty());
    let rj = eng
        .start_recover(&job, None, None, out.path().to_str().unwrap())
        .unwrap();
    wait_done(&eng, &rj);
    assert_eq!(hashes_in(out.path()).len(), 7);
    // arquivo que não é sessão
    std::fs::write(tmp.path(), b"nao e json").unwrap();
    let e = eng.load_session(tmp.path().to_str().unwrap()).unwrap_err();
    assert!(
        e.to_string().contains("não é uma sessão do Guará válida"),
        "{e}"
    );
}
