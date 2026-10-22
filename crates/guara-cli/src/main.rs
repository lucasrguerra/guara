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
        #[command(flatten)]
        asm: AsmArgs,
    },
    /// Saúde do disco (SMART): estado, temperatura, uso e tabela de atributos
    Health {
        /// Dispositivo ou partição (o SMART lido é o do disco que a contém)
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
    Image {
        path: String,
        dest: String,
        /// Formato da imagem: raw (padrão), e01 (Expert Witness) ou aff4 (AFF4 v1.0), com hash e metadados
        #[arg(long, default_value = "raw")]
        format: String,
        /// E01: número do caso
        #[arg(long, default_value = "")]
        case: String,
        /// E01: examinador
        #[arg(long, default_value = "")]
        examiner: String,
        /// E01: descrição da evidência
        #[arg(long, default_value = "")]
        description: String,
        /// E01: número da evidência
        #[arg(long, default_value = "")]
        evidence: String,
        /// E01: observações
        #[arg(long, default_value = "")]
        notes: String,
        /// E01: tamanho máximo de cada segmento em MiB (0 = um arquivo só)
        #[arg(long, default_value_t = 1500)]
        segment_mib: u64,
        /// E01: compressão 0 (nenhuma) a 9
        #[arg(long, default_value_t = 1)]
        compression: u32,
    },
    /// Confere uma imagem E01/Ex01: recalcula o MD5 e compara com o gravado na própria imagem
    Verify { path: String },
    /// Mostra a trilha de auditoria (quem fez o quê e quando) ou confere o encadeamento por hash
    Audit {
        /// Arquivo da trilha (padrão: auditoria.jsonl na pasta de configuração ou GUARA_AUDIT_LOG)
        #[arg(long)]
        log: Option<String>,
        /// Só confere a cadeia de hashes; sai com erro se ela estiver quebrada
        #[arg(long)]
        verify: bool,
        /// Mostra só as últimas N entradas
        #[arg(long)]
        tail: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Exporta os achados de uma sessão para CSV (abre no Excel)
    Export {
        session: String,
        /// Arquivo CSV de saída
        csv: String,
        #[command(flatten)]
        filter: FilterArgs,
    },
    /// Gera o relatório de recuperação (identificação do caso, origem, achados, hashes): PDF se o arquivo terminar em .pdf, senão HTML
    Report {
        session: String,
        /// Arquivo de saída (.pdf ou .html)
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
    /// Lista as sessões salvas automaticamente
    Sessions,
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
    /// Como organizar o que for recuperado: folders (pastas originais), type, date ou flat
    #[arg(long, default_value = "folders")]
    organize: String,
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

/// Montagem do disco: volumes cifrados, RAID e LVM.
#[derive(clap::Args, Clone, Default)]
struct AsmArgs {
    /// Senha de volume cifrado (BitLocker, LUKS, FileVault 2). Prefira --ask-password ou GUARA_PASSWORD
    #[arg(long = "password")]
    passwords: Vec<String>,
    /// Pergunta a senha no terminal, sem mostrar o que é digitado
    #[arg(long)]
    ask_password: bool,
    /// Chave de recuperação numérica do BitLocker (8 grupos de 6 dígitos)
    #[arg(long = "recovery-key")]
    recovery_keys: Vec<String>,
    /// Arquivo de chave (.BEK do BitLocker ou keyfile do LUKS)
    #[arg(long = "key-file")]
    key_files: Vec<String>,
    /// Chave do volume em hexadecimal (a que o Guará mostra ao abrir um volume)
    #[arg(long = "volume-key")]
    volume_keys: Vec<String>,
    /// Outro disco ou imagem do mesmo conjunto (RAID/LVM). Pode repetir; "-" marca um disco ausente
    #[arg(long = "member")]
    members: Vec<String>,
    /// Os discos (alvo + --member, nesta ordem) formam um RAID sem metadados: descobre sozinho nível, faixa e layout
    #[arg(long)]
    raid: bool,
    /// Fixa o nível do RAID: 0, 5, 6, 1
    #[arg(long)]
    raid_level: Option<String>,
    /// Fixa o tamanho da faixa do RAID em KiB
    #[arg(long)]
    raid_chunk_kb: Option<u64>,
    /// Fixa o layout: left-symmetric, left-asymmetric, right-symmetric, right-asymmetric
    #[arg(long)]
    raid_layout: Option<String>,
    /// Ordem dos discos no RAID (índices; 0 = alvo)
    #[arg(long, value_delimiter = ',')]
    raid_order: Vec<usize>,
    /// Guarda na sessão as chaves dos volumes abertos (a senha nunca é guardada)
    #[arg(long)]
    remember_keys: bool,
    /// Vídeo íntegro da mesma câmera/drone para refazer o índice de MP4 sem moov (pode repetir)
    #[arg(long = "reference")]
    references: Vec<String>,
}

impl AsmArgs {
    fn assembly(&self) -> Result<guara_core::assemble::Assembly, String> {
        use guara_core::assemble::{Assembly, RaidSpec, SecretInput};
        let mut secrets = vec![];
        for p in &self.passwords {
            secrets.push(SecretInput::Password { value: p.clone() });
        }
        if let Ok(p) = std::env::var("GUARA_PASSWORD") {
            if !p.is_empty() {
                secrets.push(SecretInput::Password { value: p });
            }
        }
        if self.ask_password {
            let p = rpassword::prompt_password("Senha do volume: ").map_err(s)?;
            secrets.push(SecretInput::Password { value: p });
        }
        for r in &self.recovery_keys {
            secrets.push(SecretInput::Recovery { value: r.clone() });
        }
        for k in &self.key_files {
            secrets.push(SecretInput::Keyfile { path: k.clone() });
        }
        for k in &self.volume_keys {
            secrets.push(SecretInput::Key { hex: k.clone() });
        }
        let raid = if self.raid
            || self.raid_level.is_some()
            || self.raid_chunk_kb.is_some()
            || self.raid_layout.is_some()
            || !self.raid_order.is_empty()
        {
            let level = match self.raid_level.as_deref() {
                None => None,
                Some("0") => Some(guara_core::raid::Level::R0),
                Some("1") => Some(guara_core::raid::Level::R1),
                Some("5") => Some(guara_core::raid::Level::R5),
                Some("6") => Some(guara_core::raid::Level::R6),
                Some(x) => {
                    return Err(format!(
                        "nível de RAID não suportado: {x} (use 0, 1, 5 ou 6)"
                    ))
                }
            };
            let layout = match self.raid_layout.as_deref() {
                None => None,
                Some("left-symmetric") => Some(guara_core::raid::Layout::LeftSymmetric),
                Some("left-asymmetric") => Some(guara_core::raid::Layout::LeftAsymmetric),
                Some("right-symmetric") => Some(guara_core::raid::Layout::RightSymmetric),
                Some("right-asymmetric") => Some(guara_core::raid::Layout::RightAsymmetric),
                Some(x) => return Err(format!("layout desconhecido: {x}")),
            };
            Some(RaidSpec {
                level,
                chunk_kb: self.raid_chunk_kb,
                layout,
                order: (!self.raid_order.is_empty()).then(|| self.raid_order.clone()),
            })
        } else {
            None
        };
        Ok(Assembly {
            raid,
            members: self.members.iter().filter(|m| *m != "-").cloned().collect(),
            secrets,
            remember_keys: self.remember_keys,
            keys: vec![],
        })
    }
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
    asm: AsmArgs,
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
        Cmd::Inspect { path, json, asm } => {
            let a = asm.assembly()?;
            let v = eng
                .rpc(
                    "inspect",
                    json!({ "path": path, "assembly": serde_json::to_value(&a).map_err(s)?}),
                )
                .map_err(s)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                print_layout(&v);
            }
            Ok(())
        }
        Cmd::Health { path, json } => {
            let v = eng.rpc("health", json!({ "path": path })).map_err(s)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                print_health(&v);
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
                assembly: a.asm.assembly()?,
                references: a.asm.references.clone(),
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
                    organize: "folders".into(),
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
        Cmd::Verify { path } => {
            let r = verify_image(&path);
            guara_core::audit::record(
                "verify",
                json!({ "path": path, "result": match &r { Ok(Some(true)) => "conferem", Ok(Some(false)) => "diferem", Ok(None) => "sem hash gravado", Err(_) => "erro" } }),
            );
            r.map(|_| ())
        }
        Cmd::Audit {
            log,
            verify,
            tail,
            json,
        } => audit(log, verify, tail, json),
        Cmd::Image {
            path,
            dest,
            format,
            case,
            examiner,
            description,
            evidence,
            notes,
            segment_mib,
            compression,
        } => {
            let fmt = match format.as_str() {
                "raw" => guara_core::image::ImageFormat::Raw,
                "e01" => guara_core::image::ImageFormat::E01 {
                    case_number: case,
                    description,
                    evidence_number: evidence,
                    examiner,
                    notes,
                    segment_mib,
                    compression,
                },
                "aff4" => guara_core::image::ImageFormat::Aff4 {
                    case_number: case,
                    examiner,
                    evidence_number: evidence,
                    notes,
                },
                x => {
                    return Err(format!(
                        "formato de imagem desconhecido: {x} (use raw, e01 ou aff4)"
                    ))
                }
            };
            let job = eng.start_image_as(&path, &dest, &fmt).map_err(s)?;
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
        Cmd::Sessions => {
            let list = eng.rpc("sessions_list", json!({})).map_err(s)?;
            let items = list.as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                println!("Nenhuma sessão salva automaticamente ainda.");
            }
            for i in items {
                println!(
                    "{:<10} {:>8} arquivos  {:<22} {}",
                    i["status"].as_str().unwrap_or(""),
                    i["found"],
                    i["target"].as_str().unwrap_or(""),
                    i["path"].as_str().unwrap_or("")
                );
            }
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
    let layout = guara_core::recover::RecoverLayout::parse(&filter.organize).ok_or(format!(
        "organização inválida: {} (use folders, type, date ou flat)",
        filter.organize
    ))?;
    let rj = eng
        .start_recover(scan_job, ids, Some(q), dest, layout)
        .map_err(s)?;
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

/// Confere o hash gravado numa imagem E01/Ex01 ou AFF4. `Some(true)` = confere.
fn verify_image(path: &str) -> Result<Option<bool>, String> {
    if path.to_ascii_lowercase().ends_with(".aff4") {
        let a = guara_core::aff4::Aff4Source::open(std::path::Path::new(path)).map_err(s)?;
        println!("Formato: AFF4");
        for (k, v) in &a.info {
            println!("{k}: {v}");
        }
        eprintln!("Recalculando o hash...");
        return match a.verify_stream_md5().map_err(s)? {
            Some(true) => {
                println!("MD5 gravado e recalculado conferem.");
                Ok(Some(true))
            }
            Some(false) => Err("O MD5 recalculado difere do gravado na imagem.".into()),
            None => {
                println!("Esta imagem não traz MD5 para conferir.");
                Ok(None)
            }
        };
    }
    let e = guara_core::ewf::EwfSource::open(std::path::Path::new(path)).map_err(s)?;
    println!(
        "Formato: {}",
        if e.info.version == 2 {
            "Ex01 (EnCase 7)"
        } else {
            "E01 (EnCase)"
        }
    );
    println!("Segmentos: {}", e.info.segments);
    for (k, v) in &e.info.fields {
        println!("{k}: {v}");
    }
    eprintln!("Recalculando o hash...");
    match e.verify().map_err(s)? {
        Some(true) => {
            println!(
                "MD5 gravado e recalculado conferem: {}",
                e.info.md5.clone().unwrap_or_default()
            );
            Ok(Some(true))
        }
        Some(false) => Err(
            "O MD5 recalculado difere do gravado na imagem: ela está corrompida ou foi alterada."
                .into(),
        ),
        None => {
            println!("Esta imagem não traz hash gravado para conferir.");
            Ok(None)
        }
    }
}

/// `guara audit`: lista a trilha ou confere a cadeia de hashes.
fn audit(log: Option<String>, verify: bool, tail: Option<usize>, json: bool) -> Result<(), String> {
    let log = match log {
        Some(p) => guara_core::audit::Log::at(p),
        None => guara_core::audit::Log::open_default(),
    };
    let v = log.verify().map_err(s)?;
    if verify {
        if json {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
        } else if v.ok {
            println!("Cadeia íntegra: {} entradas.", v.entries);
            if v.entries > 0 {
                println!(
                    "Hash da última entrada (guarde-o fora desta máquina): {}",
                    v.head
                );
            }
        }
        return match v.broken_at {
            None => Ok(()),
            Some((n, why)) => Err(format!("cadeia quebrada na entrada {n}: {why}")),
        };
    }
    let mut entries = log.entries().map_err(s)?;
    if let Some(n) = tail {
        let skip = entries.len().saturating_sub(n);
        entries.drain(..skip);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
    } else if entries.is_empty() {
        println!("Trilha vazia ({}).", log.path().display());
    } else {
        for e in &entries {
            let who = match &e.examiner {
                Some(x) => format!("{}/{x}", e.user),
                None => e.user.clone(),
            };
            println!(
                "#{:<4} {}  {who}@{}  {}  {}",
                e.seq, e.time, e.host, e.action, e.detail
            );
        }
    }
    if !v.ok {
        let (n, why) = v.broken_at.unwrap_or_default();
        eprintln!(
            "{} cadeia quebrada na entrada {n}: {why}",
            paint("31", "atenção:")
        );
    }
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

fn print_health(v: &Value) {
    let st = v["status"].as_str().unwrap_or("unknown");
    if !v["available"].as_bool().unwrap_or(false) {
        println!("{}", v["summary"].as_str().unwrap_or(""));
        return;
    }
    let word = match st {
        "ok" => paint("32", "Bom"),
        "warning" => paint("33", "Atenção"),
        "failing" => paint("31", "Ruim"),
        _ => "?".into(),
    };
    let pct = v["health_pct"]
        .as_u64()
        .map(|p| format!(" ({p}%)"))
        .unwrap_or_default();
    println!("Disco: {}", v["disk"].as_str().unwrap_or(""));
    if let Some(p) = v["partition"].as_str() {
        println!("  (o SMART é do disco inteiro que contém {p})");
    }
    println!(
        "Estado: {word}{pct}  {}",
        v["summary"].as_str().unwrap_or("")
    );
    let num = |k: &str| v[k].as_i64();
    let bytes = |k: &str| v[k].as_u64().map(human);
    let mut m = Vec::new();
    if let Some(t) = num("temperature_c") {
        m.push(format!("Temperatura: {t} °C"));
    }
    if let Some(h) = num("power_on_hours") {
        m.push(format!("Horas ligado: {h}"));
    }
    if let Some(c) = num("power_cycles") {
        m.push(format!("Vezes ligado: {c}"));
    }
    if let Some(r) = bytes("host_reads") {
        m.push(format!("Total lido: {r}"));
    }
    if let Some(w) = bytes("host_writes") {
        m.push(format!("Total gravado: {w}"));
    }
    if !m.is_empty() {
        println!("{}", m.join(" · "));
    }
    if st != "ok" {
        for kv in v["details"].as_array().into_iter().flatten() {
            println!(
                "  ! {}: {}",
                kv[0].as_str().unwrap_or(""),
                kv[1].as_str().unwrap_or("")
            );
        }
    }
    println!();
    for kv in v["info"].as_array().into_iter().flatten() {
        println!(
            "  {:<24}{}",
            kv[0].as_str().unwrap_or(""),
            kv[1].as_str().unwrap_or("")
        );
    }
    let attrs = v["attributes"].as_array().cloned().unwrap_or_default();
    if !attrs.is_empty() {
        let o = |x: &Value| {
            x.as_i64()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".into())
        };
        println!(
            "\n     ID  {:<34}{:>6}{:>6}{:>7}  Bruto",
            "Atributo", "Atual", "Pior", "Limite"
        );
        for a in attrs {
            let dot = match a["status"].as_str() {
                Some("failing") => paint("31", "●"),
                Some("warning") => paint("33", "●"),
                _ => paint("32", "●"),
            };
            println!(
                "  {dot}  {:<3} {:<34}{:>6}{:>6}{:>7}  {}",
                a["id"].as_str().unwrap_or(""),
                a["name"].as_str().unwrap_or(""),
                o(&a["value"]),
                o(&a["worst"]),
                o(&a["thresh"]),
                a["raw"].as_str().unwrap_or("")
            );
        }
    }
    for adv in v["advice"].as_array().into_iter().flatten() {
        println!("\n{}", adv.as_str().unwrap_or(""));
    }
}
