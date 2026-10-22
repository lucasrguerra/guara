//! Relatório em PDF: estrutura do arquivo (xref, páginas), texto com acentos e paginação.

use guara_core::recover::{RecoverReport, RecoveredFile};
use guara_core::report::{self, AuditNote, CaseInfo, ReportInput};
use guara_core::*;
use std::io::Read;

fn found(i: u64) -> Found {
    Found {
        id: i,
        name: format!("fotografia_{i:04}.jpg"),
        dir: Some("/Usuários/João/Imagens (cópia)".into()),
        category: Category::Image,
        ext: "jpg".into(),
        size: 1000 + i,
        origin: Origin::Carved,
        confidence: Confidence::High,
        data: DataLoc::Resident { bytes: vec![0; 4] },
        modified: Some(1_689_358_925),
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

fn build(n: u64) -> Vec<u8> {
    let files: Vec<Found> = (0..n).map(found).collect();
    let rec = RecoverReport {
        destination: "/mnt/outro/Recuperados".into(),
        recovered: (0..3)
            .map(|i| RecoveredFile {
                id: i,
                path: format!("Fotos/fotografia_{i:04}.jpg"),
                size: 1000 + i,
                sha256: "ab".repeat(32),
                md5: "cd".repeat(16),
                sha1: "ef".repeat(20),
                confidence: Confidence::High,
                origin: Origin::Carved,
                partial: false,
            })
            .collect(),
        total_bytes: 3003,
        ..Default::default()
    };
    let case = CaseInfo {
        case_id: "2026-001".into(),
        examiner: "Perita (Ana) \\ Souza".into(),
        organization: "Laboratório".into(),
        notes: "Observação com acentos: ção, ã, é, ü e “aspas”.".into(),
    };
    let opts = ScanOptions::default();
    report::pdf(&ReportInput {
        case: &case,
        target: "/dev/sdb",
        options: &opts,
        layout: None,
        found: &files,
        warnings: &["aviso importante sobre o disco".to_string()],
        started_unix: Some(1_689_358_925),
        elapsed: 1.5,
        recoveries: &[rec],
        source_hashes: None,
        audit: Some(AuditNote {
            entries: 7,
            ok: true,
            head: "12".repeat(32),
        }),
    })
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Texto dos fluxos de conteúdo, já descomprimidos.
fn streams(pdf: &[u8]) -> Vec<String> {
    let mut out = vec![];
    let mut at = 0;
    while let Some(s) = find(pdf, b">>\nstream\n", at) {
        let start = s + 10;
        let end = find(pdf, b"\nendstream", start).unwrap();
        let mut text = String::new();
        flate2::read::ZlibDecoder::new(&pdf[start..end])
            .read_to_string(&mut text)
            .unwrap_or_else(|e| panic!("fluxo inválido: {e}"));
        out.push(text);
        at = end;
    }
    out
}

#[test]
fn pdf_has_a_valid_cross_reference_table() {
    let pdf = build(5);
    assert!(pdf.starts_with(b"%PDF-1.4"));
    assert!(pdf.ends_with(b"%%EOF\n"));
    let sx = find(&pdf, b"startxref\n", 0).unwrap() + 10;
    let nl = find(&pdf, b"\n", sx).unwrap();
    let xref: usize = std::str::from_utf8(&pdf[sx..nl]).unwrap().parse().unwrap();
    assert_eq!(&pdf[xref..xref + 4], b"xref");
    // cada entrada aponta exatamente para "N 0 obj"
    let table = &pdf[xref..];
    let header_end = find(table, b"\n", 5).unwrap();
    let count: usize = std::str::from_utf8(&table[5..header_end])
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    for n in 1..count {
        let e = &table[header_end + 1 + n * 20..header_end + 1 + (n + 1) * 20];
        let off: usize = std::str::from_utf8(&e[..10]).unwrap().parse().unwrap();
        let want = format!("{n} 0 obj");
        assert_eq!(&pdf[off..off + want.len()], want.as_bytes(), "objeto {n}");
    }
}

#[test]
fn pdf_text_survives_accents_and_escaping() {
    let all = streams(&build(5)).join("\n");
    // acentos viram bytes WinAnsi em octal; parênteses e barra são escapados
    assert!(all.contains("Identifica\\347\\343o do caso"), "{all}");
    assert!(all.contains("Perita \\(Ana\\) \\\\ Souza"));
    assert!(all.contains("Laborat\\363rio"));
    assert!(all.contains("fotografia_0003.jpg"));
    assert!(all.contains(&"ab".repeat(32)));
    assert!(all.contains(&"12".repeat(32)), "hash da trilha ausente");
    // aspas curvas viram os bytes WinAnsi 0x93/0x94
    assert!(all.contains("\\223aspas\\224"));
}

#[test]
fn long_lists_paginate_with_numbered_footers() {
    let pdf = build(400);
    let pages = streams(&pdf);
    assert!(pages.len() >= 5, "{} páginas", pages.len());
    let last = pages.len();
    assert!(pages[last - 1].contains(&format!("p\\341gina {last} de {last}")));
    assert!(pages[0].contains(&format!("p\\341gina 1 de {last}")));
    // o cabeçalho da tabela se repete nas páginas seguintes
    assert!(pages[2].contains("Modificado \\(UTC\\)"));
    // todas as linhas aparecem exatamente uma vez
    let all = pages.join("\n");
    assert_eq!(all.matches("fotografia_0399.jpg").count(), 1);
    // a primeira também consta da lista de recuperados
    assert_eq!(all.matches("fotografia_0000.jpg").count(), 2);
}

#[test]
fn text_wraps_instead_of_running_off_the_page() {
    use guara_core::pdf::{text_width, wrap, Font};
    let lines = wrap(&"a".repeat(200), 8.0, Font::Mono, 100.0);
    assert!(lines.len() > 5);
    assert!(lines
        .iter()
        .all(|l| text_width(l, 8.0, Font::Mono) <= 100.0 + 1e-6));
    let words = wrap(
        "uma frase comum com várias palavras curtas",
        10.0,
        Font::Regular,
        90.0,
    );
    assert!(
        words.len() >= 3
            && words
                .iter()
                .all(|l| text_width(l, 10.0, Font::Regular) <= 90.0)
    );
    assert_eq!(wrap("a\nb", 10.0, Font::Regular, 100.0), ["a", "b"]);
}
