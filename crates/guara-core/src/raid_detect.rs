//! Descobre sozinho nível, ordem dos discos, tamanho de faixa e layout de um RAID sem
//! metadados, testando as combinações e medindo a coerência do sistema de arquivos que sai.

use crate::partition;
use crate::raid::{Layout, Level, Member, RaidParams, RaidSource};
use crate::source::{read_vec, MemSource, Source};
use crate::types::FsKind;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub level: Level,
    pub chunk: u64,
    pub layout: Layout,
    /// Índices dos membros fornecidos, na ordem do array.
    pub order: Vec<usize>,
    pub score: f64,
}

impl Candidate {
    pub fn params(&self, members: &[Arc<dyn Source>]) -> RaidParams {
        RaidParams::simple(
            self.level,
            self.chunk,
            self.layout,
            self.order.iter().map(|&i| Some(Member { src: members[i].clone(), data_offset: 0 })).collect(),
        )
    }
}

const PREFIX: u64 = 24 << 20;
const CHUNKS: [u64; 9] = [4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576];

fn permutations(n: usize) -> Vec<Vec<usize>> {
    fn rec(cur: &mut Vec<usize>, used: &mut Vec<bool>, n: usize, out: &mut Vec<Vec<usize>>) {
        if cur.len() == n {
            out.push(cur.clone());
            return;
        }
        for i in 0..n {
            if !used[i] {
                used[i] = true;
                cur.push(i);
                rec(cur, used, n, out);
                cur.pop();
                used[i] = false;
            }
        }
    }
    let mut out = vec![];
    rec(&mut vec![], &mut vec![false; n], n, &mut out);
    out
}

fn ntfs_score(src: &dyn Source, start: u64) -> f64 {
    let Ok(b) = read_vec(src, start, 512) else { return 0.0 };
    if b.len() < 512 || &b[3..11] != b"NTFS    " {
        return 0.0;
    }
    let bps = u16::from_le_bytes([b[11], b[12]]) as u64;
    let spc = b[13] as u64;
    let mft = u64::from_le_bytes(b[0x30..0x38].try_into().unwrap());
    let rs = b[0x40] as i8;
    let rec = if rs > 0 { rs as u64 * bps * spc } else { 1u64 << (-rs) };
    if bps == 0 || spc == 0 || rec == 0 || rec > 65536 {
        return 0.0;
    }
    let mft_off = start + mft * bps * spc;
    let (mut ok, mut total) = (0, 0);
    for k in [0u64, 1, 2, 3, 5, 11, 16, 24, 40, 100, 250, 600, 1500, 3000, 6000, 12000, 24000, 50000] {
        let off = mft_off + k * rec;
        if off + rec > src.size() {
            break;
        }
        let Ok(r) = read_vec(src, off, 64) else { continue };
        if r.len() < 64 {
            continue;
        }
        // Registros além do fim do MFT estão zerados: não contam contra.
        if r.iter().all(|&x| x == 0) {
            continue;
        }
        total += 1;
        if &r[..4] == b"FILE" && u32::from_le_bytes(r[0x2C..0x30].try_into().unwrap()) as u64 == k {
            ok += 1;
        }
    }
    if total == 0 {
        1.0
    } else {
        1.0 + 10.0 * ok as f64 / total as f64
    }
}

/// Coerência do volume que começa em `start`.
fn volume_score(src: &dyn Source, start: u64, fs: FsKind) -> f64 {
    match fs {
        FsKind::Ntfs => ntfs_score(src, start),
        FsKind::Ext => crate::fs::ext::consistency_score(src, start).unwrap_or(0.5),
        FsKind::Unknown => 0.0,
        _ => 1.0,
    }
}

pub fn fs_score(src: &dyn Source) -> f64 {
    let layout = partition::analyze(src, false);
    let mut best = 0.0f64;
    for p in &layout.partitions {
        best = best.max(volume_score(src, p.start, p.fs) + if p.fs != FsKind::Unknown { 0.5 } else { 0.0 });
    }
    best
}

fn first_sector_plausible(src: &dyn Source) -> bool {
    let Ok(b) = read_vec(src, 0, 4096) else { return false };
    b.len() >= 1100 && ((b[510] == 0x55 && b[511] == 0xAA) || partition::detect_fs_in(&b).is_some())
}

/// Testa as combinações e devolve as melhores (maior pontuação primeiro).
pub fn detect(members: &[Arc<dyn Source>], cancelled: &(dyn Fn() -> bool + Sync)) -> Vec<Candidate> {
    let n = members.len();
    if n < 2 {
        return vec![];
    }
    // Trabalha sobre um prefixo em memória de cada disco.
    let cached: Vec<Arc<dyn Source>> = members
        .iter()
        .map(|m| {
            let len = m.size().min(PREFIX) as usize;
            Arc::new(MemSource(read_vec(m.as_ref(), 0, len).unwrap_or_default())) as Arc<dyn Source>
        })
        .collect();
    let min_len = cached.iter().map(|c| c.size()).min().unwrap_or(0);
    if min_len < 1 << 20 {
        return vec![];
    }
    let mut out: Vec<Candidate> = vec![];

    // RAID 1: cópias idênticas.
    let probe = |s: &Arc<dyn Source>| read_vec(s.as_ref(), 0, 1 << 20).unwrap_or_default();
    let first = probe(&cached[0]);
    if first.iter().any(|&x| x != 0) && cached.iter().skip(1).all(|c| probe(c) == first) {
        let c = Candidate { level: Level::R1, chunk: 65536, layout: Layout::LeftSymmetric, order: (0..n).collect(), score: fs_score(cached[0].as_ref()) + 1.0 };
        out.push(c);
    }

    // RAID 5/4 satisfaz "XOR de todos os discos = 0" em toda faixa.
    let sample = 4usize << 20;
    let bufs: Vec<Vec<u8>> = cached.iter().map(|c| read_vec(c.as_ref(), 0, sample).unwrap_or_default()).collect();
    let mut xor_zero = 0usize;
    let mut nonzero_rows = 0usize;
    for blk in 0..sample / 4096 {
        let mut any = false;
        let mut z = true;
        for i in 0..4096 {
            let mut x = 0u8;
            for b in &bufs {
                x ^= b[blk * 4096 + i];
                any |= b[blk * 4096 + i] != 0;
            }
            if x != 0 {
                z = false;
                break;
            }
        }
        if any {
            nonzero_rows += 1;
            if z {
                xor_zero += 1;
            }
        }
    }
    let parity5 = nonzero_rows > 8 && xor_zero * 100 >= nonzero_rows * 95;

    let mut levels: Vec<Level> = vec![];
    if parity5 && n >= 3 {
        levels.push(Level::R5);
    } else {
        levels.push(Level::R0);
        if n >= 4 {
            levels.push(Level::R6);
        }
    }
    let ok_first: Vec<bool> = cached.iter().map(|c| first_sector_plausible(c.as_ref())).collect();
    let any_first = ok_first.iter().any(|&b| b);
    let perms = permutations(n);
    for level in levels {
        let layouts: Vec<Layout> = if matches!(level, Level::R0) { vec![Layout::LeftSymmetric] } else { Layout::all_raid5().to_vec() };
        for &chunk in &CHUNKS {
            for layout in &layouts {
                for order in &perms {
                    if cancelled() {
                        return finish(out);
                    }
                    let cand = Candidate { level, chunk, layout: *layout, order: order.clone(), score: 0.0 };
                    let params = cand.params(&cached);
                    let Ok(r) = RaidSource::new(params) else { continue };
                    // O primeiro setor do volume precisa estar num disco que o tenha.
                    if any_first {
                        let mut b = [0u8; 512];
                        let _ = r.read_at(0, &mut b);
                        let m0 = order.iter().position(|&i| {
                            let mut c = [0u8; 512];
                            let _ = cached[i].read_at(0, &mut c);
                            c == b
                        });
                        match m0 {
                            Some(p) if ok_first[order[p]] => {}
                            _ => continue,
                        }
                    }
                    let score = fs_score(&r);
                    if score > 0.0 {
                        out.push(Candidate { score, ..cand });
                    }
                }
            }
        }
    }
    finish(out)
}

fn finish(mut out: Vec<Candidate>) -> Vec<Candidate> {
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(10);
    out
}
