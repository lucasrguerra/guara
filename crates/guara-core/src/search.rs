//! Busca de texto bruta em todo o disco (inclusive espaço livre e não particionado),
//! em UTF-8 e UTF-16LE, útil quando não há estrutura de arquivo para recuperar.

use crate::source::Source;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOpts {
    pub terms: Vec<String>,
    /// Ignora maiúsculas/minúsculas (letras ASCII).
    #[serde(default = "yes")]
    pub ignore_case: bool,
    /// Procura também a codificação UTF-16 (Windows, Office, bancos de dados).
    #[serde(default = "yes")]
    pub utf16: bool,
    #[serde(default = "default_max")]
    pub max_hits: usize,
}

fn yes() -> bool {
    true
}
fn default_max() -> usize {
    20_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub offset: u64,
    pub term: String,
    pub encoding: String,
    pub context: String,
}

const CHUNK: usize = 8 * 1024 * 1024;
const CTX: usize = 48;

struct Pat {
    term: String,
    enc: &'static str,
    bytes: Vec<u8>,
    skip: [usize; 256],
}

fn make_pats(o: &SearchOpts) -> Vec<Pat> {
    let mut v = vec![];
    for t in o.terms.iter().filter(|t| !t.is_empty()) {
        let folded = if o.ignore_case {
            t.to_lowercase()
        } else {
            t.clone()
        };
        let mut add = |enc: &'static str, bytes: Vec<u8>| {
            if bytes.is_empty() {
                return;
            }
            let mut skip = [bytes.len(); 256];
            for (i, &b) in bytes[..bytes.len() - 1].iter().enumerate() {
                skip[b as usize] = bytes.len() - 1 - i;
            }
            v.push(Pat {
                term: t.clone(),
                enc,
                bytes,
                skip,
            });
        };
        add("UTF-8", folded.as_bytes().to_vec());
        if o.utf16 {
            add(
                "UTF-16",
                folded
                    .encode_utf16()
                    .flat_map(|c| c.to_le_bytes())
                    .collect(),
            );
        }
    }
    v
}

fn find_all(hay: &[u8], p: &Pat, mut f: impl FnMut(usize) -> bool) {
    let n = p.bytes.len();
    if hay.len() < n {
        return;
    }
    let mut i = 0;
    while i + n <= hay.len() {
        let last = hay[i + n - 1];
        if last == p.bytes[n - 1] && hay[i..i + n] == p.bytes[..] {
            if !f(i) {
                return;
            }
            i += 1;
        } else {
            i += p.skip[last as usize];
        }
    }
}

fn snippet(buf: &[u8], at: usize, len: usize, utf16: bool) -> String {
    let a = at.saturating_sub(CTX);
    let b = (at + len + CTX).min(buf.len());
    let s: String = if utf16 {
        let a = a & !1;
        let u: Vec<u16> = buf[a..b]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(&buf[a..b]).into_owned()
    };
    s.chars()
        .map(|c| if c.is_control() { '·' } else { c })
        .collect()
}

/// Varre `src` inteira. Retorna o total de ocorrências. `on_hit` pode devolver false para parar.
pub fn search(
    src: &dyn Source,
    o: &SearchOpts,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(u64, u64),
    on_hit: &mut dyn FnMut(Hit) -> bool,
) -> usize {
    let pats = make_pats(o);
    if pats.is_empty() {
        return 0;
    }
    let overlap = pats.iter().map(|p| p.bytes.len()).max().unwrap_or(1) + 2 * CTX;
    let size = src.size();
    let mut pos = 0u64;
    let mut hits = 0usize;
    let mut buf = vec![0u8; CHUNK + overlap];
    'outer: while pos < size {
        if cancelled() {
            break;
        }
        let want = ((size - pos) as usize).min(CHUNK + overlap);
        let mut got = 0;
        while got < want {
            match src.read_at(pos + got as u64, &mut buf[got..want]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(_) => {
                    // setor ruim: pula este trecho
                    for b in &mut buf[got..want] {
                        *b = 0;
                    }
                    got = want;
                }
            }
        }
        if got == 0 {
            break;
        }
        let fresh = got.min(CHUNK); // início de ocorrências aceitas neste bloco
        let folded: Vec<u8>;
        let hay: &[u8] = if o.ignore_case {
            folded = buf[..got].to_ascii_lowercase();
            &folded
        } else {
            &buf[..got]
        };
        let mut chunk_hits: Vec<(usize, usize)> = vec![]; // (offset no bloco, índice do padrão)
        for (pi, p) in pats.iter().enumerate() {
            find_all(hay, p, |i| {
                if i < fresh {
                    chunk_hits.push((i, pi));
                }
                true
            });
        }
        chunk_hits.sort();
        for (i, pi) in chunk_hits {
            let p = &pats[pi];
            hits += 1;
            let h = Hit {
                offset: pos + i as u64,
                term: p.term.clone(),
                encoding: p.enc.to_string(),
                context: snippet(&buf[..got], i, p.bytes.len(), p.enc == "UTF-16"),
            };
            if !on_hit(h) || hits >= o.max_hits {
                break 'outer;
            }
        }
        pos += CHUNK as u64;
        progress(pos.min(size), size);
    }
    progress(size, size);
    hits
}
