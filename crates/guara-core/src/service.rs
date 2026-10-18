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
    Search,
    Hash,
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
    /// Leitura de datas EXIF/dimensões em andamento: (feitos, total).
    enrich: Option<(u64, u64)>,
    recoveries: Vec<recover::RecoverReport>,
    started_unix: Option<i64>,
    hits: Vec<Value>,
}

pub struct Job {
    id: String,
    kind: JobKind,
    target: String,
    opts: ScanOptions,
    cancel: AtomicBool,
    stop_requested: AtomicBool,
    src: Mutex<Option<Arc<dyn Source>>>,
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
            src: Mutex::new(src),
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
                enrich: None,
                recoveries: vec![],
                started_unix: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs() as i64),
                hits: vec![],
            }),
        })
    }
    fn source(&self) -> Option<Arc<dyn Source>> {
        self.src.lock().unwrap().clone()
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
    thumbs: Mutex<crate::explorer::ThumbCache>,
    gate: crate::explorer::Gate,
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
            thumbs: Mutex::new(crate::explorer::ThumbCache::new()),
            gate: crate::explorer::Gate::new(
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(2)
                    .min(6),
            ),
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
        if j.source().is_none() {
            return Err(no_source());
        }
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
            "source": j.source().is_some(),
            "hits": s.hits.len(),
            "enriching": s.enrich.map(|(d, t)| json!({ "done": d, "total": t })),
        }))
    }

    pub fn results(&self, id: &str, q: &Query) -> Result<Value> {
        let j = self.job(id)?;
        let s = j.st.lock().unwrap();
        let mut items: Vec<&Found> = q.select(&s.found);
        crate::explorer::sort_items(&mut items, q.sort.as_deref());
        let total = items.len();
        let page: Vec<Value> = items
            .into_iter()
            .skip(q.offset)
            .take(q.limit.clamp(1, 5000))
            .map(crate::explorer::item_json)
            .collect();
        Ok(json!({ "total": total, "items": page }))
    }

    /// Contagens para a navegação lateral (tipos, datas, pastas, câmeras).
    pub fn facets(&self, id: &str) -> Result<Value> {
        let j = self.job(id)?;
        let s = j.st.lock().unwrap();
        Ok(crate::explorer::facets(&s.found))
    }

    /// Metadados detalhados (EXIF, PDF, Office, vídeo…) de um arquivo.
    pub fn metadata(&self, id: &str, file: u64) -> Result<crate::meta::Meta> {
        let (f, src) = self.open_file(id, file)?;
        Ok(crate::meta::extract(src.as_ref(), &f))
    }

    /// Miniatura JPEG (cache em memória; decodificações limitadas em paralelo).
    pub fn thumb(&self, job: &str, id: u64, size: u32) -> std::result::Result<Arc<Vec<u8>>, u16> {
        let size = size.clamp(48, 640);
        let key = (job.to_string(), id, size);
        if let Some(v) = self.thumbs.lock().unwrap().get(&key) {
            return if v.is_empty() { Err(422) } else { Ok(v) };
        }
        let (f, src) = self.open_file(job, id).map_err(status_of)?;
        if f.category != Category::Image {
            return Err(415);
        }
        let _permit = self.gate.acquire();
        if let Some(v) = self.thumbs.lock().unwrap().get(&key) {
            return if v.is_empty() { Err(422) } else { Ok(v) };
        }
        let out = Arc::new(crate::thumb::thumbnail(src.as_ref(), &f, size).unwrap_or_default());
        self.thumbs.lock().unwrap().put(key, out.clone());
        if out.is_empty() {
            Err(422)
        } else {
            Ok(out)
        }
    }

    /// Achado + origem para leitura posicional (streaming de pré-visualização).
    pub fn open_file(&self, id: &str, file: u64) -> Result<(Found, Arc<dyn Source>)> {
        let j = self.job(id)?;
        let s = j.st.lock().unwrap();
        let i = *s
            .index
            .get(&file)
            .ok_or_else(|| Error::msg("arquivo não encontrado"))?;
        Ok((s.found[i].clone(), j.source().ok_or_else(no_source)?))
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
        let src = sj.source().ok_or_else(no_source)?;
        let files: Vec<Found> = {
            let s = sj.st.lock().unwrap();
            match (&ids, &filter) {
                (Some(ids), _) => ids
                    .iter()
                    .filter_map(|i| s.index.get(i).map(|&k| s.found[k].clone()))
                    .collect(),
                (None, Some(q)) => q.select(&s.found).into_iter().cloned().collect(),
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
        let parent = sj.clone();
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
                    parent.st.lock().unwrap().recoveries.push(rep.clone());
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

    // ---------------------------------------------------------------- exportação e relatório

    /// CSV dos achados que atendem ao filtro (ou todos). Devolve quantas linhas gravou.
    pub fn export_csv(&self, id: &str, path: &str, q: Option<&Query>) -> Result<usize> {
        let j = self.job(id)?;
        let s = j.st.lock().unwrap();
        let items: Vec<&Found> = match q {
            Some(q) => q.select(&s.found),
            None => s.found.iter().collect(),
        };
        crate::owner::write_owned(Path::new(path), crate::report::csv(&items))?;
        Ok(items.len())
    }

    /// Relatório HTML de recuperação (identificação do caso, origem, achados, hashes).
    pub fn export_report(
        &self,
        id: &str,
        path: &str,
        case: crate::report::CaseInfo,
        hashes: Option<crate::hashing::Hashes>,
    ) -> Result<()> {
        let j = self.job(id)?;
        let s = j.st.lock().unwrap();
        let elapsed = s
            .finished_in
            .unwrap_or_else(|| s.started.elapsed().as_secs_f64());
        let html = crate::report::html(&crate::report::ReportInput {
            case: &case,
            target: &j.target,
            options: &j.opts,
            layout: s.layout.as_ref(),
            found: &s.found,
            warnings: &s.warnings,
            started_unix: s.started_unix,
            elapsed,
            recoveries: &s.recoveries,
            source_hashes: hashes.as_ref(),
        });
        crate::owner::write_owned(Path::new(path), html)?;
        Ok(())
    }

    // ---------------------------------------------------------------- busca de texto e hash da origem

    pub fn start_search(&self, path: &str, opts: crate::search::SearchOpts) -> Result<String> {
        if opts.terms.iter().all(|t| t.trim().is_empty()) {
            return Err(Error::msg("informe pelo menos um termo"));
        }
        let src = open_path(Path::new(path)).map_err(|e| open_error(path, e))?;
        let job = self.register(
            JobKind::Search,
            path.to_string(),
            ScanOptions::default(),
            Some(src.clone()),
        );
        let j2 = job.clone();
        std::thread::spawn(move || {
            let layout = crate::partition::analyze(src.as_ref(), false);
            let cancelled = || j2.cancelled();
            let mut prog = |d: u64, t: u64| {
                j2.st.lock().unwrap().progress = Progress {
                    phase: "Procurando texto no disco".into(),
                    done: d,
                    total: t,
                };
            };
            let mut on_hit = |h: crate::search::Hit| {
                let part = layout
                    .partitions
                    .iter()
                    .find(|p| h.offset >= p.start && h.offset < p.start + p.len)
                    .map(|p| p.index + 1);
                let mut s = j2.st.lock().unwrap();
                s.hits.push(json!({
                    "offset": h.offset, "term": h.term, "encoding": h.encoding,
                    "context": h.context, "partition": part,
                }));
                true
            };
            crate::search::search(src.as_ref(), &opts, &cancelled, &mut prog, &mut on_hit);
            let mut s = j2.st.lock().unwrap();
            s.status = if j2.stop_requested.load(Ordering::Relaxed) {
                Status::Stopped
            } else {
                Status::Done
            };
            s.finished_in = Some(s.started.elapsed().as_secs_f64());
        });
        Ok(job.id.clone())
    }

    pub fn search_results(&self, id: &str, offset: usize, limit: usize) -> Result<Value> {
        let j = self.job(id)?;
        let s = j.st.lock().unwrap();
        Ok(json!({
            "total": s.hits.len(),
            "items": s.hits.iter().skip(offset).take(limit.clamp(1, 2000)).cloned().collect::<Vec<_>>(),
        }))
    }

    /// Calcula MD5/SHA-1/SHA-256 da origem inteira (verificação de integridade).
    pub fn start_hash(&self, path: &str) -> Result<String> {
        let src = open_path(Path::new(path)).map_err(|e| open_error(path, e))?;
        let job = self.register(
            JobKind::Hash,
            path.to_string(),
            ScanOptions::default(),
            None,
        );
        let j2 = job.clone();
        std::thread::spawn(move || {
            let size = src.size();
            let mut h = crate::hashing::MultiHasher::new();
            let mut buf = vec![0u8; 4 << 20];
            let mut pos = 0u64;
            let mut err = None;
            while pos < size && !j2.cancelled() {
                let want = ((size - pos) as usize).min(buf.len());
                match crate::source::read_exact_at(src.as_ref(), pos, &mut buf[..want]) {
                    Ok(()) => {
                        h.update(&buf[..want]);
                        pos += want as u64;
                    }
                    Err(e) => {
                        err = Some(format!("falha de leitura em {pos}: {e}. Para discos com defeito use a imagem forense."));
                        break;
                    }
                }
                j2.st.lock().unwrap().progress = Progress {
                    phase: "Calculando hashes da origem".into(),
                    done: pos,
                    total: size,
                };
            }
            let mut s = j2.st.lock().unwrap();
            if let Some(e) = err {
                s.error = Some(e);
                s.status = Status::Error;
            } else if pos < size {
                s.status = Status::Stopped;
            } else {
                s.report = serde_json::to_value(h.finalize()).ok();
                s.status = Status::Done;
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
            recoveries: s.recoveries.clone(),
            started_unix: s.started_unix,
        };
        crate::owner::write_owned(Path::new(path), serde_json::to_vec(&data)?)?;
        Ok(())
    }

    pub fn load_session(&self, path: &str) -> Result<String> {
        let bytes = std::fs::read(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::msg(format!(
                    "Sem permissão para ler a sessão {path}. Se ela foi criada como administrador (por exemplo em /root), abra o Guará como administrador ou copie o arquivo para a sua pasta."
                ))
            } else {
                Error::msg(format!("Não foi possível ler a sessão {path}: {e}"))
            }
        })?;
        let data: SessionFile = serde_json::from_slice(&bytes).map_err(|e| {
            Error::msg(format!(
                "O arquivo {path} não é uma sessão do Guará válida (ou foi criado por uma versão incompatível): {e}"
            ))
        })?;
        // A sessão abre mesmo sem a origem: dá para navegar, exportar e gerar relatório.
        // Pré-visualização e recuperação voltam quando a origem for reconectada.
        let (src, missing) = match open_path(Path::new(&data.target)) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(open_error(&data.target, e).to_string())),
        };
        let job = self.register(
            JobKind::Scan,
            data.target.clone(),
            data.options.clone(),
            src,
        );
        {
            let mut s = job.st.lock().unwrap();
            for (i, f) in data.found.iter().enumerate() {
                s.index.insert(f.id, i);
            }
            s.found = data.found;
            s.layout = data.layout;
            s.warnings = data.warnings;
            s.recoveries = data.recoveries;
            s.started_unix = data.started_unix.or(s.started_unix);
            s.checkpoint = data.checkpoint;
            s.status = if data.done {
                Status::Done
            } else {
                Status::Paused
            };
        }
        if let Some(m) = missing {
            job.st.lock().unwrap().warnings.push(format!(
                "A origem {} não está disponível agora ({m}). Você pode navegar, exportar e gerar relatório; para pré-visualizar e recuperar, reconecte o disco ou escolha a origem.",
                data.target
            ));
        }
        if data.done {
            spawn_enrich(job.clone());
        }
        Ok(job.id.clone())
    }

    /// Aponta uma sessão carregada para a origem (disco ou imagem) atual.
    pub fn relink(&self, id: &str, path: &str) -> Result<()> {
        let j = self.job(id)?;
        let src = open_path(Path::new(path)).map_err(|e| open_error(path, e))?;
        {
            let s = j.st.lock().unwrap();
            if let Some(l) = &s.layout {
                if l.size != 0 && l.size != src.size() {
                    return Err(Error::msg(format!(
                        "Essa origem tem {} bytes, mas a sessão foi feita sobre um disco de {} bytes. Escolha o mesmo disco ou uma imagem dele.",
                        src.size(),
                        l.size
                    )));
                }
            }
        }
        *j.src.lock().unwrap() = Some(src);
        j.st.lock()
            .unwrap()
            .warnings
            .retain(|w| !w.starts_with("A origem "));
        if j.st.lock().unwrap().status == Status::Done {
            spawn_enrich(j);
        }
        Ok(())
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
            "facets" => self.facets(&s("job")?)?,
            "open_external" => {
                let id = p
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| Error::msg("id ausente"))?;
                let (f, src) = self.open_file(&s("job")?, id)?;
                let path = crate::external::open_found(src.as_ref(), &f)?;
                json!({ "path": path.to_string_lossy() })
            }
            "export_csv" => {
                let q: Option<Query> = p
                    .get("query")
                    .cloned()
                    .filter(|v| !v.is_null())
                    .map(serde_json::from_value)
                    .transpose()?;
                json!({ "rows": self.export_csv(&s("job")?, &s("path")?, q.as_ref())? })
            }
            "export_report" => {
                let case: crate::report::CaseInfo = p
                    .get("case")
                    .cloned()
                    .filter(|v| !v.is_null())
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_default();
                let hashes: Option<crate::hashing::Hashes> = p
                    .get("hashes")
                    .cloned()
                    .filter(|v| !v.is_null())
                    .map(serde_json::from_value)
                    .transpose()?;
                self.export_report(&s("job")?, &s("path")?, case, hashes)?;
                json!({})
            }
            "search_start" => {
                let opts: crate::search::SearchOpts = serde_json::from_value(p.clone())?;
                json!({ "job": self.start_search(&s("path")?, opts)? })
            }
            "search_results" => self.search_results(
                &s("job")?,
                p.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                p.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as usize,
            )?,
            "hash_start" => json!({ "job": self.start_hash(&s("path")?)? }),
            "metadata" => json!(self.metadata(
                &s("job")?,
                p.get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| Error::msg("id ausente"))?
            )?),
            "results" => {
                let q: Query = p
                    .get("query")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_default();
                self.results(&s("job")?, &q)?
            }
            "settings_get" => json!(crate::settings::load()),
            "settings_set" => {
                let st: crate::settings::Settings = serde_json::from_value(p.clone())?;
                crate::settings::save(&st)?;
                json!({})
            }
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
            "session_relink" => {
                self.relink(&s("job")?, &s("path")?)?;
                json!({})
            }
            "session_load" => json!({ "job": self.load_session(&s("path")?)? }),
            "defaults" => {
                let home = crate::owner::user_home().to_string_lossy().to_string();
                json!({ "home": home, "sep": std::path::MAIN_SEPARATOR.to_string() })
            }
            "version" => {
                json!({ "version": env!("CARGO_PKG_VERSION"), "os": std::env::consts::OS })
            }
            other => return Err(Error::msg(format!("método desconhecido: {other}"))),
        })
    }
}

pub use crate::explorer::Query;

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
    #[serde(default)]
    recoveries: Vec<recover::RecoverReport>,
    #[serde(default)]
    started_unix: Option<i64>,
}

use crate::explorer::cat_key;

/// 503 quando a origem da sessão está desconectada; 404 nos demais casos.
fn status_of(e: Error) -> u16 {
    if e.to_string().contains("não está disponível") {
        503
    } else {
        404
    }
}

fn no_source() -> Error {
    Error::msg("A origem desta sessão não está disponível. Reconecte o disco ou escolha a origem para pré-visualizar e recuperar.")
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
        let Some(src) = job.source() else {
            let mut s = job.st.lock().unwrap();
            s.status = Status::Error;
            s.error = Some(no_source().to_string());
            return;
        };
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
        let done = s.status == Status::Done;
        drop(s);
        if done {
            spawn_enrich(job);
        }
    });
}

/// Lê EXIF/dimensões das imagens achadas (em segundo plano) para permitir
/// navegar por data, câmera e resolução.
fn spawn_enrich(job: Arc<Job>) {
    std::thread::spawn(move || {
        let Some(src) = job.source() else { return };
        let todo: Vec<(u64, Found)> = {
            let s = job.st.lock().unwrap();
            s.found
                .iter()
                .filter(|f| {
                    f.sig.is_none()
                        || (f.category == Category::Image && f.px.is_none() && f.taken.is_none())
                })
                .map(|f| (f.id, f.clone()))
                .collect()
        };
        if todo.is_empty() {
            return;
        }
        let total = todo.len() as u64;
        for (i, (id, f)) in todo.into_iter().enumerate() {
            if job.cancelled() && job.stop_requested.load(Ordering::Relaxed) {
                break;
            }
            let sig = crate::explorer::quick_sig(src.as_ref(), &f);
            let m = (f.category == Category::Image && f.px.is_none() && f.taken.is_none())
                .then(|| crate::meta::extract(src.as_ref(), &f));
            let mut s = job.st.lock().unwrap();
            if let Some(&k) = s.index.get(&id) {
                let t = &mut s.found[k];
                t.sig = sig;
                if let Some(m) = m {
                    t.taken = m.taken;
                    t.camera = m.camera.clone();
                    t.px = m.px.or(t.px);
                    t.gps = m.gps.is_some();
                }
            }
            s.enrich = Some((i as u64 + 1, total));
        }
        job.st.lock().unwrap().enrich = None;
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

/// Tipo MIME para servir a pré-visualização. Formatos que poderiam executar
/// script (HTML/SVG/XML) saem como texto puro.
pub fn mime_for(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Trecho de um arquivo achado, pronto para responder a uma requisição HTTP
/// (com suporte a `Range`) tanto no modo `guara ui` quanto no app Tauri.
pub struct FileSlice {
    found: Found,
    src: Arc<dyn Source>,
    pub start: u64,
    /// Inclusivo.
    pub end: u64,
    pub total: u64,
    pub partial: bool,
}

impl FileSlice {
    pub fn len(&self) -> u64 {
        if self.total == 0 {
            0
        } else {
            self.end - self.start + 1
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn status(&self) -> u16 {
        if self.partial {
            206
        } else {
            200
        }
    }
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut h = vec![
            ("Content-Type", mime_for(&self.found.ext).to_string()),
            ("Accept-Ranges", "bytes".into()),
            ("Cache-Control", "no-store".into()),
            ("X-Content-Type-Options", "nosniff".into()),
            // Conteúdo recuperado é não confiável: nunca executa script nem carrega nada.
            (
                "Content-Security-Policy",
                "sandbox; default-src 'none'".into(),
            ),
            ("Access-Control-Allow-Origin", "*".into()),
            (
                "Access-Control-Expose-Headers",
                "Content-Range, Content-Length, Accept-Ranges".into(),
            ),
            ("Content-Disposition", "inline".into()),
        ];
        if self.partial {
            h.push((
                "Content-Range",
                format!("bytes {}-{}/{}", self.start, self.end, self.total),
            ));
        }
        h
    }
    /// Lê o trecho inteiro em memória (limitado a `cap` bytes).
    pub fn read_all(&self, cap: u64) -> std::io::Result<Vec<u8>> {
        let n = self.len().min(cap) as usize;
        let mut out = vec![0u8; n];
        let mut got = 0;
        while got < n {
            let r = crate::recover::read_found_at(
                self.src.as_ref(),
                &self.found,
                self.start + got as u64,
                &mut out[got..],
            )?;
            if r == 0 {
                break;
            }
            got += r;
        }
        out.truncate(got);
        Ok(out)
    }
    pub fn reader(self) -> FileSliceReader {
        FileSliceReader {
            pos: self.start,
            slice: self,
        }
    }
}

pub struct FileSliceReader {
    slice: FileSlice,
    pos: u64,
}

impl std::io::Read for FileSliceReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos > self.slice.end || self.slice.total == 0 {
            return Ok(0);
        }
        let left = (self.slice.end - self.pos + 1) as usize;
        let n = buf.len().min(left).min(1 << 20);
        let r = crate::recover::read_found_at(
            self.slice.src.as_ref(),
            &self.slice.found,
            self.pos,
            &mut buf[..n],
        )?;
        self.pos += r as u64;
        Ok(r)
    }
}

impl Engine {
    /// Resolve `/file/<job>/<id>` + cabeçalho Range. `Err(status)` em caso de falha.
    pub fn file_slice(
        &self,
        job: &str,
        id: u64,
        range: Option<&str>,
    ) -> std::result::Result<FileSlice, u16> {
        let (found, src) = self.open_file(job, id).map_err(status_of)?;
        let total = found.size;
        let (mut start, mut end, mut partial) = (0u64, total.saturating_sub(1), false);
        if let Some(spec) = range.and_then(|r| r.strip_prefix("bytes=")) {
            let spec = spec.split(',').next().unwrap_or("");
            if let Some((a, b)) = spec.split_once('-') {
                let parsed = match (a.trim().parse::<u64>(), b.trim().parse::<u64>()) {
                    (Ok(a), Ok(b)) => Some((a, b.min(total.saturating_sub(1)))),
                    (Ok(a), Err(_)) if b.trim().is_empty() => Some((a, total.saturating_sub(1))),
                    (Err(_), Ok(n)) if a.trim().is_empty() && n > 0 => {
                        Some((total.saturating_sub(n), total.saturating_sub(1)))
                    }
                    _ => None,
                };
                if let Some((a, b)) = parsed {
                    if a >= total || a > b {
                        return Err(416);
                    }
                    start = a;
                    end = b;
                    partial = true;
                }
            }
        }
        Ok(FileSlice {
            found,
            src,
            start,
            end,
            total,
            partial,
        })
    }
}
