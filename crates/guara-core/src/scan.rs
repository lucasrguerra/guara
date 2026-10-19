//! Orquestra a varredura: layout → metadados dos sistemas de arquivos →
//! carving no espaço livre/desconhecido → fusão sem duplicatas.

use crate::carve::{carve_range, CarveCfg, CarveEnd, CarveEvents, HitAction};
use crate::fs::{self, merge_extents, overlaps, Ctx};
use crate::partition;
use crate::signatures::category_for_ext;
use crate::source::Source;
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Progress {
    pub phase: String,
    pub done: u64,
    pub total: u64,
}

/// Receptor dos resultados (a interface/CLI implementa).
pub trait Sink {
    fn found(&mut self, f: Found);
    /// Um achado de carving confirmou um arquivo já listado por metadados.
    fn confirm(&mut self, id: u64, confidence: Confidence);
    /// Um achado já emitido ganhou dados melhores (ex.: JPEG remontado de fragmentos).
    fn replace(&mut self, f: Found) {
        self.found(f);
    }
    fn progress(&mut self, p: Progress);
    fn warning(&mut self, w: String);
    fn layout(&mut self, l: &DiskLayout);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeState {
    pub start: u64,
    pub end: u64,
    pub next: u64,
}

/// Estado serializável para pausar/retomar.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Checkpoint {
    pub fs_done: bool,
    pub ranges: Vec<RangeState>,
    pub next_id: u64,
    pub layout: Option<DiskLayout>,
    /// A varredura parou porque o dispositivo parou de responder (desconectado?).
    #[serde(default)]
    pub io_lost: bool,
}

pub enum Outcome {
    Completed,
    Interrupted(Checkpoint),
}

pub fn run(
    src: &dyn Source,
    opts: &ScanOptions,
    resume: Option<Checkpoint>,
    existing: &[Found],
    cancelled: &(dyn Fn() -> bool + Sync),
    sink: &mut dyn Sink,
) -> Outcome {
    let mut cp = resume.unwrap_or_default();
    let next_id = AtomicU64::new(
        cp.next_id
            .max(existing.iter().map(|f| f.id + 1).max().unwrap_or(1))
            .max(1),
    );

    sink.progress(Progress {
        phase: "Lendo tabela de partições".into(),
        done: 0,
        total: 0,
    });
    let layout = match cp.layout.clone() {
        Some(l) => l,
        None => {
            let l = partition::analyze(src, opts.mode == ScanMode::Deep);
            sink.layout(&l);
            for w in &l.warnings {
                sink.warning(w.clone());
            }
            l
        }
    };
    let mut layout = layout;
    if opts.skip_primary {
        layout.partitions.clear();
        layout.gaps.clear();
        layout.table = "membro de RAID".into();
    }
    // Volumes lógicos e outros volumes montados que não estão na tabela do disco.
    for p in &opts.extra_parts {
        if !layout.partitions.iter().any(|q| q.start == p.start) {
            let mut p = p.clone();
            p.index = layout.partitions.len();
            layout.partitions.push(p);
        }
    }
    cp.layout = Some(layout.clone());

    // Índices para fusão: primeiro offset -> (id, confiança), e extents de achados por metadados.
    let mut starts: HashMap<u64, (u64, Confidence)> = HashMap::new();
    let mut fs_extents: Vec<Extent> = vec![];
    for f in existing {
        index_found(f, &mut starts, &mut fs_extents);
    }

    if !cp.fs_done {
        let mut live_all: Vec<Extent> = vec![];
        let mut free_all: Vec<Extent> = vec![];
        let mut have_free_for: Vec<(u64, u64)> = vec![];
        for p in &layout.partitions {
            if p.encrypted.is_some() {
                continue;
            }
            let phase = format!("Lendo metadados {} (partição {})", p.fs.name(), p.index + 1);
            sink.progress(Progress {
                phase: phase.clone(),
                done: 0,
                total: 0,
            });
            let mut emit_buf: Vec<Found> = vec![];
            let res = {
                let progress_phase = phase.clone();
                let prog_cell = std::sync::Mutex::new((0u64, 0u64));
                let progress = |d: u64, t: u64| {
                    *prog_cell.lock().unwrap() = (d, t);
                };
                let mut emit = |f: Found| emit_buf.push(f);
                let mut ctx = Ctx {
                    src,
                    next_id: &next_id,
                    cancelled,
                    progress: &progress,
                    emit: &mut emit,
                };
                let r = match p.fs {
                    FsKind::Ntfs => Some(fs::ntfs::scan(&mut ctx, p.start)),
                    FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => {
                        Some(fs::fat::scan(&mut ctx, p.start, p.fs))
                    }
                    FsKind::Exfat => Some(fs::exfat::scan(&mut ctx, p.start)),
                    FsKind::Ext => Some(fs::ext::scan(&mut ctx, p.start, p.len)),
                    FsKind::HfsPlus => Some(fs::hfsplus::scan(&mut ctx, p.start, p.len)),
                    FsKind::Btrfs => Some(fs::btrfs::scan(&mut ctx, p.start, p.len)),
                    FsKind::Xfs => Some(fs::xfs::scan(&mut ctx, p.start, p.len)),
                    FsKind::Apfs => Some(fs::apfs::scan(&mut ctx, p.start, p.len)),
                    _ => None,
                };
                let _ = progress_phase;
                r
            };
            for f in emit_buf {
                index_found(&f, &mut starts, &mut fs_extents);
                sink.found(f);
            }
            if let Some(r) = res {
                for w in r.warnings {
                    sink.warning(w);
                }
                live_all.extend(r.live);
                if let Some(fr) = r.free {
                    free_all.extend(fr);
                    have_free_for.push((p.start, p.start + p.len));
                }
            }
            if cancelled() {
                cp.next_id = next_id.load(std::sync::atomic::Ordering::Relaxed);
                return Outcome::Interrupted(cp);
            }
        }
        cp.fs_done = true;
        cp.ranges = if opts.mode == ScanMode::Deep {
            build_ranges(
                &layout,
                &live_all,
                &free_all,
                &have_free_for,
                opts,
                src.size(),
            )
        } else {
            vec![]
        };
    }
    fs_extents = merge_extents(fs_extents);

    let total: u64 = cp.ranges.iter().map(|r| r.end - r.start).sum();
    let done: u64 = cp
        .ranges
        .iter()
        .map(|r| r.next.saturating_sub(r.start).min(r.end - r.start))
        .sum();
    let cfg = CarveCfg {
        max_size: opts.max_carve_size,
        categories: &opts.categories,
    };
    let mut handler = Handler {
        src,
        sink,
        starts,
        fs_extents,
        next_id: &next_id,
        done,
        total,
        index: crate::defrag::ClusterIndex::new(src.size()),
        pending: vec![],
        intact_mp4: vec![],
        refs: opts
            .references
            .iter()
            .filter_map(|p| {
                let s = crate::source::open_path(std::path::Path::new(p)).ok()?;
                crate::mp4rebuild::config_from_mp4(s.as_ref(), 0, s.size())
            })
            .collect(),
        carved: vec![],
    };
    for ri in 0..cp.ranges.len() {
        let (start, end) = (cp.ranges[ri].start, cp.ranges[ri].end);
        let mut next = cp.ranges[ri].next;
        if next >= end {
            continue;
        }
        let end_state = carve_range(src, start, end, &mut next, &cfg, cancelled, &mut handler);
        cp.ranges[ri].next = next;
        if end_state != CarveEnd::Done {
            cp.io_lost = end_state == CarveEnd::IoLost;
            cp.next_id = next_id.load(std::sync::atomic::Ordering::Relaxed);
            return Outcome::Interrupted(cp);
        }
    }
    handler.rebuild_pending();
    Outcome::Completed
}

/// Arquivo que parece quebrado: tenta recompor depois de varrer o disco todo.
struct Pending {
    found: Found,
    hit_len: u64,
    mp4: bool,
}

struct Handler<'a> {
    src: &'a dyn Source,
    sink: &'a mut dyn Sink,
    starts: HashMap<u64, (u64, Confidence)>,
    fs_extents: Vec<Extent>,
    next_id: &'a AtomicU64,
    done: u64,
    total: u64,
    index: crate::defrag::ClusterIndex,
    pending: Vec<Pending>,
    /// Vídeos íntegros achados no carving (início, tamanho): fontes de SPS/PPS.
    intact_mp4: Vec<(u64, u64)>,
    refs: Vec<crate::mp4rebuild::MediaConfig>,
    /// Extents de arquivos carved já emitidos (não podem servir de fragmento para outro).
    carved: Vec<Extent>,
}

impl Handler<'_> {
    fn rebuild_mp4(&mut self, p: Pending, off: u64) {
        // Referências: as informadas pelo usuário e os vídeos íntegros mais próximos no disco.
        let mut refs = self.refs.clone();
        let mut near = self.intact_mp4.clone();
        near.sort_by_key(|&(o, _)| o.abs_diff(off));
        for (o, l) in near.into_iter().take(6) {
            if let Some(c) = crate::mp4rebuild::config_from_mp4(self.src, o, l) {
                refs.push(c);
            }
        }
        let Some(r) = crate::mp4rebuild::rebuild(self.src, off, p.hit_len, &refs) else {
            return;
        };
        let Ok(pre) = crate::source::read_vec(self.src, off, r.pre_len as usize) else {
            return;
        };
        let payload_len = r.payload.1 - r.payload.0;
        let mut f = p.found;
        let hdr = r.mdat_header();
        f.size = pre.len() as u64 + hdr.len() as u64 + payload_len + r.moov.len() as u64;
        f.data = DataLoc::Parts {
            parts: vec![
                Part::Bytes { bytes: pre },
                Part::Bytes { bytes: hdr },
                Part::Run {
                    offset: r.payload.0,
                    len: payload_len,
                },
                Part::Bytes { bytes: r.moov },
            ],
        };
        f.ext = "mp4".into();
        f.name = format!("arquivo_{:06}.mp4", f.id);
        f.confidence = Confidence::Medium;
        f.note = Some(format!(
            "índice (moov) reconstruído: {} quadros {}x{} a {:.1} fps, sem áudio; ajuste de tempo aproximado",
            r.frames, r.width, r.height, r.fps
        ));
        self.carved.push(Extent::new(r.payload.0, payload_len));
        self.sink.replace(f);
    }

    fn rebuild_pending(&mut self) {
        let pend = std::mem::take(&mut self.pending);
        for p in pend {
            let off = p.found.first_offset().unwrap_or(0);
            if p.mp4 {
                self.rebuild_mp4(p, off);
                continue;
            }
            let mut used = self.fs_extents.clone();
            used.extend(self.carved.iter().copied());
            // O próprio começo do arquivo não conta como "ocupado" para ele mesmo.
            used.retain(|e| !(e.offset == off));
            let pool = crate::defrag::Pool {
                index: &self.index,
                used: &used,
            };
            if let crate::defrag::Analysis::Rebuilt { extents, size } =
                crate::defrag::analyze_jpeg(self.src, off, p.hit_len, &pool, true)
            {
                let mut f = p.found;
                f.size = size;
                f.data = DataLoc::Extents {
                    extents: extents.clone(),
                };
                f.confidence = Confidence::Medium;
                f.note = Some(format!(
                    "remontado a partir de {} fragmentos separados no disco; confira a imagem",
                    extents.len()
                ));
                self.carved.extend(extents);
                self.sink.replace(f);
            }
        }
    }
}

impl CarveEvents for Handler<'_> {
    fn chunk(&mut self, abs: u64, data: &[u8]) {
        self.index.scan_chunk(abs, data);
    }

    fn advance(&mut self, n: u64) {
        self.done += n;
        self.sink.progress(Progress {
            phase: "Varredura profunda por assinatura".into(),
            done: self.done.min(self.total),
            total: self.total,
        });
    }

    fn hit(&mut self, off: u64, hit: &crate::signatures::Hit) -> HitAction {
        if let Some(&(id, conf)) = self.starts.get(&off) {
            if hit.confidence > conf {
                self.sink.confirm(id, hit.confidence);
                self.starts.insert(off, (id, hit.confidence));
            }
            return HitAction::Skip;
        }
        if overlaps(&self.fs_extents, off, 1) {
            return HitAction::Continue; // dentro de arquivo já listado (ex.: miniatura embutida)
        }
        let mut hit = hit.clone();
        let mut skip_len = None;
        let mut pending_len = None;
        if hit.ext == "jpg" {
            let pool = crate::defrag::Pool {
                index: &self.index,
                used: &[],
            };
            match crate::defrag::analyze_jpeg(self.src, off, hit.len, &pool, false) {
                crate::defrag::Analysis::Intact { len } => {
                    hit.len = len;
                    hit.confidence = Confidence::High;
                }
                crate::defrag::Analysis::Broken { good_len } => {
                    pending_len = Some(hit.len);
                    hit.len = good_len;
                    hit.confidence = Confidence::Low;
                    skip_len = Some(good_len);
                }
                crate::defrag::Analysis::Unsupported | crate::defrag::Analysis::Rebuilt { .. } => {}
            }
        }
        let mut pending_mp4 = false;
        if matches!(hit.ext, "mp4" | "mov" | "m4v" | "3gp") {
            if hit.confidence == Confidence::High {
                self.intact_mp4.push((off, hit.len));
            } else {
                pending_mp4 = true;
                pending_len = Some(hit.len);
            }
        }
        let hit = &hit;
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let f = Found {
            id,
            name: format!("arquivo_{id:06}.{}", hit.ext),
            dir: None,
            ext: hit.ext.to_string(),
            category: category_for_ext(hit.ext),
            size: hit.len,
            origin: Origin::Carved,
            confidence: hit.confidence,
            data: DataLoc::Extents {
                extents: vec![Extent::new(off, hit.len)],
            },
            modified: None,
            created: None,
            accessed: None,
            taken: None,
            camera: None,
            px: None,
            gps: false,
            sig: None,
            note: if hit.confidence == Confidence::Low {
                Some("sem rodapé/estrutura completa: pode estar truncado".into())
            } else {
                None
            },
        };
        self.starts.insert(off, (id, hit.confidence));
        self.carved.push(Extent::new(off, hit.len));
        if let Some(hl) = pending_len {
            let mut pf = f.clone();
            pf.note = Some(if pending_mp4 {
                "vídeo sem índice (moov)".into()
            } else {
                "JPEG interrompido ou fragmentado".into()
            });
            self.pending.push(Pending {
                found: pf,
                hit_len: hl,
                mp4: pending_mp4,
            });
        }
        self.sink.found(f);
        match skip_len {
            Some(n) => HitAction::SkipLen(n),
            None => HitAction::Skip,
        }
    }
}

fn index_found(f: &Found, starts: &mut HashMap<u64, (u64, Confidence)>, ex: &mut Vec<Extent>) {
    if let DataLoc::Extents { extents } = &f.data {
        if let Some(o) = f.first_offset() {
            starts.insert(o, (f.id, f.confidence));
        }
        ex.extend(extents.iter().filter(|e| !e.sparse).copied());
    }
}

fn build_ranges(
    layout: &DiskLayout,
    live: &[Extent],
    free: &[Extent],
    have_free_for: &[(u64, u64)],
    opts: &ScanOptions,
    disk_size: u64,
) -> Vec<RangeState> {
    let live = merge_extents(live.to_vec());
    let free = merge_extents(free.to_vec());
    let mut out: Vec<Extent> = vec![];
    for p in &layout.partitions {
        if p.encrypted.is_some() || opts.covered.contains(&p.start) {
            continue;
        }
        let whole = Extent::new(p.start, p.len.min(disk_size.saturating_sub(p.start)));
        if opts.include_allocated {
            out.push(whole);
        } else if have_free_for
            .iter()
            .any(|&(s, e)| s == p.start && e == p.start + p.len)
        {
            out.extend(clip(&free, whole));
        } else if p.fs.has_metadata_scanner() {
            out.extend(subtract(whole, &live));
        } else {
            out.push(whole);
        }
    }
    for g in &layout.gaps {
        out.push(Extent::new(g.start, g.len));
    }
    if layout.partitions.is_empty() && layout.gaps.is_empty() {
        out.push(Extent::new(0, disk_size));
    }
    merge_extents(out)
        .into_iter()
        .filter(|e| e.len >= 4096)
        .map(|e| RangeState {
            start: e.offset,
            end: e.end(),
            next: e.offset,
        })
        .collect()
}

fn clip(sorted: &[Extent], window: Extent) -> Vec<Extent> {
    sorted
        .iter()
        .filter_map(|e| {
            let s = e.offset.max(window.offset);
            let t = e.end().min(window.end());
            (t > s).then(|| Extent::new(s, t - s))
        })
        .collect()
}

fn subtract(window: Extent, sorted_live: &[Extent]) -> Vec<Extent> {
    let mut out = vec![];
    let mut cur = window.offset;
    let i = sorted_live.partition_point(|e| e.end() <= window.offset);
    for e in &sorted_live[i..] {
        if e.offset >= window.end() {
            break;
        }
        if e.offset > cur {
            out.push(Extent::new(cur, e.offset - cur));
        }
        cur = cur.max(e.end());
    }
    if cur < window.end() {
        out.push(Extent::new(cur, window.end() - cur));
    }
    out
}
