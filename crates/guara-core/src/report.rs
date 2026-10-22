//! Exportação para análise e relatório de recuperação (cadeia de custódia):
//! CSV dos achados e um relatório HTML autocontido, pronto para imprimir.

use crate::hashing::Hashes;
use crate::meta::fmt_unix;
use crate::recover::RecoverReport;
use crate::types::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaseInfo {
    #[serde(default)]
    pub case_id: String,
    #[serde(default)]
    pub examiner: String,
    #[serde(default)]
    pub organization: String,
    #[serde(default)]
    pub notes: String,
}

/// Situação da trilha de auditoria no momento do relatório.
pub struct AuditNote {
    pub entries: usize,
    pub ok: bool,
    /// Hash da última entrada, para guardar fora da máquina.
    pub head: String,
}

pub struct ReportInput<'a> {
    pub case: &'a CaseInfo,
    pub target: &'a str,
    pub options: &'a ScanOptions,
    pub layout: Option<&'a DiskLayout>,
    pub found: &'a [Found],
    pub warnings: &'a [String],
    pub started_unix: Option<i64>,
    pub elapsed: f64,
    pub recoveries: &'a [RecoverReport],
    pub source_hashes: Option<&'a Hashes>,
    pub audit: Option<AuditNote>,
}

fn cell(s: &str) -> String {
    // aspas e escapa; neutraliza "injeção de fórmula" em planilhas
    let mut t = s.replace('"', "\"\"");
    if t.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        t.insert(0, '\'');
    }
    format!("\"{t}\"")
}

fn iso(t: Option<i64>) -> String {
    match t {
        Some(t) => {
            let (y, m, d) = crate::meta::ymd(t);
            let r = t.rem_euclid(86400);
            format!(
                "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
                r / 3600,
                r % 3600 / 60,
                r % 60
            )
        }
        None => String::new(),
    }
}

/// CSV com `;` (abre direto no Excel em português) e BOM UTF-8.
pub fn csv(found: &[&Found]) -> String {
    let mut out = String::from("\u{feff}id;nome;pasta;extensao;categoria;tamanho_bytes;origem;confianca;modificado_utc;criado_utc;acessado_utc;foto_tirada;camera;largura;altura;gps;deslocamento_no_disco;observacao\r\n");
    for f in found {
        let (w, h) =
            f.px.map(|(w, h)| (w.to_string(), h.to_string()))
                .unwrap_or_default();
        let off = f.first_offset().map(|o| o.to_string()).unwrap_or_default();
        let cat = crate::explorer::cat_key(f.category);
        let row = [
            f.id.to_string(),
            cell(&f.name),
            cell(f.dir.as_deref().unwrap_or("")),
            cell(&f.ext),
            cat.to_string(),
            f.size.to_string(),
            format!("{:?}", f.origin).to_lowercase(),
            format!("{:?}", f.confidence).to_lowercase(),
            iso(f.modified),
            iso(f.created),
            iso(f.accessed),
            iso(f.taken),
            cell(f.camera.as_deref().unwrap_or("")),
            w,
            h,
            if f.gps { "sim".into() } else { String::new() },
            off,
            cell(f.note.as_deref().unwrap_or("")),
        ];
        out.push_str(&row.join(";"));
        out.push_str("\r\n");
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

const MAX_ROWS: usize = 20_000;

pub fn html(r: &ReportInput) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut o = String::new();
    o.push_str(
        r#"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Guará: relatório de recuperação</title><style>
:root{color-scheme:light}*{box-sizing:border-box}body{margin:0;font:14px/1.5 -apple-system,"Segoe UI",Inter,Roboto,sans-serif;color:#10201f;background:#f3f7f7}
main{max-width:1100px;margin:0 auto;padding:36px 28px 60px}header{border-bottom:3px solid #0f766e;padding-bottom:14px;margin-bottom:22px}
h1{margin:0;font-size:28px;letter-spacing:-.02em}h2{margin:30px 0 10px;font-size:17px;color:#0f766e}small,.mut{color:#5b6f6d}
table{width:100%;border-collapse:collapse;background:#fff;border-radius:10px;overflow:hidden;box-shadow:0 1px 3px rgba(0,0,0,.08)}
th,td{padding:7px 10px;text-align:left;border-bottom:1px solid #e4eded;vertical-align:top;font-size:13px}th{background:#e7f3f2;color:#0b4f4a;font-weight:600}
td.n{text-align:right;font-variant-numeric:tabular-nums}.mono{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:12px;word-break:break-all}
.kv td:first-child{width:230px;color:#5b6f6d}.pill{display:inline-block;padding:1px 8px;border-radius:99px;font-size:12px;font-weight:600}
.alta{background:#d7f5e0;color:#146c2e}.media{background:#fff0cf;color:#8a5a00}.baixa{background:#fde0e0;color:#a32121}
.note{background:#fff7e0;border-left:4px solid #e0a100;padding:10px 14px;border-radius:6px;margin:8px 0}
footer{margin-top:34px;color:#5b6f6d;font-size:12px}@media print{body{background:#fff}main{padding:0}table{box-shadow:none}h2{break-after:avoid}tr{break-inside:avoid}}
</style></head><body><main>"#,
    );
    o.push_str("<header><h1>Relatório de recuperação de arquivos</h1><div class=\"mut\">Gerado pelo Guará ");
    o.push_str(env!("CARGO_PKG_VERSION"));
    o.push_str(&format!(" em {}</div></header>", fmt_unix(now)));

    o.push_str("<h2>Identificação do caso</h2><table class=\"kv\">");
    for (k, v) in [
        ("Número do caso", &r.case.case_id),
        ("Responsável pelo exame", &r.case.examiner),
        ("Organização", &r.case.organization),
    ] {
        o.push_str(&format!(
            "<tr><td>{k}</td><td>{}</td></tr>",
            esc(if v.is_empty() { "não informado" } else { v })
        ));
    }
    if !r.case.notes.is_empty() {
        o.push_str(&format!(
            "<tr><td>Observações</td><td>{}</td></tr>",
            esc(&r.case.notes).replace('\n', "<br>")
        ));
    }
    o.push_str("</table>");

    o.push_str("<h2>Origem examinada</h2><table class=\"kv\">");
    o.push_str(&format!(
        "<tr><td>Dispositivo / imagem</td><td class=\"mono\">{}</td></tr>",
        esc(r.target)
    ));
    if let Some(l) = r.layout {
        o.push_str(&format!("<tr><td>Tamanho</td><td>{} ({} bytes)</td></tr><tr><td>Tabela de partições</td><td>{}</td></tr>", human(l.size), l.size, esc(&l.table.to_uppercase())));
    }
    if let Some(h) = r.source_hashes {
        o.push_str(&format!(
            "<tr><td>Hash da origem</td><td class=\"mono\">MD5 {}<br>SHA-1 {}<br>SHA-256 {}</td></tr>",
            esc(&h.md5),
            esc(&h.sha1),
            esc(&h.sha256)
        ));
    } else {
        o.push_str("<tr><td>Hash da origem</td><td class=\"mut\">não calculado nesta sessão (use <span class=\"mono\">guara hash</span> ou <span class=\"mono\">guara image</span>)</td></tr>");
    }
    if let Some(t) = r.started_unix {
        o.push_str(&format!(
            "<tr><td>Início da varredura</td><td>{}</td></tr>",
            fmt_unix(t)
        ));
    }
    o.push_str(&format!(
        "<tr><td>Duração da varredura</td><td>{:.1} s</td></tr><tr><td>Modo</td><td>{}{}</td></tr><tr><td>Garantia de leitura</td><td>A origem foi aberta somente para leitura; nenhuma escrita foi feita nela.</td></tr></table>",
        r.elapsed,
        if r.options.mode == ScanMode::Deep { "profunda (metadados + assinatura)" } else { "rápida (metadados)" },
        if r.options.include_allocated { ", incluindo áreas em uso" } else { "" }
    ));

    if let Some(a) = &r.audit {
        o.push_str(&format!(
            "<h2>Trilha de auditoria</h2><table class=\"kv\"><tr><td>Entradas</td><td>{}</td></tr><tr><td>Encadeamento</td><td>{}</td></tr><tr><td>Hash da última entrada</td><td class=\"mono\">{}</td></tr></table>",
            a.entries,
            if a.ok { "íntegro" } else { "QUEBRADO: confira com guara audit --verify" },
            esc(&a.head)
        ));
    }

    if let Some(l) = r.layout {
        o.push_str("<h2>Partições</h2><table><tr><th>#</th><th>Sistema</th><th>Início (bytes)</th><th>Tamanho</th><th>Rótulo</th><th>Observação</th></tr>");
        for p in &l.partitions {
            let obs = [
                p.lost.then_some("encontrada por varredura".to_string()),
                p.encrypted
                    .as_ref()
                    .map(|e| format!("cifrada ({e}), não examinada")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; ");
            o.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td>{}</td><td>{}</td></tr>",
                p.index + 1,
                esc(p.fs.name()),
                p.start,
                human(p.len),
                esc(p.label.as_deref().unwrap_or("")),
                esc(&obs)
            ));
        }
        for g in &l.gaps {
            o.push_str(&format!("<tr><td>-</td><td>espaço não particionado</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td></td><td></td></tr>", g.start, human(g.len)));
        }
        o.push_str("</table>");
    }

    // Resumo
    let mut by_cat = std::collections::BTreeMap::new();
    let mut conf = [0usize; 3];
    let mut bytes = 0u64;
    for f in r.found {
        *by_cat.entry(f.category.label_pt()).or_insert(0usize) += 1;
        conf[f.confidence as usize] += 1;
        bytes += f.size;
    }
    o.push_str(&format!("<h2>Resumo dos achados</h2><table class=\"kv\"><tr><td>Arquivos encontrados</td><td><b>{}</b> ({})</td></tr>", r.found.len(), human(bytes)));
    o.push_str(&format!(
        "<tr><td>Confiança</td><td><span class=\"pill alta\">alta {}</span> <span class=\"pill media\">média {}</span> <span class=\"pill baixa\">baixa {}</span></td></tr>",
        conf[2], conf[1], conf[0]
    ));
    o.push_str(&format!(
        "<tr><td>Por tipo</td><td>{}</td></tr></table>",
        by_cat
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(" · ")
    ));
    for w in r.warnings {
        o.push_str(&format!("<div class=\"note\">{}</div>", esc(w)));
    }

    if !r.recoveries.is_empty() {
        o.push_str("<h2>Arquivos recuperados (integridade)</h2>");
        for rec in r.recoveries {
            o.push_str(&format!(
                "<p>Destino: <span class=\"mono\">{}</span>, {} arquivo(s), {}{}</p>",
                esc(&rec.destination),
                rec.recovered.len(),
                human(rec.total_bytes),
                if rec.failed.is_empty() {
                    String::new()
                } else {
                    format!(", {} falha(s)", rec.failed.len())
                }
            ));
            o.push_str("<table><tr><th>Arquivo</th><th>Tamanho</th><th>Hashes</th></tr>");
            for f in rec.recovered.iter().take(MAX_ROWS) {
                o.push_str(&format!(
                    "<tr><td class=\"mono\">{}</td><td class=\"n\">{}</td><td class=\"mono\">MD5 {}<br>SHA-1 {}<br>SHA-256 {}</td></tr>",
                    esc(&f.path),
                    human(f.size),
                    f.md5,
                    f.sha1,
                    f.sha256
                ));
            }
            o.push_str("</table>");
        }
    }

    o.push_str("<h2>Relação de arquivos encontrados</h2>");
    if r.found.len() > MAX_ROWS {
        o.push_str(&format!("<div class=\"note\">Mostrando os primeiros {MAX_ROWS} de {}. Use a exportação CSV para a lista completa.</div>", r.found.len()));
    }
    o.push_str("<table><tr><th>Nome</th><th>Pasta</th><th>Tamanho</th><th>Modificado (UTC)</th><th>Criado (UTC)</th><th>Origem</th><th>Confiança</th><th>Posição no disco</th></tr>");
    for f in r.found.iter().take(MAX_ROWS) {
        let (cl, cc) = match f.confidence {
            Confidence::High => ("alta", "alta"),
            Confidence::Medium => ("média", "media"),
            Confidence::Low => ("baixa", "baixa"),
        };
        o.push_str(&format!(
            "<tr><td>{}</td><td class=\"mono\">{}</td><td class=\"n\">{}</td><td>{}</td><td>{}</td><td>{}</td><td><span class=\"pill {cc}\">{cl}</span></td><td class=\"n mono\">{}</td></tr>",
            esc(&f.name),
            esc(f.dir.as_deref().unwrap_or("")),
            human(f.size),
            iso(f.modified),
            iso(f.created),
            f.origin.label(),
            f.first_offset().map(|o| o.to_string()).unwrap_or_default()
        ));
    }
    o.push_str("</table><footer>Este relatório descreve o que o Guará encontrou e recuperou. Arquivos de confiança baixa podem estar incompletos ou sobrescritos. Datas dos metadados do sistema de arquivos estão em UTC. Guará é software livre, sem garantias.</footer></main></body></html>");
    o
}

/// Relatório em PDF (A4), com as mesmas seções do HTML.
pub fn pdf(r: &ReportInput) -> Vec<u8> {
    use crate::pdf::{Col, Font, Pdf, BLACK, GRAY};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let stamp = crate::audit::utc_iso(now);
    let version = env!("CARGO_PKG_VERSION");
    let mut d = Pdf::new(
        "Guará: relatório de recuperação",
        &format!("Guará {version}"),
    );
    d.title(
        "Relatório de recuperação de arquivos",
        &format!("Gerado pelo Guará {version} em {}", fmt_unix(now)),
    );

    let nf = |v: &str| {
        if v.is_empty() {
            "não informado".to_string()
        } else {
            v.to_string()
        }
    };
    d.heading("Identificação do caso");
    let mut rows = vec![
        ("Número do caso", nf(&r.case.case_id), Font::Regular),
        (
            "Responsável pelo exame",
            nf(&r.case.examiner),
            Font::Regular,
        ),
        ("Organização", nf(&r.case.organization), Font::Regular),
    ];
    if !r.case.notes.is_empty() {
        rows.push(("Observações", r.case.notes.clone(), Font::Regular));
    }
    let rows: Vec<(&str, &str, Font)> = rows.iter().map(|(k, v, f)| (*k, v.as_str(), *f)).collect();
    d.kv(&rows);

    d.heading("Origem examinada");
    let mut rows: Vec<(&str, String, Font)> =
        vec![("Dispositivo / imagem", r.target.to_string(), Font::Mono)];
    if let Some(l) = r.layout {
        rows.push((
            "Tamanho",
            format!("{} ({} bytes)", human(l.size), l.size),
            Font::Regular,
        ));
        rows.push(("Tabela de partições", l.table.to_uppercase(), Font::Regular));
    }
    rows.push((
        "Hash da origem",
        match r.source_hashes {
            Some(h) => format!("MD5 {}\nSHA-1 {}\nSHA-256 {}", h.md5, h.sha1, h.sha256),
            None => "não calculado nesta sessão (use guara hash ou guara image)".into(),
        },
        if r.source_hashes.is_some() {
            Font::Mono
        } else {
            Font::Regular
        },
    ));
    if let Some(t) = r.started_unix {
        rows.push(("Início da varredura", fmt_unix(t), Font::Regular));
    }
    rows.push((
        "Duração da varredura",
        format!("{:.1} s", r.elapsed),
        Font::Regular,
    ));
    rows.push((
        "Modo",
        format!(
            "{}{}",
            if r.options.mode == ScanMode::Deep {
                "profunda (metadados + assinatura)"
            } else {
                "rápida (metadados)"
            },
            if r.options.include_allocated {
                ", incluindo áreas em uso"
            } else {
                ""
            }
        ),
        Font::Regular,
    ));
    rows.push((
        "Garantia de leitura",
        "A origem foi aberta somente para leitura; nenhuma escrita foi feita nela.".into(),
        Font::Regular,
    ));
    let rows: Vec<(&str, &str, Font)> = rows.iter().map(|(k, v, f)| (*k, v.as_str(), *f)).collect();
    d.kv(&rows);

    if let Some(a) = &r.audit {
        d.heading("Trilha de auditoria");
        let ok = if a.ok {
            "íntegro"
        } else {
            "QUEBRADO: confira com guara audit --verify"
        };
        d.kv(&[
            ("Entradas", &a.entries.to_string(), Font::Regular),
            ("Encadeamento", ok, Font::Regular),
            ("Hash da última entrada", &a.head, Font::Mono),
        ]);
    }

    if let Some(l) = r.layout {
        d.heading("Partições");
        let cols = [
            Col {
                title: "#",
                width: 22.0,
                font: Font::Regular,
                right: false,
            },
            Col {
                title: "Sistema",
                width: 80.0,
                font: Font::Regular,
                right: false,
            },
            Col {
                title: "Início (bytes)",
                width: 90.0,
                font: Font::Regular,
                right: true,
            },
            Col {
                title: "Tamanho",
                width: 65.0,
                font: Font::Regular,
                right: true,
            },
            Col {
                title: "Rótulo",
                width: 90.0,
                font: Font::Regular,
                right: false,
            },
            Col {
                title: "Observação",
                width: 168.28,
                font: Font::Regular,
                right: false,
            },
        ];
        let mut rows: Vec<Vec<String>> = l
            .partitions
            .iter()
            .map(|p| {
                let obs = [
                    p.lost.then_some("encontrada por varredura".to_string()),
                    p.encrypted
                        .as_ref()
                        .map(|e| format!("cifrada ({e}), não examinada")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; ");
                vec![
                    (p.index + 1).to_string(),
                    p.fs.name().to_string(),
                    p.start.to_string(),
                    human(p.len),
                    p.label.clone().unwrap_or_default(),
                    obs,
                ]
            })
            .collect();
        for g in &l.gaps {
            rows.push(vec![
                "-".into(),
                "espaço não particionado".into(),
                g.start.to_string(),
                human(g.len),
                String::new(),
                String::new(),
            ]);
        }
        d.table(&cols, &rows);
    }

    let mut by_cat = std::collections::BTreeMap::new();
    let mut conf = [0usize; 3];
    let mut bytes = 0u64;
    for f in r.found {
        *by_cat.entry(f.category.label_pt()).or_insert(0usize) += 1;
        conf[f.confidence as usize] += 1;
        bytes += f.size;
    }
    d.heading("Resumo dos achados");
    d.kv(&[
        (
            "Arquivos encontrados",
            &format!("{} ({})", r.found.len(), human(bytes)),
            Font::Bold,
        ),
        (
            "Confiança",
            &format!(
                "alta {}  ·  média {}  ·  baixa {}",
                conf[2], conf[1], conf[0]
            ),
            Font::Regular,
        ),
        (
            "Por tipo",
            &by_cat
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join("  ·  "),
            Font::Regular,
        ),
    ]);
    for w in r.warnings {
        d.note(w);
    }

    if !r.recoveries.is_empty() {
        d.heading("Arquivos recuperados (integridade)");
        for rec in r.recoveries {
            d.paragraph(
                &format!(
                    "Destino: {}, {} arquivo(s), {}{}",
                    rec.destination,
                    rec.recovered.len(),
                    human(rec.total_bytes),
                    if rec.failed.is_empty() {
                        String::new()
                    } else {
                        format!(", {} falha(s)", rec.failed.len())
                    }
                ),
                9.0,
                Font::Regular,
                BLACK,
            );
            let cols = [
                Col {
                    title: "Arquivo",
                    width: 140.0,
                    font: Font::Mono,
                    right: false,
                },
                Col {
                    title: "Tamanho",
                    width: 55.0,
                    font: Font::Regular,
                    right: true,
                },
                Col {
                    title: "Hashes",
                    width: 320.28,
                    font: Font::Mono,
                    right: false,
                },
            ];
            let rows: Vec<Vec<String>> = rec
                .recovered
                .iter()
                .take(MAX_ROWS)
                .map(|f| {
                    vec![
                        f.path.clone(),
                        human(f.size),
                        format!("MD5 {}\nSHA-1 {}\nSHA-256 {}", f.md5, f.sha1, f.sha256),
                    ]
                })
                .collect();
            d.table(&cols, &rows);
        }
    }

    d.heading("Relação de arquivos encontrados");
    if r.found.len() > MAX_ROWS {
        d.note(&format!(
            "Mostrando os primeiros {MAX_ROWS} de {}. Use a exportação CSV para a lista completa.",
            r.found.len()
        ));
    }
    let cols = [
        Col {
            title: "Nome",
            width: 110.0,
            font: Font::Regular,
            right: false,
        },
        Col {
            title: "Pasta",
            width: 110.0,
            font: Font::Regular,
            right: false,
        },
        Col {
            title: "Tamanho",
            width: 50.0,
            font: Font::Regular,
            right: true,
        },
        Col {
            title: "Modificado (UTC)",
            width: 92.0,
            font: Font::Regular,
            right: false,
        },
        Col {
            title: "Origem",
            width: 50.0,
            font: Font::Regular,
            right: false,
        },
        Col {
            title: "Conf.",
            width: 32.0,
            font: Font::Regular,
            right: false,
        },
        Col {
            title: "Posição",
            width: 71.28,
            font: Font::Mono,
            right: true,
        },
    ];

    let rows: Vec<Vec<String>> = r
        .found
        .iter()
        .take(MAX_ROWS)
        .map(|f| {
            vec![
                f.name.clone(),
                f.dir.clone().unwrap_or_default(),
                human(f.size),
                iso(f.modified),
                f.origin.label().to_string(),
                f.confidence.label_pt().to_string(),
                f.first_offset().map(|o| o.to_string()).unwrap_or_default(),
            ]
        })
        .collect();
    d.table(&cols, &rows);
    d.paragraph(
        "Este relatório descreve o que o Guará encontrou e recuperou. Arquivos de confiança baixa podem estar incompletos ou sobrescritos. Datas dos metadados do sistema de arquivos estão em UTC. Guará é software livre, sem garantias.",
        8.0,
        Font::Regular,
        GRAY,
    );
    d.finish(&stamp.replace(['-', ':', 'T', 'Z'], ""))
}
