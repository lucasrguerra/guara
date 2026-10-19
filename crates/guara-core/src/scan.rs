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
        sink,
        starts,
        fs_extents,
        next_id: &next_id,
        done,
        total,
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
    Outcome::Completed
}

struct Handler<'a> {
    sink: &'a mut dyn Sink,
    starts: HashMap<u64, (u64, Confidence)>,
    fs_extents: Vec<Extent>,
    next_id: &'a AtomicU64,
    done: u64,
    total: u64,
}

impl CarveEvents for Handler<'_> {
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
        self.sink.found(f);
        HitAction::Skip
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
