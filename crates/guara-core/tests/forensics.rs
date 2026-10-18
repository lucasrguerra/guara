//! Recursos periciais: hashes, metadados (EXIF/PDF/Office/MP4), miniaturas,
//! busca de texto, exportações e navegação (facetas/duplicadas).

use guara_core::explorer::{self, Query};
use guara_core::hashing::hash_bytes;
use guara_core::source::MemSource;
use guara_core::*;
use std::io::Write;

fn resident(name: &str, bytes: Vec<u8>) -> Found {
    let ext = name.rsplit('.').next().unwrap_or("").to_string();
    Found {
        id: 1,
        name: name.into(),
        dir: None,
        category: signatures::category_for_ext(&ext),
        ext,
        size: bytes.len() as u64,
        origin: Origin::Carved,
        confidence: Confidence::High,
        data: DataLoc::Resident { bytes },
        modified: None,
        created: None,
        accessed: None,
        taken: None,
        camera: None,
        px: None,
        gps: false,
        sig: None,
        note: None,
    }
}

// ------------------------------------------------------------------ hashes

#[test]
fn hashes_conferem_com_vetores_conhecidos() {
    let h = hash_bytes(b"abc");
    assert_eq!(h.md5, "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(h.sha1, "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(
        h.sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

// ------------------------------------------------------------------ EXIF

fn ifd(entries: &[(u16, u16, u32, Vec<u8>)], start: usize) -> Vec<u8> {
    let n = entries.len();
    let mut table = (n as u16).to_le_bytes().to_vec();
    let mut data: Vec<u8> = vec![];
    let data_off = start + 2 + 12 * n + 4;
    for (tag, typ, cnt, val) in entries {
        table.extend(tag.to_le_bytes());
        table.extend(typ.to_le_bytes());
        table.extend(cnt.to_le_bytes());
        if val.len() <= 4 {
            let mut v = val.clone();
            v.resize(4, 0);
            table.extend(v);
        } else {
            if data.len() % 2 == 1 {
                data.push(0);
            }
            table.extend(((data_off + data.len()) as u32).to_le_bytes());
            data.extend(val);
        }
    }
    table.extend([0, 0, 0, 0]);
    table.extend(data);
    table
}

fn rat(p: &[(u32, u32)]) -> Vec<u8> {
    p.iter()
        .flat_map(|(a, b)| a.to_le_bytes().into_iter().chain(b.to_le_bytes()))
        .collect()
}

fn jpeg_with_exif(orientation: u16, w: usize, h: usize) -> Vec<u8> {
    // pixels: gradiente
    let mut px = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            px.extend([(x * 255 / w) as u8, (y * 255 / h) as u8, 128]);
        }
    }
    let mut jpg = vec![];
    jpeg_encoder::Encoder::new(&mut jpg, 85)
        .encode(&px, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    let asc = |s: &str| {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    };
    let make = asc("Canon");
    let model = asc("Canon EOS 80D");
    let dt = asc("2023:07:14 18:22:05");
    let e0 = |exif: u32, gps: u32| {
        ifd(
            &[
                (0x010F, 2, make.len() as u32, make.clone()),
                (0x0110, 2, model.len() as u32, model.clone()),
                (0x0112, 3, 1, orientation.to_le_bytes().to_vec()),
                (0x8769, 4, 1, exif.to_le_bytes().to_vec()),
                (0x8825, 4, 1, gps.to_le_bytes().to_vec()),
            ],
            8,
        )
    };
    let first = e0(0, 0);
    let exif_off = 8 + first.len();
    let exif_ifd = ifd(&[(0x9003, 2, dt.len() as u32, dt.clone())], exif_off);
    let gps_off = exif_off + exif_ifd.len();
    let gps_ifd = ifd(
        &[
            (1, 2, 2, b"S\0".to_vec()),
            (2, 5, 3, rat(&[(23, 1), (33, 1), (1800, 100)])),
            (3, 2, 2, b"W\0".to_vec()),
            (4, 5, 3, rat(&[(46, 1), (38, 1), (0, 1)])),
        ],
        gps_off,
    );
    let mut tiff = b"II*\0".to_vec();
    tiff.extend(8u32.to_le_bytes());
    tiff.extend(e0(exif_off as u32, gps_off as u32));
    tiff.extend(exif_ifd);
    tiff.extend(gps_ifd);
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(tiff);
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend(((app1.len() + 2) as u16).to_be_bytes());
    out.extend(app1);
    out.extend(&jpg[2..]);
    out
}

#[test]
fn exif_camera_data_e_gps() {
    let f = resident("foto.jpg", jpeg_with_exif(1, 64, 48));
    let m = meta::extract(&MemSource(vec![]), &f);
    assert_eq!(m.camera.as_deref(), Some("Canon EOS 80D"));
    assert_eq!(m.px, Some((64, 48)));
    // 2023-07-14 18:22:05 lido como UTC
    assert_eq!(m.taken, Some(1_689_358_925));
    let (lat, lon) = m.gps.unwrap();
    assert!((lat + 23.5550).abs() < 0.001, "{lat}");
    assert!((lon + 46.6333).abs() < 0.001, "{lon}");
    assert!(m
        .fields
        .iter()
        .any(|(k, v)| k == "Foto tirada em" && v == "14/07/2023 18:22:05"));
}

#[test]
fn miniatura_respeita_orientacao_e_tamanho() {
    let src = MemSource(vec![]);
    // 64x48 com orientação 6 (girar 90 graus) => retrato 48x64, reduzido para caber em 32
    let f = resident("foto.jpg", jpeg_with_exif(6, 64, 48));
    let t = thumb::thumbnail(&src, &f, 32).expect("miniatura");
    let mut dec = jpeg_decoder::Decoder::new(&t[..]);
    dec.decode().unwrap();
    let i = dec.info().unwrap();
    assert_eq!((i.width, i.height), (24, 32));
    // PNG também
    let mut img = ::image::RgbImage::new(100, 50);
    for p in img.pixels_mut() {
        *p = ::image::Rgb([200, 30, 30]);
    }
    let mut png = std::io::Cursor::new(vec![]);
    img.write_to(&mut png, ::image::ImageFormat::Png).unwrap();
    let f = resident("x.png", png.into_inner());
    let t = thumb::thumbnail(&src, &f, 40).expect("miniatura png");
    let mut dec = jpeg_decoder::Decoder::new(&t[..]);
    dec.decode().unwrap();
    assert_eq!(
        (dec.info().unwrap().width, dec.info().unwrap().height),
        (40, 20)
    );
    // lixo não vira miniatura
    assert!(thumb::thumbnail(&src, &resident("x.jpg", vec![1, 2, 3, 4, 5]), 32).is_none());
}

// ------------------------------------------------------------------ PDF, Office, MP4

#[test]
fn metadados_de_pdf() {
    let pdf = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Count 12 /Kids [] >>\nendobj\n3 0 obj\n<< /Title (Relat\\363rio Final) /Author (Maria \\(Perita\\) Silva) /Creator (LibreOffice Writer) /Producer <FEFF00500044004600310032> /CreationDate (D:20230714182205-03'00') >>\nendobj\ntrailer\n%%EOF\n".to_vec();
    let m = meta::extract(&MemSource(vec![]), &resident("a.pdf", pdf));
    let get = |k: &str| {
        m.fields
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(get("Título").as_deref(), Some("Relatório Final"));
    assert_eq!(get("Autor").as_deref(), Some("Maria (Perita) Silva"));
    assert_eq!(get("Criado com").as_deref(), Some("LibreOffice Writer"));
    assert_eq!(get("Gerado por").as_deref(), Some("PDF12"));
    assert_eq!(get("Criado em").as_deref(), Some("14/07/2023 18:22:05"));
    assert_eq!(get("Páginas (aprox.)").as_deref(), Some("12"));
}

fn zip_store(entries: &[(&str, &str)], deflate: bool) -> Vec<u8> {
    let mut out = vec![];
    let mut cd = vec![];
    for (name, body) in entries {
        let (method, data): (u16, Vec<u8>) = if deflate {
            let mut e = flate2::write::DeflateEncoder::new(vec![], flate2::Compression::default());
            e.write_all(body.as_bytes()).unwrap();
            (8, e.finish().unwrap())
        } else {
            (0, body.as_bytes().to_vec())
        };
        let off = out.len() as u32;
        let crc = signatures::crc32(body.as_bytes());
        let mut lh = b"PK\x03\x04".to_vec();
        lh.extend([20, 0, 0, 0]);
        lh.extend(method.to_le_bytes());
        lh.extend([0, 0, 0, 0]);
        lh.extend(crc.to_le_bytes());
        lh.extend((data.len() as u32).to_le_bytes());
        lh.extend((body.len() as u32).to_le_bytes());
        lh.extend((name.len() as u16).to_le_bytes());
        lh.extend([0, 0]);
        lh.extend(name.as_bytes());
        out.extend(&lh);
        out.extend(&data);
        let mut c = b"PK\x01\x02".to_vec();
        c.extend([20, 0, 20, 0, 0, 0]);
        c.extend(method.to_le_bytes());
        c.extend([0, 0, 0, 0]);
        c.extend(crc.to_le_bytes());
        c.extend((data.len() as u32).to_le_bytes());
        c.extend((body.len() as u32).to_le_bytes());
        c.extend((name.len() as u16).to_le_bytes());
        c.extend([0; 12]);
        c.extend(off.to_le_bytes());
        c.extend(name.as_bytes());
        cd.extend(c);
    }
    let cd_off = out.len() as u32;
    out.extend(&cd);
    out.extend(b"PK\x05\x06\0\0\0\0");
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((cd.len() as u32).to_le_bytes());
    out.extend(cd_off.to_le_bytes());
    out.extend([0, 0]);
    out
}

#[test]
fn metadados_de_documentos_office() {
    let core = r#"<?xml version="1.0"?><cp:coreProperties xmlns:cp="x" xmlns:dc="y" xmlns:dcterms="z"><dc:title>Tese &amp; Defesa</dc:title><dc:creator>Ana Souza</dc:creator><cp:lastModifiedBy>Bruno Lima</cp:lastModifiedBy><dcterms:created xsi:type="dcterms:W3CDTF">2023-07-14T18:22:05Z</dcterms:created></cp:coreProperties>"#;
    let app = "<Properties><Application>Microsoft Office Word</Application><Company>ACME</Company></Properties>";
    for deflate in [false, true] {
        let z = zip_store(
            &[
                ("[Content_Types].xml", "<Types/>"),
                ("docProps/core.xml", core),
                ("docProps/app.xml", app),
            ],
            deflate,
        );
        let m = meta::extract(&MemSource(vec![]), &resident("t.docx", z));
        let get = |k: &str| {
            m.fields
                .iter()
                .find(|(a, _)| a == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            get("Título").as_deref(),
            Some("Tese & Defesa"),
            "deflate={deflate}"
        );
        assert_eq!(get("Autor").as_deref(), Some("Ana Souza"));
        assert_eq!(get("Última modificação por").as_deref(), Some("Bruno Lima"));
        assert_eq!(
            get("Criado em").as_deref(),
            Some("14/07/2023 18:22:05 (UTC)")
        );
        assert_eq!(get("Aplicativo").as_deref(), Some("Microsoft Office Word"));
        assert_eq!(get("Empresa").as_deref(), Some("ACME"));
    }
}

#[test]
fn metadados_de_mp4() {
    fn bx(ty: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend(ty);
        v.extend(body);
        v
    }
    let created_1904: u32 = 1_689_358_925 + 2_082_844_800;
    let mut mvhd = vec![0, 0, 0, 0];
    mvhd.extend(created_1904.to_be_bytes());
    mvhd.extend(created_1904.to_be_bytes());
    mvhd.extend(1000u32.to_be_bytes()); // timescale
    mvhd.extend(125_000u32.to_be_bytes()); // 125 s
    mvhd.extend(vec![0u8; 80]);
    let mut tkhd = vec![0u8; 84];
    tkhd[76..80].copy_from_slice(&(1920u32 << 16).to_be_bytes());
    tkhd[80..84].copy_from_slice(&(1080u32 << 16).to_be_bytes());
    let moov = bx(
        b"moov",
        &[bx(b"mvhd", &mvhd), bx(b"trak", &bx(b"tkhd", &tkhd))].concat(),
    );
    let mut file = bx(b"ftyp", b"isom\0\0\0\0isom");
    file.extend(moov);
    file.extend(bx(b"mdat", &[0u8; 32]));
    let m = meta::extract(&MemSource(vec![]), &resident("v.mp4", file));
    let get = |k: &str| {
        m.fields
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(get("Duração").as_deref(), Some("0:02:05"));
    assert_eq!(
        get("Gravado em (metadado)").as_deref(),
        Some("14/07/2023 18:22:05 (UTC)")
    );
    assert_eq!(get("Dimensões").as_deref(), Some("1920 × 1080 px"));
}

// ------------------------------------------------------------------ busca de texto

#[test]
fn busca_acha_utf8_utf16_e_na_fronteira_dos_blocos() {
    let mb = 1024 * 1024;
    let mut disk = vec![0u8; 20 * mb];
    let put = |d: &mut Vec<u8>, at: usize, b: &[u8]| d[at..at + b.len()].copy_from_slice(b);
    put(&mut disk, 1000, b"... Segredo-123 ...");
    let u16s: Vec<u8> = "segredo-123"
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    put(&mut disk, 3 * mb + 6, &u16s);
    // exatamente cruzando o limite do primeiro bloco de 8 MiB
    put(&mut disk, 8 * mb - 4, b"SEGREDO-123");
    let opts = search::SearchOpts {
        terms: vec!["segredo-123".into()],
        ignore_case: true,
        utf16: true,
        max_hits: 100,
    };
    let mut hits = vec![];
    let n = search::search(
        &MemSource(disk.clone()),
        &opts,
        &|| false,
        &mut |_, _| {},
        &mut |h| {
            hits.push(h);
            true
        },
    );
    assert_eq!(n, 3, "{hits:?}");
    assert_eq!(hits[0].offset, 1004);
    assert_eq!(hits[0].encoding, "UTF-8");
    assert!(hits[0].context.contains("Segredo-123"));
    assert_eq!(hits[1].offset, 3 * mb as u64 + 6);
    assert_eq!(hits[1].encoding, "UTF-16");
    assert_eq!(hits[2].offset, 8 * mb as u64 - 4);
    // sem ignorar maiúsculas só acha a grafia exata
    let opts = search::SearchOpts {
        terms: vec!["Segredo-123".into()],
        ignore_case: false,
        utf16: false,
        max_hits: 100,
    };
    let n = search::search(
        &MemSource(disk),
        &opts,
        &|| false,
        &mut |_, _| {},
        &mut |_| true,
    );
    assert_eq!(n, 1);
}

// ------------------------------------------------------------------ exportações

#[test]
fn csv_escapa_e_neutraliza_formulas() {
    let mut f = resident("=SOMA(A1).xlsx", vec![1]);
    f.note = Some("diz \"oi\"; ok".into());
    f.camera = Some("Cam;X".into());
    let csv = report::csv(&[&f]);
    assert!(csv.starts_with('\u{feff}'));
    assert!(csv.contains("\"'=SOMA(A1).xlsx\""), "{csv}");
    assert!(csv.contains("\"diz \"\"oi\"\"; ok\""));
    assert_eq!(csv.lines().count(), 2);
}

#[test]
fn relatorio_html_escapa_conteudo_e_traz_o_caso() {
    let f = resident("<script>alert(1)</script>.txt", vec![1, 2, 3]);
    let case = report::CaseInfo {
        case_id: "2026-001".into(),
        examiner: "Perita <Ana>".into(),
        organization: "Lab".into(),
        notes: "obs".into(),
    };
    let opts = ScanOptions::default();
    let hashes = hash_bytes(b"disco");
    let html = report::html(&report::ReportInput {
        case: &case,
        target: "/dev/sdb",
        options: &opts,
        layout: None,
        found: &[f],
        warnings: &["aviso <b>".to_string()],
        started_unix: Some(1_689_358_925),
        elapsed: 1.5,
        recoveries: &[],
        source_hashes: Some(&hashes),
    });
    assert!(!html.contains("<script>alert"), "HTML não escapado");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;.txt"));
    assert!(html.contains("2026-001") && html.contains("Perita &lt;Ana&gt;"));
    assert!(html.contains(&hashes.sha256));
    assert!(html.contains("somente para leitura"));
}

// ------------------------------------------------------------------ navegação

#[test]
fn facetas_filtros_e_duplicadas() {
    let mk = |id: u64,
              name: &str,
              dir: Option<&str>,
              taken: Option<i64>,
              cam: Option<&str>,
              sig: Option<u64>| {
        let mut f = resident(name, vec![0; 10]);
        f.id = id;
        f.dir = dir.map(String::from);
        f.origin = if dir.is_some() {
            Origin::Fat
        } else {
            Origin::Carved
        };
        f.taken = taken;
        f.camera = cam.map(String::from);
        f.sig = sig;
        f
    };
    let jul23 = 1_689_358_925; // 2023-07-14
    let jan24 = 1_705_000_000; // 2024-01-11
    let found = vec![
        mk(
            1,
            "a.jpg",
            Some("DCIM/100"),
            Some(jul23),
            Some("Canon"),
            Some(7),
        ),
        mk(
            2,
            "b.jpg",
            Some("DCIM/100"),
            Some(jan24),
            Some("Canon"),
            Some(8),
        ),
        mk(3, "c.jpg", Some("Viagens"), Some(jul23), None, Some(7)), // duplicada de a.jpg
        mk(4, "d.jpg", None, None, None, None),
    ];
    let q = |json: &str| -> Query { serde_json::from_str(json).unwrap() };
    let f = explorer::facets(&found);
    assert_eq!(f["total"], 4);
    assert_eq!(f["years"][0]["year"], 2024);
    assert_eq!(f["years"][1]["year"], 2023);
    assert_eq!(f["years"][1]["count"], 2);
    assert_eq!(f["undated"], 1);
    assert_eq!(f["carved"], 1);
    assert_eq!(f["duplicates"], 1);
    assert_eq!(f["small"], 4); // todos têm 10 bytes
    assert_eq!(q(r#"{"min_size":30720}"#).select(&found).len(), 0);
    assert_eq!(f["cameras"][0]["name"], "Canon");
    let dcim = f["folders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["path"] == "DCIM")
        .unwrap();
    assert_eq!(dcim["count"], 2);

    assert_eq!(q(r#"{"year":2023}"#).select(&found).len(), 2);
    assert_eq!(q(r#"{"year":2023,"month":7}"#).select(&found).len(), 2);
    assert_eq!(q(r#"{"year":2023,"month":8}"#).select(&found).len(), 0);
    assert_eq!(q(r#"{"folder":"DCIM"}"#).select(&found).len(), 2);
    assert_eq!(q(r#"{"folder":"@carved"}"#).select(&found).len(), 1);
    assert_eq!(q(r#"{"camera":"Canon"}"#).select(&found).len(), 2);
    assert_eq!(q(r#"{"undated":true}"#).select(&found).len(), 1);
    assert_eq!(q(r#"{"dedupe":true}"#).select(&found).len(), 3);
    assert_eq!(q(r#"{"query":"viagens"}"#).select(&found).len(), 1);
    let mut items = q("{}").select(&found);
    explorer::sort_items(&mut items, Some("date"));
    assert_eq!(items[0].id, 2); // mais recente primeiro
    assert_eq!(items.last().unwrap().id, 4); // sem data por último
}

// ------------------------------------------------------------------ carving

fn png_chunk(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend(ty);
    c.extend(data);
    c.extend([0, 0, 0, 0]);
    c
}

/// Regressão: texto ASCII depois de um PNG truncado vira "tamanho" de ~1,9 GB;
/// o carving não pode inventar um PNG gigante.
#[test]
fn png_truncado_seguido_de_texto_nao_vira_arquivo_gigante() {
    let mut d = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    d.extend(png_chunk(b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 2, 0, 0, 0]));
    d.extend(png_chunk(b"IDAT", &vec![7u8; 5000]));
    let real_end = d.len();
    d.extend(b"ualiBase64 texto qualquer que segue no disco, como parte de outro arquivo html ");
    d.resize(4 * 1024 * 1024, 0x41);
    let src = MemSource(d.clone());
    let hit =
        signatures::detect(&src, 0, &d[..64], 8 << 30).expect("PNG truncado ainda é reconhecido");
    assert_eq!(
        hit.len as usize, real_end,
        "deve parar no fim do último bloco válido"
    );
    assert_eq!(hit.confidence, Confidence::Low);

    // sem nenhum dado de imagem (só cabeçalho) não é arquivo
    let mut d2 = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    d2.extend(png_chunk(b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 2, 0, 0, 0]));
    d2.resize(1 << 20, b'e');
    assert!(signatures::detect(&MemSource(d2.clone()), 0, &d2[..64], 8 << 30).is_none());

    // PNG completo continua com confiança alta e tamanho exato
    let mut ok = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    ok.extend(png_chunk(b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 2, 0, 0, 0]));
    ok.extend(png_chunk(b"IDAT", &[1; 100]));
    ok.extend(png_chunk(b"IEND", &[]));
    let n = ok.len();
    ok.resize(1 << 20, 0);
    let h = signatures::detect(&MemSource(ok.clone()), 0, &ok[..64], 8 << 30).unwrap();
    assert_eq!((h.len as usize, h.confidence), (n, Confidence::High));
}

// ------------------------------------------------------------------ abrir no player do sistema

/// O vídeo é copiado para uma pasta temporária e entregue ao aplicativo padrão do sistema
/// (aqui um `xdg-open`/`open` falso grava o que recebeu).
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn abre_no_player_do_sistema() {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    let marker = bin.path().join("recebido");
    let name = if cfg!(target_os = "linux") {
        "xdg-open"
    } else {
        "open"
    };
    let script = bin.path().join(name);
    std::fs::write(
        &script,
        format!("#!/bin/sh\ncp \"$1\" '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{old}", bin.path().display()));
    let content: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
    let mut f = resident("meu video.mp4", content.clone());
    f.id = 42;
    let path = external::open_found(&MemSource(vec![]), &f).unwrap();
    std::env::set_var("PATH", old);
    assert!(path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("42-"));
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert_eq!(std::fs::read(&marker).unwrap(), content);
    assert_eq!(std::fs::read(&path).unwrap(), content);
    let _ = std::fs::remove_file(path);
}
