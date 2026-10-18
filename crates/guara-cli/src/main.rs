//! Guará CLI: `guara devices | inspect | scan | resume | recover | image | ui`.

mod serve;

use clap::{Parser, Subcommand};
use guara_core::service::{Engine, Query};
use guara_core::{Category, Confidence, ScanMode, ScanOptions};
use serde_json::{json, Value};
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "guara", version, about = "Guará: recuperação de arquivos (somente leitura na origem)", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Lista discos e pendrives detectados
    Devices {
        #[arg(long)]
        json: bool,
    },
    /// Mostra partições, sistemas de arquivos, espaço não particionado e volumes cifrados
    Inspect {
        /// Dispositivo (ex.: /dev/sdb, \\.\PhysicalDrive1) ou arquivo de imagem
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// Procura arquivos apagados/perdidos (metadados + assinatura)
    Scan(ScanArgs),
    /// Continua uma varredura pausada (Ctrl+C) a partir do arquivo de sessão
    Resume {
        session: String,
        #[command(flatten)]
        out: OutArgs,
    },
    /// Recupera arquivos de uma sessão salva
    Recover {
        session: String,
        /// Pasta de destino (em OUTRO disco)
        dest: String,
        #[command(flatten)]
        filter: FilterArgs,
    },
    /// Cria uma imagem do disco (tolerante a setores ruins) para trabalhar na cópia
    Image { path: String, dest: String },
    /// Exporta os achados de uma sessão para CSV (abre no Excel)
    Export {
        session: String,
        /// Arquivo CSV de saída
        csv: String,
        #[command(flatten)]
        filter: FilterArgs,
    },
    /// Gera o relatório HTML de recuperação (identificação do caso, origem, achados, hashes)
    Report {
        session: String,
        /// Arquivo HTML de saída
        out: String,
        #[arg(long, default_value = "")]
        case: String,
        #[arg(long, default_value = "")]
        examiner: String,
        #[arg(long, default_value = "")]
        org: String,
        #[arg(long, default_value = "")]
        notes: String,
        /// Calcula MD5/SHA-1/SHA-256 da origem e inclui no relatório (pode demorar)
        #[arg(long)]
        hash_source: bool,
    },
    /// Procura texto em todo o disco, inclusive espaço livre (UTF-8 e UTF-16)
    Search {
        path: String,
        /// Termos a procurar
        #[arg(required = true)]
        terms: Vec<String>,
        /// Diferencia maiúsculas de minúsculas
        #[arg(long)]
        case_sensitive: bool,
        /// Não procura em UTF-16
        #[arg(long)]
        no_utf16: bool,
        #[arg(long, default_value_t = 200)]
        max: usize,
        #[arg(long)]
        json: bool,
    },
    /// Calcula MD5, SHA-1 e SHA-256 do dispositivo ou imagem (verificação de integridade)
    Hash { path: String },
    /// Abre a interface gráfica no navegador (modo local)
    Ui {
        #[arg(long, default_value_t = 0)]
        port: u16,
        #[arg(long)]
        no_open: bool,
    },
}

#[derive(clap::Args, Clone)]
struct FilterArgs {
    /// Categorias: image, document, video, audio, archive, database, other
    #[arg(long, value_delimiter = ',')]
    category: Vec<String>,
    /// Confiança mínima: low, medium, high
    #[arg(long, default_value = "low")]
    min_confidence: String,
    /// Só nomes/caminhos que contenham este texto
    #[arg(long)]
    name: Option<String>,
}

#[derive(clap::Args, Clone)]
struct OutArgs {
    /// Salva a sessão (necessária para recover/resume)
    #[arg(long)]
    session: Option<String>,
    /// Lista os arquivos encontrados
    #[arg(long)]
    list: bool,
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct ScanArgs {
    /// Dispositivo ou imagem
    path: String,
    /// Varredura rápida: só metadados dos sistemas de arquivos
    #[arg(long)]
    quick: bool,
    /// Também varre áreas ainda alocadas (mais lento)
    #[arg(long)]
    include_allocated: bool,
    /// Recupera tudo que casar com os filtros para esta pasta ao terminar
    #[arg(long)]
    recover: Option<String>,
    #[command(flatten)]
    filter: FilterArgs,
    #[command(flatten)]
    out: OutArgs,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("{} {e}", paint("31", "erro:"));
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let eng = Engine::new();
    match cli.cmd {
        Cmd::Devices { json } => {
            let v = eng.rpc("devices", json!({})).map_err(s)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
                return Ok(());
            }
            let list = v.as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!("Nenhum dispositivo encontrado (ou sem permissão de leitura).");
            }
            for d in list {
                let trim = match d["trim"].as_bool() {
                    Some(true) => "  [TRIM ativo: apagados tendem a sumir]",
                    _ => "",
                };
                println!(
                    "{:<22} {:<6} {:>10}  {}{}{}",
                    d["path"].as_str().unwrap_or(""),
                    d["kind"].as_str().unwrap_or(""),
                    human(d["size"].as_u64().unwrap_or(0)),
                    d["name"].as_str().unwrap_or(""),
                    if d["readable"].as_bool() == Some(false) {
                        "  (precisa de administrador)"
                    } else {
                        ""
                    },
                    trim
                );
                for m in d["mounted_at"].as_array().into_iter().flatten() {
                    println!("{:<22} montado em {}", "", m.as_str().unwrap_or(""));
                }
            }
            Ok(())
        }
        Cmd::Inspect { path, json } => {
            let v = eng.rpc("inspect", json!({ "path": path })).map_err(s)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                print_layout(&v);
            }
            Ok(())
        }
        Cmd::Scan(a) => {
            let opts = ScanOptions {
                mode: if a.quick {
                    ScanMode::Quick
                } else {
                    ScanMode::Deep
                },
                include_allocated: a.include_allocated,
                categories: parse_cats(&a.filter.category)?,
                ..Default::default()
            };
            let job = eng.start_scan(&a.path, opts).map_err(s)?;
            drive_scan(&eng, &job, &a.out, &a.filter, a.recover.as_deref())
        }
        Cmd::Resume { session, out } => {
            let job = eng.load_session(&session).map_err(s)?;
            eng.resume(&job).map_err(s)?;
            let out = OutArgs {
                session: out.session.or(Some(session)),
                ..out
            };
            drive_scan(
                &eng,
                &job,
                &out,
                &FilterArgs {
                    category: vec![],
                    min_confidence: "low".into(),
                    name: None,
                },
                None,
            )
        }
        Cmd::Recover {
            session,
            dest,
            filter,
        } => {
            let job = eng.load_session(&session).map_err(s)?;
            let st = eng.status(&job).map_err(s)?;
            if st["status"] == "paused" {
                return Err(
                    "a varredura dessa sessão não terminou; use `guara resume` primeiro".into(),
                );
            }
            recover_job(&eng, &job, &dest, &filter)?;
            eng.save_session(&job, &session).map_err(s)?;
            Ok(())
        }
        Cmd::Image { path, dest } => {
            let job = eng.start_image(&path, &dest).map_err(s)?;
            let st = wait(&eng, &job, None)?;
            if st["status"] == "error" {
                return Err(st["error"].as_str().unwrap_or("falha").to_string());
            }
            let r = &st["report"];
            eprintln!();
            println!("Imagem criada: {dest}");
            println!("SHA-256: {}", r["sha256"].as_str().unwrap_or(""));
            let bad = r["bad_ranges"].as_array().map(|a| a.len()).unwrap_or(0);
            if bad > 0 {
                println!("Atenção: {bad} trecho(s) ilegível(is) preenchidos com zeros (veja {}.map.json).", dest.trim_end_matches(".img"));
            }
            Ok(())
        }
        Cmd::Export {
            session,
            csv,
            filter,
        } => {
            let job = eng.load_session(&session).map_err(s)?;
            let q = query_from(&filter)?;
            let r = eng
                .rpc("export_csv", json!({ "job": job, "path": csv, "query": q }))
                .map_err(s)?;
            println!("{} linha(s) gravadas em {csv}", r["rows"]);
            Ok(())
        }
        Cmd::Report {
            session,
            out,
            case,
            examiner,
            org,
            notes,
            hash_source,
        } => {
            let job = eng.load_session(&session).map_err(s)?;
            let mut hashes = Value::Null;
            if hash_source {
                let st = eng.status(&job).map_err(s)?;
                let target = st["target"].as_str().unwrap_or("").to_string();
                let hj = eng.start_hash(&target).map_err(s)?;
                let hst = wait(&eng, &hj, None)?;
                eprintln!();
                if hst["status"] == "error" {
                    return Err(hst["error"].as_str().unwrap_or("falha").to_string());
                }
                hashes = hst["report"].clone();
            }
            let case = json!({ "case_id": case, "examiner": examiner, "organization": org, "notes": notes });
            eng.rpc(
                "export_report",
                json!({ "job": job, "path": out, "case": case, "hashes": hashes }),
            )
            .map_err(s)?;
            println!("Relatório gravado em {out}");
            Ok(())
        }
        Cmd::Search {
            path,
            terms,
            case_sensitive,
            no_utf16,
            max,
            json,
        } => {
            let opts = json!({ "path": path, "terms": terms, "ignore_case": !case_sensitive, "utf16": !no_utf16, "max_hits": max });
            let job = eng.rpc("search_start", opts).map_err(s)?["job"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let st = wait(&eng, &job, None)?;
            eprintln!();
            if st["status"] == "error" {
                return Err(st["error"].as_str().unwrap_or("falha").to_string());
            }
            let res = eng
                .rpc("search_results", json!({ "job": job, "limit": 2000 }))
                .map_err(s)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
                return Ok(());
            }
            for h in res["items"].as_array().into_iter().flatten() {
                println!(
                    "{:>14}  {}  {:<6} {:<8} {}",
                    h["offset"],
                    h["partition"]
                        .as_u64()
                        .map(|p| format!("part.{p}"))
                        .unwrap_or_else(|| "fora".into()),
                    h["encoding"].as_str().unwrap_or(""),
                    h["term"].as_str().unwrap_or(""),
                    h["context"].as_str().unwrap_or("")
                );
            }
            println!("{} ocorrência(s).", res["total"]);
            Ok(())
        }
        Cmd::Hash { path } => {
            let job = eng.start_hash(&path).map_err(s)?;
            let st = wait(&eng, &job, None)?;
            eprintln!();
            if st["status"] == "error" {
                return Err(st["error"].as_str().unwrap_or("falha").to_string());
            }
            let r = &st["report"];
            println!(
                "MD5     {}\nSHA-1   {}\nSHA-256 {}",
                r["md5"].as_str().unwrap_or(""),
                r["sha1"].as_str().unwrap_or(""),
                r["sha256"].as_str().unwrap_or("")
            );
            Ok(())
        }
        Cmd::Ui { port, no_open } => serve::run(port, !no_open),
    }
}

fn parse_cats(v: &[String]) -> Result<Vec<Category>, String> {
    v.iter()
        .map(|c| Category::parse(c).ok_or(format!("categoria desconhecida: {c}")))
        .collect()
}

fn query_from(f: &FilterArgs) -> Result<Query, String> {
    let cats = parse_cats(&f.category)?;
    let min = match f.min_confidence.as_str() {
        "low" | "baixa" => Confidence::Low,
        "medium" | "media" | "média" => Confidence::Medium,
        "high" | "alta" => Confidence::High,
        o => return Err(format!("confiança inválida: {o}")),
    };
    // Query aceita uma categoria; várias categorias são tratadas filtrando depois.
    Ok(Query {
        category: if cats.len() == 1 { Some(cats[0]) } else { None },
        min_confidence: Some(min),
        query: f.name.clone(),
        sort: Some("confidence".into()),
        offset: 0,
        limit: 2000,
        ..Default::default()
    })
}

fn drive_scan(
    eng: &Engine,
    job: &str,
    out: &OutArgs,
    filter: &FilterArgs,
    recover: Option<&str>,
) -> Result<(), String> {
    let st = wait(eng, job, out.session.as_deref())?;
    eprintln!();
    match st["status"].as_str() {
        Some("error") => return Err(st["error"].as_str().unwrap_or("falha").to_string()),
        Some("paused") => {
            if let Some(p) = &out.session {
                eng.save_session(job, p).map_err(s)?;
                eprintln!("Varredura pausada. Continue com: guara resume {p}");
            } else {
                eprintln!("Varredura pausada (use --session <arquivo> para poder retomar).");
            }
            return Ok(());
        }
        _ => {}
    }
    if let Some(p) = &out.session {
        eng.save_session(job, p).map_err(s)?;
    }
    if out.json {
        let res = eng.results(job, &query_from(filter)?).map_err(s)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "status": st, "results": res })).unwrap()
        );
    } else {
        print_summary(&st);
        if out.list {
            let q = query_from(filter)?;
            let res = eng.results(job, &Query { limit: 2000, ..q }).map_err(s)?;
            print_list(&res);
        }
    }
    if let Some(dest) = recover {
        recover_job(eng, job, dest, filter)?;
        if let Some(p) = &out.session {
            eng.save_session(job, p).map_err(s)?;
        }
    } else if st["found"].as_u64().unwrap_or(0) > 0 && out.session.is_some() {
        eprintln!(
            "Para recuperar: guara recover {} <pasta-de-destino>",
            out.session.as_deref().unwrap()
        );
    }
    Ok(())
}

fn recover_job(
    eng: &Engine,
    scan_job: &str,
    dest: &str,
    filter: &FilterArgs,
) -> Result<(), String> {
    let mut q = query_from(filter)?;
    // Várias categorias: seleciona por ids.
    let cats = parse_cats(&filter.category)?;
    let mut ids: Option<Vec<u64>> = None;
    if cats.len() > 1 {
        let mut all = vec![];
        for c in cats {
            q.category = Some(c);
            let r = eng
                .results(
                    scan_job,
                    &Query {
                        limit: 2000,
                        ..q.clone()
                    },
                )
                .map_err(s)?;
            all.extend(
                r["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|i| i["id"].as_u64()),
            );
        }
        ids = Some(all);
    }
    let rj = eng.start_recover(scan_job, ids, Some(q), dest).map_err(s)?;
    let st = wait(eng, &rj, None)?;
    eprintln!();
    if st["status"] == "error" {
        return Err(st["error"].as_str().unwrap_or("falha").to_string());
    }
    let r = &st["report"];
    let n = r["recovered"].as_array().map(|a| a.len()).unwrap_or(0);
    let failed = r["failed"].as_array().map(|a| a.len()).unwrap_or(0);
    println!(
        "{} {n} arquivo(s) recuperado(s) para {dest} ({}).",
        paint("32", "✓"),
        human(r["total_bytes"].as_u64().unwrap_or(0))
    );
    if failed > 0 {
        println!(
            "{} {failed} falharam (detalhes no manifesto).",
            paint("33", "!")
        );
    }
    println!(
        "Manifesto com SHA-256: {}/guara-manifesto.json",
        dest.trim_end_matches('/')
    );
    Ok(())
}

fn wait(eng: &Engine, job: &str, session: Option<&str>) -> Result<Value, String> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = interrupted.clone();
    let _ = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst));
    let tty = std::io::stderr().is_terminal();
    let mut asked = false;
    loop {
        let st = eng.status(job).map_err(s)?;
        let status = st["status"].as_str().unwrap_or("").to_string();
        if interrupted.load(Ordering::SeqCst) && !asked && status == "running" {
            asked = true;
            if session.is_some() {
                eprintln!("\nPausando…");
                let _ = eng.pause(job);
            } else {
                let _ = eng.stop(job);
            }
        }
        if status != "running" {
            return Ok(st);
        }
        if tty {
            let p = &st["progress"];
            let (d, t) = (
                p["done"].as_u64().unwrap_or(0),
                p["total"].as_u64().unwrap_or(0),
            );
            let pct = d
                .checked_mul(100)
                .and_then(|v| v.checked_div(t))
                .map_or("  … ".to_string(), |v| format!("{v:>3}%"));
            let found = st["found"].as_u64().unwrap_or(0);
            let found = if st["hits"].as_u64().unwrap_or(0) > 0 {
                st["hits"].as_u64().unwrap_or(0)
            } else {
                found
            };
            let line = format!(
                "{pct}  {}  ·  {found} resultado(s)  ·  {:.0}s",
                p["phase"].as_str().unwrap_or(""),
                st["elapsed"].as_f64().unwrap_or(0.0)
            );
            eprint!("\r\x1b[2K{line}");
            let _ = std::io::stderr().flush();
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn print_layout(v: &Value) {
    println!(
        "Tamanho: {}   Tabela de partições: {}",
        human(v["size"].as_u64().unwrap_or(0)),
        v["table"].as_str().unwrap_or("-").to_uppercase()
    );
    for p in v["partitions"].as_array().into_iter().flatten() {
        let enc = p["encrypted"]
            .as_str()
            .map(|e| format!("  🔒 cifrada ({e})"))
            .unwrap_or_default();
        println!(
            "  #{:<2} início {:>12}  tamanho {:>10}  {:<10} {}{}{}",
            p["index"].as_u64().unwrap_or(0) + 1,
            p["start"].as_u64().unwrap_or(0),
            human(p["len"].as_u64().unwrap_or(0)),
            p["fs"].as_str().unwrap_or("?"),
            p["label"].as_str().unwrap_or(""),
            if p["lost"].as_bool() == Some(true) {
                "  (perdida)"
            } else {
                ""
            },
            enc
        );
    }
    for g in v["gaps"].as_array().into_iter().flatten() {
        println!(
            "  espaço não particionado: início {} tamanho {}",
            g["start"],
            human(g["len"].as_u64().unwrap_or(0))
        );
    }
    for w in v["warnings"].as_array().into_iter().flatten() {
        println!("{} {}", paint("33", "!"), w.as_str().unwrap_or(""));
    }
}

fn print_summary(st: &Value) {
    println!(
        "Concluído em {:.1}s: {} arquivo(s) encontrado(s).",
        st["elapsed"].as_f64().unwrap_or(0.0),
        st["found"]
    );
    let c = &st["confidence"];
    println!(
        "  confiança alta: {}   média: {}   baixa: {}",
        c["high"], c["medium"], c["low"]
    );
    if let Some(m) = st["categories"].as_object() {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by_key(|(k, _)| k.to_string());
        let line: Vec<String> = v.iter().map(|(k, n)| format!("{k}: {n}")).collect();
        println!("  {}", line.join("   "));
    }
    for w in st["warnings"].as_array().into_iter().flatten() {
        println!("{} {}", paint("33", "!"), w.as_str().unwrap_or(""));
    }
}

fn print_list(res: &Value) {
    println!(
        "\n{:>6}  {:<7} {:>10}  {:<9} arquivo",
        "id", "conf.", "tamanho", "origem"
    );
    for i in res["items"].as_array().into_iter().flatten() {
        let name = match i["dir"].as_str() {
            Some(d) if !d.is_empty() => format!("{d}/{}", i["name"].as_str().unwrap_or("")),
            _ => i["name"].as_str().unwrap_or("").to_string(),
        };
        let conf = i["confidence"].as_str().unwrap_or("");
        let colored = match conf {
            "high" => paint("32", "alta  "),
            "medium" => paint("33", "média "),
            _ => paint("31", "baixa "),
        };
        println!(
            "{:>6}  {} {:>10}  {:<9} {}",
            i["id"],
            colored,
            human(i["size"].as_u64().unwrap_or(0)),
            i["origin"].as_str().unwrap_or(""),
            name
        );
    }
    println!("({} no total)", res["total"]);
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

fn paint(code: &str, s: &str) -> String {
    if std::io::stdout().is_terminal() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}
