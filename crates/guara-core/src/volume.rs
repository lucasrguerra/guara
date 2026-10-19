//! Disco virtual: a origem real com trechos substituídos por volumes desbloqueados,
//! membros de RAID montados ou volumes lógicos (LVM) anexados depois do fim do disco.
//!
//! Os deslocamentos continuam sendo "do disco": quem varre e quem recupera enxergam o mesmo
//! espaço de endereços, desde que a montagem seja refeita igual (a `Assembly` é determinística).

use crate::source::Source;
use std::io;
use std::sync::Arc;

#[derive(Clone)]
pub struct Region {
    pub start: u64,
    pub len: u64,
    pub src: Arc<dyn Source>,
    pub label: String,
}

pub struct Overlay {
    base: Arc<dyn Source>,
    regions: Vec<Region>,
    size: u64,
}

impl Overlay {
    pub fn new(base: Arc<dyn Source>, mut regions: Vec<Region>) -> Self {
        regions.sort_by_key(|r| r.start);
        let size = regions
            .iter()
            .map(|r| r.start + r.len)
            .max()
            .unwrap_or(0)
            .max(base.size());
        Self {
            base,
            regions,
            size,
        }
    }
    pub fn regions(&self) -> &[Region] {
        &self.regions
    }
}

impl Source for Overlay {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((self.size - offset) as usize);
        let i = self.regions.partition_point(|r| r.start + r.len <= offset);
        if let Some(r) = self.regions.get(i) {
            if r.start <= offset {
                let n = want.min((r.start + r.len - offset) as usize);
                return r.src.read_at(offset - r.start, &mut buf[..n]);
            }
            // Antes da próxima região: lê da base só até ela.
            let n = want.min((r.start - offset) as usize);
            return self.read_base(offset, &mut buf[..n]);
        }
        self.read_base(offset, &mut buf[..want])
    }
}

impl Overlay {
    fn read_base(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.base.size() {
            // Lacuna entre a base e uma região anexada.
            buf.fill(0);
            return Ok(buf.len());
        }
        let n = buf.len().min((self.base.size() - offset) as usize);
        let got = self.base.read_at(offset, &mut buf[..n])?;
        if got < buf.len() && got == n {
            buf[n..].fill(0);
            return Ok(buf.len());
        }
        Ok(got)
    }
}
