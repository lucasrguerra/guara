//! Carving por assinatura/estrutura sobre intervalos de bytes.

use crate::signatures::{category_for_ext, detect, might_start, Hit};
use crate::source::Source;
use crate::types::Category;

pub const SECTOR: u64 = 512;
const CHUNK: usize = 8 * 1024 * 1024;
const LOOKAHEAD: usize = 4096;
const MIN_LEN: u64 = 256;

pub struct CarveCfg<'a> {
    pub max_size: u64,
    pub categories: &'a [Category],
}

/// Varre [start, end) a partir de `*next`. Chama `on_hit(offset, hit)` e retorna
/// `true` se terminou o intervalo (`false` se cancelado).
/// `advance(done_bytes)` informa o progresso dentro do intervalo.
pub fn carve_range(
    src: &dyn Source,
    start: u64,
    end: u64,
    next: &mut u64,
    cfg: &CarveCfg,
    cancelled: &(dyn Fn() -> bool + Sync),
    ev: &mut dyn CarveEvents,
) -> CarveEnd {
    let end = end.min(src.size());
    let mut pos = (*next).max(start);
    // Alinha ao setor.
    pos = pos.div_ceil(SECTOR) * SECTOR;
    let mut buf = vec![0u8; CHUNK + LOOKAHEAD];
    let mut skip_until = 0u64;
    let mut fails = 0u32;
    while pos < end {
        if cancelled() {
            *next = pos;
            return CarveEnd::Cancelled;
        }
        let want = ((end - pos) as usize).min(CHUNK + LOOKAHEAD);
        let got = match read_full(src, pos, &mut buf[..want]) {
            Ok(n) => {
                fails = 0;
                n
            }
            Err(_) => {
                // Várias falhas seguidas = dispositivo sumiu: para com o ponto de retomada salvo.
                fails += 1;
                if fails >= 4 {
                    *next = pos - (fails as u64 - 1) * CHUNK as u64;
                    return CarveEnd::IoLost;
                }
                // Setor ruim isolado: pula um bloco inteiro e segue (a imagem forense trata isso melhor).
                pos += CHUNK as u64;
                ev.advance(CHUNK as u64);
                *next = pos;
                continue;
            }
        };
        if got == 0 {
            break;
        }
        let scan_len = got.min(CHUNK).min((end - pos) as usize);
        let mut i = 0usize;
        while i < scan_len {
            let abs = pos + i as u64;
            if abs < skip_until {
                i += SECTOR as usize;
                continue;
            }
            let b0 = buf[i];
            if might_start(b0) {
                let hend = (i + 128).min(got);
                let head = &buf[i..hend];
                if let Some(hit) = detect(src, abs, head, cfg.max_size) {
                    let allowed = cfg.categories.is_empty()
                        || cfg.categories.contains(&category_for_ext(hit.ext));
                    if hit.len >= MIN_LEN && allowed {
                        let action = ev.hit(abs, &hit);
                        if matches!(action, HitAction::Skip)
                            && hit.confidence != crate::types::Confidence::Low
                        {
                            skip_until = abs + hit.len;
                        }
                    }
                }
            }
            i += SECTOR as usize;
        }
        let step = scan_len as u64;
        pos += step.div_ceil(SECTOR) * SECTOR;
        ev.advance(step);
        *next = pos;
    }
    *next = end;
    CarveEnd::Done
}

/// Como o carving de um intervalo terminou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarveEnd {
    Done,
    Cancelled,
    /// Várias leituras seguidas falharam: o dispositivo provavelmente foi desconectado.
    IoLost,
}

/// Receptor de eventos do carving.
pub trait CarveEvents {
    fn advance(&mut self, bytes: u64);
    fn hit(&mut self, offset: u64, hit: &Hit) -> HitAction;
}

/// O que fazer depois de um acerto.
pub enum HitAction {
    /// Pula o conteúdo do arquivo reconhecido (comportamento padrão).
    Skip,
    /// Segue setor a setor (acerto descartado).
    Continue,
}

fn read_full(src: &dyn Source, off: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        let n = src.read_at(off + got as u64, &mut buf[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    Ok(got)
}
