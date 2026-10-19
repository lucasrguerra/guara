//! Recomposição de arquivos fragmentados no carving.
//!
//! Para JPEG: valida os bytes contíguos com o decodificador de entropia; se a imagem quebra,
//! procura entre os clusters "com cara de JPEG" próximos a continuação cujo DC de luminância
//! emenda sem salto, até três fragmentos. É o que dá para fazer sem tabela de alocação, e só
//! vale quando o sistema de arquivos não deixou metadados para o arquivo.

use crate::jpegscan::{self, Header, State, Stop};
use crate::source::{read_vec, Source};
use crate::types::Extent;

const MAX_FILE: usize = 48 << 20;
/// Janela de busca da continuação, em bytes, depois e antes do primeiro fragmento.
const WINDOW_AFTER: u64 = 512 << 20;
const WINDOW_BEFORE: u64 = 64 << 20;
const PROBE: usize = 8192;
pub const CLUSTER: u64 = 4096;
const MAX_PROBES: usize = 20_000;

/// Marca os clusters (de 4 KiB, alinhados ao início da origem) que parecem dados entropy-coded
/// de JPEG: sem marcadores no meio e com bytes bem distribuídos.
#[derive(Default, Clone)]
pub struct ClusterIndex {
    bits: Vec<u64>,
}

impl ClusterIndex {
    pub fn new(size: u64) -> Self {
        Self {
            bits: vec![0; (size / CLUSTER / 64 + 1) as usize],
        }
    }
    pub fn set(&mut self, off: u64) {
        let c = (off / CLUSTER) as usize;
        if let Some(w) = self.bits.get_mut(c / 64) {
            *w |= 1 << (c % 64);
        }
    }
    pub fn get(&self, off: u64) -> bool {
        let c = (off / CLUSTER) as usize;
        self.bits
            .get(c / 64)
            .map(|w| w >> (c % 64) & 1 == 1)
            .unwrap_or(false)
    }
    /// Examina um trecho lido e marca os clusters candidatos.
    pub fn scan_chunk(&mut self, abs: u64, buf: &[u8]) {
        let first = abs.div_ceil(CLUSTER) * CLUSTER;
        let mut o = first;
        while o + CLUSTER <= abs + buf.len() as u64 {
            let c = &buf[(o - abs) as usize..(o - abs + CLUSTER) as usize];
            if jpeg_like(c) {
                self.set(o);
            }
            o += CLUSTER;
        }
    }
    pub fn iter_range(&self, from: u64, to: u64) -> impl Iterator<Item = u64> + '_ {
        let (a, b) = (from.div_ceil(CLUSTER), to / CLUSTER);
        (a..b)
            .filter(move |&c| {
                self.bits
                    .get(c as usize / 64)
                    .map(|w| w >> (c % 64) & 1 == 1)
                    .unwrap_or(false)
            })
            .map(|c| c * CLUSTER)
    }
}

pub fn jpeg_like(c: &[u8]) -> bool {
    let mut seen = [false; 256];
    let mut distinct = 0;
    for (i, &b) in c.iter().enumerate() {
        if !seen[b as usize] {
            seen[b as usize] = true;
            distinct += 1;
        }
        if b == 0xFF {
            match c.get(i + 1) {
                Some(&n) if n == 0x00 || (0xD0..=0xD7).contains(&n) => {}
                Some(0xD9) if i + 2 >= c.len() - 16 => {}
                None => {}
                _ => return false,
            }
        }
    }
    distinct >= 190
}

pub struct Pool<'a> {
    pub index: &'a ClusterIndex,
    /// Extents já atribuídos a outros arquivos.
    pub used: &'a [Extent],
}

impl Pool<'_> {
    fn free_at(&self, off: u64, len: u64) -> bool {
        !self
            .used
            .iter()
            .any(|u| off < u.end() && off + len > u.offset)
    }
}

#[derive(Debug)]
pub enum Analysis {
    /// Contíguo e consistente; `len` inclui o EOI.
    Intact { len: u64 },
    /// Remontado a partir de fragmentos.
    Rebuilt { extents: Vec<Extent>, size: u64 },
    /// Só o início é confiável.
    Broken { good_len: u64 },
    /// Formato que não sabemos validar (progressivo etc.).
    Unsupported,
}

/// Procura FF D9 logo depois do último MCU (até 64 bytes de preenchimento).
fn eoi_after(data: &[u8], pos: usize) -> Option<usize> {
    let end = (pos + 64).min(data.len().saturating_sub(1));
    (pos.saturating_sub(2)..end)
        .find(|&i| data[i] == 0xFF && data[i + 1] == 0xD9)
        .map(|i| i + 2)
}

/// Valida o JPEG que começa em `off` e, se estiver quebrado, tenta recompô-lo.
pub fn analyze_jpeg(
    src: &dyn Source,
    off: u64,
    hit_len: u64,
    pool: &Pool,
    allow_rebuild: bool,
) -> Analysis {
    let read_len = (hit_len as usize).clamp(4096, MAX_FILE);
    let Ok(buf) = read_vec(src, off, read_len) else {
        return Analysis::Unsupported;
    };
    let Some(h) = jpegscan::parse_header(&buf) else {
        return Analysis::Unsupported;
    };
    let d = jpegscan::decode(&h, &buf, State::start(&h), u32::MAX, 512);
    let disc = jpegscan::first_discontinuity(&h, &d.dc);
    if d.stop == Stop::Done && disc.is_none() {
        if let Some(end) = eoi_after(&buf, d.state.pos) {
            return Analysis::Intact { len: end as u64 };
        }
    }
    let mut stop_pos = match d.stop {
        Stop::Marker(_, p) | Stop::Error(p) => p,
        Stop::Truncated | Stop::Done => buf.len(),
    };
    if let Some(m) = disc {
        let bad = d
            .snaps
            .iter()
            .rev()
            .find(|s| (s.mcu as usize) <= m)
            .map(|s| s.pos)
            .unwrap_or(h.scan_start);
        stop_pos = stop_pos.min(bad);
    }
    let cl = CLUSTER as usize;
    if off % CLUSTER != 0 || !allow_rebuild {
        return Analysis::Broken {
            good_len: (stop_pos / 512 * 512).max(512) as u64,
        };
    }

    let mut pieces: Vec<(u64, usize)> = vec![];
    let mut joined = buf;
    let mut cur_base = off; // início absoluto do fragmento atual
    let mut frag_start = 0usize; // posição em `joined` onde o fragmento atual começa
    let mut snaps = d.snaps;
    let mut suspect = stop_pos;

    for _ in 0..3 {
        let top = suspect.saturating_sub(frag_start) / cl * cl;
        let mut best: Option<(usize, u64, State, f64)> = None;
        let mut probes = 0usize;
        for back in 0..4usize {
            let Some(bc_rel) = top.checked_sub(back * cl).filter(|&b| b > 0) else {
                break;
            };
            let bc = frag_start + bc_rel;
            let Some(snap) = snaps
                .iter()
                .rev()
                .find(|s| s.pos <= bc)
                .cloned()
                .or_else(|| (frag_start == 0).then(|| State::start(&h)))
            else {
                continue;
            };
            let mut plane = jpegscan::Plane::new(&h);
            let _ =
                jpegscan::decode_plane(&h, &joined[..bc], State::start(&h), u32::MAX, &mut plane);
            let lo = off.saturating_sub(WINDOW_BEFORE);
            let hi = cur_base + bc_rel as u64 + WINDOW_AFTER;
            let pre = jpegscan::decode(&h, &joined[..bc], State::start(&h), u32::MAX, 0);
            let bpm_prefix =
                (pre.state.pos.saturating_sub(h.scan_start)) as f64 / pre.dc.len().max(1) as f64;
            let mut ranked: Vec<(f64, u64)> = vec![];
            for q in pool.index.iter_range(lo, hi) {
                if q >= cur_base && q < cur_base + bc_rel as u64 + CLUSTER {
                    continue;
                }
                if !pool.free_at(q, CLUSTER)
                    || pieces
                        .iter()
                        .any(|&(po, pl)| q < po + pl as u64 && q + CLUSTER > po)
                {
                    continue;
                }
                probes += 1;
                if probes > MAX_PROBES {
                    break;
                }
                if let Some(sc) =
                    probe_candidate(src, &h, &joined[..bc], &plane, &snap, q, bpm_prefix)
                {
                    ranked.push((sc, q));
                }
            }
            ranked.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            ranked.truncate(120);
            // Etapa 2: a continuação certa fecha a imagem exatamente no último MCU.
            let mut longest: Option<(usize, u64, f64)> = None;
            for (sc, q) in ranked {
                let avail = (src.size() - q) as usize;
                let more = read_vec(src, q, MAX_FILE.min(avail)).unwrap_or_default();
                let mut j2 = joined[..bc].to_vec();
                j2.extend_from_slice(&more);
                let d2 = jpegscan::decode(&h, &j2, snap.clone(), u32::MAX, 0);
                if d2.stop == Stop::Done && eoi_after(&j2, d2.state.pos).is_some() {
                    best = Some((bc, q, snap.clone(), 0.0));
                    break;
                }
                let reach = match d2.stop {
                    Stop::Marker(_, p) | Stop::Error(p) => p.saturating_sub(bc),
                    _ => j2.len() - bc,
                };
                if reach >= 3 * cl && longest.map_or(true, |l| sc < l.2) {
                    longest = Some((reach, q, sc));
                }
            }
            if best.is_none() {
                if let Some((_, q, sc)) = longest {
                    if sc < 25.0 {
                        let cand = (bc, q, snap.clone(), sc);
                        if best.as_ref().map_or(true, |b| sc < b.3) {
                            best = Some(cand);
                        }
                    }
                }
            }
            if best.as_ref().is_some_and(|b| b.3 == 0.0) {
                break;
            }
        }
        let Some((bc, q, snap, _)) = best else { break };
        pieces.push((cur_base, bc - frag_start));
        let more = read_vec(src, q, MAX_FILE.min((src.size() - q) as usize)).unwrap_or_default();
        joined.truncate(bc);
        joined.extend_from_slice(&more);
        let d2 = jpegscan::decode(&h, &joined, snap, u32::MAX, 512);
        if d2.stop == Stop::Done {
            if let Some(end) = eoi_after(&joined, d2.state.pos) {
                pieces.push((q, end - bc));
                return finish(pieces);
            }
        }
        // Quebrou de novo: onde?
        let mut sp = match d2.stop {
            Stop::Marker(_, p) | Stop::Error(p) => p,
            _ => joined.len(),
        };
        let all = jpegscan::decode(&h, &joined, State::start(&h), u32::MAX, 0);
        if let Some(m) = jpegscan::first_discontinuity(&h, &all.dc) {
            let bad = d2
                .snaps
                .iter()
                .rev()
                .find(|s| (s.mcu as usize) <= m)
                .map(|s| s.pos)
                .unwrap_or(bc);
            if bad > bc {
                sp = sp.min(bad);
            }
        }
        snaps = d2.snaps;
        cur_base = q;
        frag_start = bc;
        suspect = sp.max(bc + cl);
    }
    let good = pieces.iter().map(|p| p.1).sum::<usize>()
        + suspect.saturating_sub(frag_start).min(cl * 4) / 512 * 512;
    Analysis::Broken {
        good_len: (stop_pos.min(good.max(512)) / 512 * 512).max(512) as u64,
    }
}

fn finish(pieces: Vec<(u64, usize)>) -> Analysis {
    let mut extents: Vec<Extent> = vec![];
    for (o, l) in &pieces {
        match extents.last_mut() {
            Some(last) if last.end() == *o => last.len += *l as u64,
            _ => extents.push(Extent::new(*o, *l as u64)),
        }
    }
    let size = pieces.iter().map(|p| p.1 as u64).sum();
    if extents.len() == 1 {
        return Analysis::Intact { len: size };
    }
    Analysis::Rebuilt { extents, size }
}

/// Pontuação (menor é melhor) de `q` como continuação de `prefix`: diferença média de pixels
/// nas bordas entre os MCUs novos e seus vizinhos. Natural: poucos níveis de cinza; lixo: dezenas.
fn probe_candidate(
    src: &dyn Source,
    h: &Header,
    prefix: &[u8],
    plane: &jpegscan::Plane,
    snap: &State,
    q: u64,
    bpm_prefix: f64,
) -> Option<f64> {
    let more = read_vec(src, q, PROBE).ok()?;
    if more.len() < 1024 {
        return None;
    }
    let mut joined = Vec::with_capacity(prefix.len() + more.len());
    joined.extend_from_slice(prefix);
    joined.extend_from_slice(&more);
    let seam = plane.have;
    const K: u32 = 36;
    let mut news: std::collections::HashMap<u32, Vec<u8>> = std::collections::HashMap::new();
    let mut hook = |mcu: u32, blocks: &[[i32; 64]]| {
        if mcu >= seam {
            news.insert(mcu, jpegscan::mcu_luma(h, blocks));
        }
    };
    let want = seam.saturating_sub(snap.mcu) + K + 1;
    let d = jpegscan::decode_hook(h, &joined, snap.clone(), want, 0, Some(&mut hook));
    match d.stop {
        Stop::Error(_) => return None,
        Stop::Marker(m, _) if m != 0xD9 => return None,
        _ => {}
    }
    let (mw, mh) = (plane.mw, plane.mh);
    let mx = h.mcus_x;
    let get = |idx: u32| -> Option<Vec<u8>> {
        if idx < seam {
            let (x, y) = ((idx % mx) as usize, (idx / mx) as usize);
            let mut v = vec![0u8; mw * mh];
            for r in 0..mh {
                let o = (y * mh + r) * plane.w + x * mw;
                v[r * mw..(r + 1) * mw].copy_from_slice(&plane.px[o..o + mw]);
            }
            Some(v)
        } else {
            news.get(&idx).cloned()
        }
    };
    let (mut sum, mut n) = (0f64, 0u32);
    for idx in seam + 1..seam + 1 + K {
        let Some(cur) = news.get(&idx) else { break };
        if idx % mx > 0 {
            if let Some(l) = get(idx - 1) {
                for y in 0..mh {
                    sum += (cur[y * mw] as i32 - l[y * mw + mw - 1] as i32).abs() as f64;
                    n += 1;
                }
            }
        }
        if idx >= mx {
            if let Some(t) = get(idx - mx) {
                for x in 0..mw {
                    sum += (cur[x] as i32 - t[(mh - 1) * mw + x] as i32).abs() as f64;
                    n += 1;
                }
            }
        }
    }
    if n < 40 {
        return None;
    }
    // Bytes por MCU depois da emenda, comparados com os do trecho anterior.
    let after = d.dc.len() as f64 - (seam.saturating_sub(snap.mcu)) as f64;
    let used = d.state.pos.saturating_sub(prefix.len()) as f64;
    let ratio = if after > 4.0 && used > 0.0 {
        (used / after) / bpm_prefix.max(0.5)
    } else {
        1.0
    };
    let sc = sum / n as f64 + 20.0 * ratio.ln().abs();
    Some(sc)
}
