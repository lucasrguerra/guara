//! Camada de serviço usada pela interface gráfica (Tauri e modo `guara ui`):
//! jobs em segundo plano (varredura, recuperação, imagem) e despachante RPC.

use crate::device;
use crate::error::{Error, Result};
use crate::image;
use crate::partition;
use crate::recover;
use crate::scan::{self, Checkpoint, Outcome, Progress, Sink};
use crate::source::{open_path, Source};
use crate::types::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Paused,
    Done,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    Scan,
    Recover,
    Image,
}

struct State {
    status: Status,
    progress: Progress,
    found: Vec<Found>,
    index: HashMap<u64, usize>,
    warnings: Vec<String>,
    layout: Option<DiskLayout>,
    checkpoint: Option<Checkpoint>,
    error: Option<String>,
    report: Option<Value>,
    started: Instant,
    finished_in: Option<f64>,
}

pub struct Job {
    id: String,
    kind: JobKind,
    target: String,
    opts: ScanOptions,
    cancel: AtomicBool,
    stop_requested: AtomicBool,
    src: Option<Arc<dyn Source>>,
    st: Mutex<State>,
}

impl Job {
    fn new(
        id: String,
        kind: JobKind,
        target: String,
        opts: ScanOptions,
        src: Option<Arc<dyn Source>>,
    ) -> Arc<Job> {
        Arc::new(Job {
            id,
            kind,
            target,
            opts,
            cancel: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            src,
            st: Mutex::new(State {
                status: Status::Running,
                progress: Progress::default(),
                found: vec![],
                index: HashMap::new(),
                warnings: vec![],
                layout: None,
                checkpoint: None,
                error: None,
                report: None,
                started: Instant::now(),
                finished_in: None,
            }),
        })
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

struct JobSink(Arc<Job>);

impl Sink for JobSink {
    fn found(&mut self, f: Found) {
        let mut s = self.0.st.lock().unwrap();
        let i = s.found.len();
        s.index.insert(f.id, i);
        s.found.push(f);
    }
    fn confirm(&mut self, id: u64, c: Confidence) {
        let mut s = self.0.st.lock().unwrap();
        if let Some(&i) = s.index.get(&id) {
            s.found[i].confidence = s.found[i].confidence.max(c);
        }
    }
    fn progress(&mut self, p: Progress) {
        self.0.st.lock().unwrap().progress = p;
    }
    fn warning(&mut self, w: String) {
        let mut s = self.0.st.lock().unwrap();
        if !s.warnings.contains(&w) {
            s.warnings.push(w);
        }
    }
    fn layout(&mut self, l: &DiskLayout) {
        self.0.st.lock().unwrap().layout = Some(l.clone());
    }
}

pub struct Engine {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    counter: AtomicU64,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(1),
        }
    }

    fn job(&self, id: &str) -> Result<Arc<Job>> {
        self.jobs
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::msg(format!("job {id} não existe")))
    }

    fn register(
        &self,
        kind: JobKind,
        target: String,
        opts: ScanOptions,
        src: Option<Arc<dyn Source>>,
    ) -> Arc<Job> {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let id = format!("j{n}");
        let job = Job::new(id.clone(), kind, target, opts, src);
        self.jobs.lock().unwrap().insert(id, job.clone());
        job
    }

    // ---------------------------------------------------------------- varredura

    pub fn start_scan(&self, path: &str, opts: ScanOptions) -> Result<String> {
        let src = open_path(Path::new(path)).map_err(|e| open_error(path, e))?;
        let job = self.register(JobKind::Scan, path.to_string(), opts, Some(src));
        spawn_scan(job.clone(), None);
        Ok(job.id.clone())
    }

    pub fn pause(&self, id: &str) -> Result<()> {
        let j = self.job(id)?;
        j.cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn stop(&self, id: &str) -> Result<()> {
        let j = self.job(id)?;
        j.stop_requested.store(true, Ordering::Relaxed);
        j.cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn resume(&self, id: &str) -> Result<()> {
        let j = self.job(id)?;
        let cp = {
            let mut s = j.st.lock().unwrap();
            if s.status != Status::Paused {
                return Err(Error::msg("a varredura não está pausada"));
            }
            s.status = Status::Running;
            s.checkpoint.take()
        };
        j.cancel.store(false, Ordering::Relaxed);
        j.stop_requested.store(false, Ordering::Relaxed);
        spawn_scan(j, cp);
        Ok(())
    }

    pub fn status(&self, id: &str) -> Result<Value> {
        let j = self.job(id)?;
        let s = j.st.lock().unwrap();
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        let mut by_conf = [0usize; 3];
        for f in &s.found {
            *counts.entry(cat_key(f.category)).or_default() += 1;
            by_conf[f.confidence as usize] += 1;
        }
        let elapsed = s
            .finished_in
            .unwrap_or_else(|| s.started.elapsed().as_secs_f64());
        Ok(json!({
            "id": j.id,
            "kind": j.kind,
            "target": j.target,
            "status": s.status,
            "progress": s.progress,
            "found": s.found.len(),
            "categories": counts,
            "confidence": { "low": by_conf[0], "medium": by_conf[1], "high": by_conf[2] },
            "warnings": s.warnings,
            "layout": s.layout,
            "error": s.error,
            "report": s.report,
            "elapsed": elapsed,
        }))
    }

    pub fn results(&self, id: &str, q: &Query) -> Result<Value> {
        let j = self.job(id)?;
        let s = j.st.lock().unwrap();
        let needle = q
            .query
            .as_deref()
            .map(|x| x.to_lowercase())
            .filter(|x| !x.is_empty());
        let mut items: Vec<&Found> = s
            .found
            .iter()
            .filter(|f| q.category.map(|c| f.category == c).unwrap_or(true))
            .filter(|f| q.min_confidence.map(|c| f.confidence >= c).unwrap_or(true))
            .filter(|f| {
                needle
                    .as_ref()
                    .map(|n| f.full_path().to_lowercase().contains(n.as_str()))
                    .unwrap_or(true)
            })
            .collect();
        match q.sort.as_deref() {
            Some("name") => items.sort_by_key(|f| f.name.to_lowercase()),
            Some("size") => items.sort_by_key(|f| std::cmp::Reverse(f.size)),
            Some("confidence") => {
                items.sort_by(|a, b| b.confidence.cmp(&a.confidence).then(b.size.cmp(&a.size)))
            }
            _ => {}
        }
        let total = items.len();
        let page: Vec<Value> = items
            .into_iter()
            .skip(q.offset)
            .take(q.limit.clamp(1, 2000))
            .map(|f| {
                json!({
                    "id": f.id, "name": f.name, "dir": f.dir, "ext": f.ext, "category": f.category,
                    "size": f.size, "origin": f.origin, "confidence": f.confidence,
                    "modified": f.modified, "note": f.note,
                })
            })
            .collect();
        Ok(json!({ "total": total, "items": page }))
    }

    pub fn preview(&self, id: &str, file: u64) -> Result<Value> {
        let j = self.job(id)?;
        let (f, src) = {
            let s = j.st.lock().unwrap();
            let i = *s
                .index
                .get(&file)
                .ok_or_else(|| Error::msg("arquivo não encontrado"))?;
            (
                s.found[i].clone(),
                j.src.clone().ok_or_else(|| Error::msg("sem origem"))?,
            )
        };
        let mime = match f.ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            _ => return Ok(json!({ "mime": null })),
        };
        if f.size > 8 * 1024 * 1024 {
            return Ok(json!({ "mime": null, "reason": "grande demais para pré-visualizar" }));
        }
        let tmp =
            std::env::temp_dir().join(format!("guara-preview-{}-{}", std::process::id(), f.id));
        let r = recover::write_file(src.as_ref(), &f, &tmp);
        let bytes = std::fs::read(&tmp);
        let _ = std::fs::remove_file(&tmp);
        r?;
        Ok(json!({ "mime": mime, "base64": base64(&bytes?) }))
    }

    // ---------------------------------------------------------------- recuperação

    pub fn start_recover(
        &self,
        scan_id: &str,
        ids: Option<Vec<u64>>,
        filter: Option<Query>,
        dest: &str,
    ) -> Result<String> {
        let sj = self.job(scan_id)?;
        let src = sj.src.clone().ok_or_else(|| Error::msg("sem origem"))?;
        let files: Vec<Found> = {
            let s = sj.st.lock().unwrap();
            match (&ids, &filter) {
                (Some(ids), _) => ids
                    .iter()
                    .filter_map(|i| s.index.get(i).map(|&k| s.found[k].clone()))
                    .collect(),
                (None, Some(q)) => {
                    let needle = q
                        .query
                        .as_deref()
                        .map(|x| x.to_lowercase())
                        .filter(|x| !x.is_empty());
                    s.found
                        .iter()
                        .filter(|f| q.category.map(|c| f.category == c).unwrap_or(true))
                        .filter(|f| q.min_confidence.map(|c| f.confidence >= c).unwrap_or(true))
                        .filter(|f| {
                            needle
                                .as_ref()
                                .map(|n| f.full_path().to_lowercase().contains(n.as_str()))
                                .unwrap_or(true)
                        })
                        .cloned()
                        .collect()
                }
                (None, None) => s.found.clone(),
            }
        };
        if files.is_empty() {
            return Err(Error::msg("nenhum arquivo selecionado"));
        }
        let dest_path = PathBuf::from(dest);
        recover::check_destination(Some(Path::new(&sj.target)), &dest_path)?;
        let job = self.register(
            JobKind::Recover,
            dest.to_string(),
            ScanOptions::default(),
            None,
        );
        let j2 = job.clone();
        std::thread::spawn(move || {
            let cancelled = || j2.cancelled();
            let mut prog = |done: usize, total: usize, bytes: u64| {
                let mut s = j2.st.lock().unwrap();
                s.progress = Progress {
                    phase: format!("Recuperando ({bytes} bytes)"),
                    done: done as u64,
                    total: total as u64,
                };
            };
            let refs: Vec<&Found> = files.iter().collect();
            let res = recover::recover(src.as_ref(), &refs, &dest_path, &cancelled, &mut prog);
            let mut s = j2.st.lock().unwrap();
            match res {
                Ok(rep) => {
                    s.report = serde_json::to_value(&rep).ok();
                    s.status = if rep.cancelled {
                        Status::Stopped
                    } else {
                        Status::Done
                    };
                }
                Err(e) => {
                    s.error = Some(e.to_string());
                    s.status = Status::Error;
                }
            }
            s.finished_in = Some(s.started.elapsed().as_secs_f64());
        });
        Ok(job.id.clone())
    }

    // ---------------------------------------------------------------- imagem

    pub fn start_image(&self, path: &str, dest: &str) -> Result<String> {
        let src = open_path(Path::new(path)).map_err(|e| open_error(path, e))?;
        recover::check_destination(
            Some(Path::new(path)),
            Path::new(dest).parent().unwrap_or(Path::new(".")),
        )?;
        let job = self.register(
            JobKind::Image,
            dest.to_string(),
            ScanOptions::default(),
            None,
        );
        let j2 = job.clone();
        let dest = PathBuf::from(dest);
        std::thread::spawn(move || {
            let cancelled = || j2.cancelled();
            let mut prog = |d: u64, t: u64| {
                j2.st.lock().unwrap().progress = Progress {
                    phase: "Criando imagem do disco".into(),
                    done: d,
                    total: t,
                };
            };
            let res = image::create_image(src.as_ref(), &dest, &cancelled, &mut prog);
            let mut s = j2.st.lock().unwrap();
            match res {
                Ok(rep) => {
                    s.status = if rep.cancelled {
                        Status::Stopped
                    } else {
                        Status::Done
                    };
                    s.report = serde_json::to_value(&rep).ok();
                }
                Err(e) => {
                    s.error = Some(e.to_string());
                    s.status = Status::Error;
                }
            }
            s.finished_in = Some(s.started.elapsed().as_secs_f64());
        });
        Ok(job.id.clone())
    }

    // ---------------------------------------------------------------- sessões

    pub fn save_session(&self, id: &str, path: &str) -> Result<()> {
        let j = self.job(id)?;
        let s = j.st.lock().unwrap();
        let data = SessionFile {
            version: 1,
            target: j.target.clone(),
            options: j.opts.clone(),
            found: s.found.clone(),
            checkpoint: s.checkpoint.clone(),
            layout: s.layout.clone(),
            warnings: s.warnings.clone(),
            done: s.status == Status::Done,
        };
        std::fs::write(path, serde_json::to_vec(&data)?)?;
        Ok(())
    }

    pub fn load_session(&self, path: &str) -> Result<String> {
        let data: SessionFile = serde_json::from_slice(&std::fs::read(path)?)?;
        let src = open_path(Path::new(&data.target)).map_err(|e| open_error(&data.target, e))?;
        let job = self.register(
            JobKind::Scan,
            data.target.clone(),
            data.options.clone(),
            Some(src),
        );
        {
            let mut s = job.st.lock().unwrap();
            for (i, f) in data.found.iter().enumerate() {
                s.index.insert(f.id, i);
            }
            s.found = data.found;
            s.layout = data.layout;
            s.warnings = data.warnings;
            s.checkpoint = data.checkpoint;
            s.status = if data.done {
                Status::Done
            } else {
                Status::Paused
            };
        }
        Ok(job.id.clone())
    }

    // ---------------------------------------------------------------- RPC

    pub fn rpc(&self, method: &str, p: Value) -> Result<Value> {
        let s = |k: &str| -> Result<String> {
            p.get(k)
                .and_then(|v| v.as_str())
                .map(String::from)
                .ok_or_else(|| Error::msg(format!("parâmetro '{k}' ausente")))
        };
        Ok(match method {
            "devices" => json!(device::list_devices()),
            "inspect" => {
                let path = s("path")?;
                let src = open_path(Path::new(&path)).map_err(|e| open_error(&path, e))?;
                json!(partition::analyze(src.as_ref(), false))
            }
            "scan_start" => {
                let opts: ScanOptions = p
                    .get("options")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_default();
                json!({ "job": self.start_scan(&s("path")?, opts)? })
            }
            "scan_pause" => {
                self.pause(&s("job")?)?;
                json!({})
            }
            "scan_resume" => {
                self.resume(&s("job")?)?;
                json!({})
            }
            "scan_stop" | "job_stop" => {
                self.stop(&s("job")?)?;
                json!({})
            }
            "status" => self.status(&s("job")?)?,
            "results" => {
                let q: Query = p
                    .get("query")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_default();
                self.results(&s("job")?, &q)?
            }
            "preview" => self.preview(
                &s("job")?,
                p.get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| Error::msg("id ausente"))?,
            )?,
            "recover_start" => {
                let ids: Option<Vec<u64>> = p
                    .get("ids")
                    .cloned()
                    .filter(|v| !v.is_null())
                    .map(serde_json::from_value)
                    .transpose()?;
                let filter: Option<Query> = p
                    .get("filter")
                    .cloned()
                    .filter(|v| !v.is_null())
                    .map(serde_json::from_value)
                    .transpose()?;
                json!({ "job": self.start_recover(&s("job")?, ids, filter, &s("dest")?)? })
            }
            "image_start" => json!({ "job": self.start_image(&s("path")?, &s("dest")?)? }),
            "session_save" => {
                self.save_session(&s("job")?, &s("path")?)?;
                json!({})
            }
            "session_load" => json!({ "job": self.load_session(&s("path")?)? }),
            "defaults" => {
                let home = std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_default();
                json!({ "home": home, "sep": std::path::MAIN_SEPARATOR.to_string() })
            }
            "version" => {
                json!({ "version": env!("CARGO_PKG_VERSION"), "os": std::env::consts::OS })
            }
            other => return Err(Error::msg(format!("método desconhecido: {other}"))),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    #[serde(default)]
    pub category: Option<Category>,
    #[serde(default)]
    pub min_confidence: Option<Confidence>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    200
}

impl Default for Query {
    fn default() -> Self {
        Self {
            category: None,
            min_confidence: None,
            query: None,
            sort: None,
            offset: 0,
            limit: 200,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    target: String,
    options: ScanOptions,
    found: Vec<Found>,
    checkpoint: Option<Checkpoint>,
    layout: Option<DiskLayout>,
    warnings: Vec<String>,
    done: bool,
}

fn cat_key(c: Category) -> &'static str {
    match c {
        Category::Image => "image",
        Category::Document => "document",
        Category::Video => "video",
        Category::Audio => "audio",
        Category::Archive => "archive",
        Category::Database => "database",
        Category::Other => "other",
    }
}

fn open_error(path: &str, e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        Error::msg(format!(
            "Sem permissão para ler {path}. Discos exigem privilégios de administrador: no Linux/macOS rode com sudo; no Windows, execute como administrador."
        ))
    } else {
        Error::msg(format!("Não foi possível abrir {path}: {e}"))
    }
}

fn spawn_scan(job: Arc<Job>, cp: Option<Checkpoint>) {
    std::thread::spawn(move || {
        let src = job.src.clone().expect("origem");
        let existing: Vec<Found> = job.st.lock().unwrap().found.clone();
        let cancelled = || job.cancelled();
        let mut sink = JobSink(job.clone());
        let outcome = scan::run(
            src.as_ref(),
            &job.opts,
            cp,
            &existing,
            &cancelled,
            &mut sink,
        );
        let mut s = job.st.lock().unwrap();
        match outcome {
            Outcome::Completed => {
                s.status = Status::Done;
                s.checkpoint = None;
            }
            Outcome::Interrupted(cp) => {
                if job.stop_requested.load(Ordering::Relaxed) {
                    s.status = Status::Stopped;
                } else {
                    s.status = Status::Paused;
                    s.checkpoint = Some(cp);
                }
            }
        }
        s.finished_in = Some(s.started.elapsed().as_secs_f64());
    });
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
