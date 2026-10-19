//! RAID por software: linear, 0, 1, 4, 5, 6 e 10, a partir dos discos soltos.
//!
//! Lê os metadados do mdadm (superbloco 0.90 e 1.x) para montar sozinho; sem metadados, o
//! módulo `raid_detect` descobre ordem, tamanho de faixa e layout. Se faltarem discos, o
//! array é lido em modo degradado (paridade reconstrói o que faltar).

use crate::source::{read_vec, Source};
use std::io;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Linear,
    R0,
    R1,
    R4,
    R5,
    R6,
    R10,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Linear => "linear",
            Level::R0 => "RAID 0",
            Level::R1 => "RAID 1",
            Level::R4 => "RAID 4",
            Level::R5 => "RAID 5",
            Level::R6 => "RAID 6",
            Level::R10 => "RAID 10",
        }
    }
}

/// Posição da paridade e ordem dos dados, como no driver md do Linux.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    LeftAsymmetric,
    RightAsymmetric,
    LeftSymmetric,
    RightSymmetric,
    /// RAID 4 e variantes "parity-0"/"parity-n".
    ParityFirst,
    ParityLast,
}

impl Layout {
    pub fn from_md(v: u32) -> Layout {
        match v {
            0 => Layout::LeftAsymmetric,
            1 => Layout::RightAsymmetric,
            2 => Layout::LeftSymmetric,
            3 => Layout::RightSymmetric,
            4 => Layout::ParityFirst,
            5 => Layout::ParityLast,
            _ => Layout::LeftSymmetric,
        }
    }
    pub fn all_raid5() -> [Layout; 4] {
        [
            Layout::LeftSymmetric,
            Layout::LeftAsymmetric,
            Layout::RightSymmetric,
            Layout::RightAsymmetric,
        ]
    }
}

#[derive(Clone)]
pub struct Member {
    pub src: Arc<dyn Source>,
    /// Onde começam os dados dentro do membro.
    pub data_offset: u64,
}

#[derive(Clone)]
pub struct RaidParams {
    pub level: Level,
    /// Tamanho da faixa por disco, em bytes.
    pub chunk: u64,
    pub layout: Layout,
    /// Membros na ordem do array; `None` = disco ausente.
    pub members: Vec<Option<Member>>,
    /// RAID 10: cópias "perto" (near) e "longe" (far).
    pub near: u32,
    pub far: u32,
    pub far_offset: bool,
    /// Bytes úteis por membro (0 = até o fim do menor).
    pub per_member: u64,
}

impl RaidParams {
    pub fn simple(level: Level, chunk: u64, layout: Layout, members: Vec<Option<Member>>) -> Self {
        Self {
            level,
            chunk,
            layout,
            members,
            near: 2,
            far: 1,
            far_offset: false,
            per_member: 0,
        }
    }
}

pub struct RaidSource {
    p: RaidParams,
    size: u64,
    per_member: u64,
}

// ------------------------------------------------------------ GF(2^8) para RAID 6

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1d;
        }
        b >>= 1;
    }
    r
}

fn gf_pow2(n: usize) -> u8 {
    let mut r = 1u8;
    for _ in 0..n % 255 {
        r = gf_mul(r, 2);
    }
    r
}

fn gf_inv(a: u8) -> u8 {
    // a^254
    let mut r = 1u8;
    let mut base = a;
    let mut e = 254u32;
    while e > 0 {
        if e & 1 != 0 {
            r = gf_mul(r, base);
        }
        base = gf_mul(base, base);
        e >>= 1;
    }
    r
}

impl RaidSource {
    pub fn new(p: RaidParams) -> Result<Self, String> {
        let n = p.members.len();
        if n == 0 {
            return Err("RAID sem membros".into());
        }
        let present: Vec<&Member> = p.members.iter().flatten().collect();
        let min = |f: &dyn Fn(&Member) -> u64| present.iter().map(|m| f(m)).min().unwrap_or(0);
        let avail = min(&|m| m.src.size().saturating_sub(m.data_offset));
        let mut per_member = if p.per_member > 0 {
            p.per_member.min(avail)
        } else {
            avail
        };
        if p.level != Level::Linear {
            per_member = per_member / p.chunk.max(512) * p.chunk.max(512);
        }
        let missing = n - present.len();
        let need = match p.level {
            Level::Linear | Level::R0 => 0,
            Level::R1 => n - 1,
            Level::R4 | Level::R5 => 1,
            Level::R6 => 2,
            Level::R10 => (p.near.max(1) as usize).saturating_sub(1).min(1),
        };
        if missing > need && p.level != Level::R10 {
            return Err(format!(
                "faltam {missing} membro(s); {} tolera no máximo {need}",
                p.level.name()
            ));
        }
        if matches!(p.level, Level::R4 | Level::R5) && n < 3 || p.level == Level::R6 && n < 4 {
            return Err("poucos membros para o nível".into());
        }
        if p.level == Level::R10 && (n < 2 || p.near as usize * p.far as usize > n.max(2)) {
            return Err("parâmetros de RAID 10 inválidos".into());
        }
        let size = match p.level {
            Level::Linear => present
                .iter()
                .map(|m| m.src.size().saturating_sub(m.data_offset))
                .sum(),
            Level::R0 => per_member * n as u64,
            Level::R1 => per_member,
            Level::R4 | Level::R5 => per_member * (n as u64 - 1),
            Level::R6 => per_member * (n as u64 - 2),
            Level::R10 => per_member * n as u64 / (p.near.max(1) * p.far.max(1)) as u64,
        };
        Ok(Self {
            p,
            size,
            per_member,
        })
    }

    pub fn params(&self) -> &RaidParams {
        &self.p
    }

    fn read_member(&self, i: usize, off: u64, buf: &mut [u8]) -> io::Result<()> {
        let m = self.p.members[i]
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "membro ausente"))?;
        let mut done = 0;
        while done < buf.len() {
            let n = m
                .src
                .read_at(m.data_offset + off + done as u64, &mut buf[done..])?;
            if n == 0 {
                buf[done..].fill(0);
                break;
            }
            done += n;
        }
        Ok(())
    }

    /// (disco de paridade P, disco de paridade Q, disco que guarda o dado `d` da linha `row`)
    fn raid5_map(&self, row: u64, d: usize) -> (usize, usize) {
        let n = self.p.members.len();
        let r = (row % n as u64) as usize;
        match self.p.layout {
            Layout::LeftAsymmetric | Layout::LeftSymmetric => {
                let pd = n - 1 - r;
                let disk = if self.p.layout == Layout::LeftSymmetric {
                    (pd + 1 + d) % n
                } else if d >= pd {
                    d + 1
                } else {
                    d
                };
                (pd, disk)
            }
            Layout::RightAsymmetric | Layout::RightSymmetric => {
                let pd = r;
                let disk = if self.p.layout == Layout::RightSymmetric {
                    (pd + 1 + d) % n
                } else if d >= pd {
                    d + 1
                } else {
                    d
                };
                (pd, disk)
            }
            Layout::ParityFirst => (0, d + 1),
            Layout::ParityLast => (n - 1, d),
        }
    }

    fn raid6_map(&self, row: u64, d: usize) -> (usize, usize, usize) {
        let n = self.p.members.len();
        let r = (row % n as u64) as usize;
        match self.p.layout {
            Layout::LeftAsymmetric | Layout::LeftSymmetric => {
                let pd = n - 1 - r;
                let qd = (pd + 1) % n;
                let disk = if self.p.layout == Layout::LeftSymmetric {
                    (pd + 2 + d) % n
                } else {
                    // assimétrico: dados em ordem pulando P e Q
                    let mut k = d;
                    for skip in sorted2(pd, qd) {
                        if k >= skip {
                            k += 1;
                        }
                    }
                    k
                };
                (pd, qd, disk)
            }
            Layout::RightAsymmetric | Layout::RightSymmetric => {
                let pd = r;
                let qd = (pd + 1) % n;
                let disk = if self.p.layout == Layout::RightSymmetric {
                    (pd + 2 + d) % n
                } else {
                    let mut k = d;
                    for skip in sorted2(pd, qd) {
                        if k >= skip {
                            k += 1;
                        }
                    }
                    k
                };
                (pd, qd, disk)
            }
            Layout::ParityFirst => (0, 1, d + 2),
            Layout::ParityLast => (n - 2, n - 1, d),
        }
    }

    /// Lê `buf.len()` bytes dentro de um único chunk lógico.
    fn read_in_chunk(&self, chunk_idx: u64, within: u64, buf: &mut [u8]) -> io::Result<()> {
        let n = self.p.members.len();
        let c = self.p.chunk;
        match self.p.level {
            Level::Linear => unreachable!(),
            Level::R0 => {
                let (row, disk) = (chunk_idx / n as u64, (chunk_idx % n as u64) as usize);
                self.read_member(disk, row * c + within, buf)
            }
            Level::R1 => {
                for i in 0..n {
                    if self.p.members[i].is_some()
                        && self.read_member(i, chunk_idx * c + within, buf).is_ok()
                    {
                        return Ok(());
                    }
                }
                Err(io::Error::other("nenhuma cópia legível"))
            }
            Level::R4 | Level::R5 => {
                let dd = n - 1;
                let (row, d) = (chunk_idx / dd as u64, (chunk_idx % dd as u64) as usize);
                let (pd, disk) = self.raid5_map(row, d);
                let off = row * c + within;
                if self.p.members[disk].is_some() {
                    return self.read_member(disk, off, buf);
                }
                // Reconstrói por XOR de todos os outros (dados + paridade) da linha.
                buf.fill(0);
                let mut tmp = vec![0u8; buf.len()];
                for i in 0..n {
                    if i == disk {
                        continue;
                    }
                    let _ = pd;
                    self.read_member(i, off, &mut tmp)?;
                    for (a, b) in buf.iter_mut().zip(&tmp) {
                        *a ^= *b;
                    }
                }
                Ok(())
            }
            Level::R6 => {
                let dd = n - 2;
                let (row, d) = (chunk_idx / dd as u64, (chunk_idx % dd as u64) as usize);
                let (_, _, disk) = self.raid6_map(row, d);
                let off = row * c + within;
                if self.p.members[disk].is_some() {
                    return self.read_member(disk, off, buf);
                }
                self.raid6_rebuild(row, d, off, buf)
            }
            Level::R10 => {
                let mut last = None;
                for (disk, off) in self.r10_locations(chunk_idx) {
                    if self.p.members[disk].is_some() {
                        match self.read_member(disk, off + within, buf) {
                            Ok(()) => return Ok(()),
                            Err(e) => last = Some(e),
                        }
                    }
                }
                Err(last
                    .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cópias ausentes")))
            }
        }
    }

    /// Onde ficam todas as cópias de um chunk lógico (como `raid10_find_phys` do kernel).
    fn r10_locations(&self, chunk_idx: u64) -> Vec<(usize, u64)> {
        let n = self.p.members.len() as u64;
        let c = self.p.chunk;
        let near = self.p.near.max(1) as u64;
        let far = self.p.far.max(1) as u64;
        let slot0 = chunk_idx * near;
        let (mut stripe, dev) = (slot0 / n, slot0 % n);
        if self.p.far_offset {
            stripe *= far;
        }
        let stride = if self.p.far_offset {
            c
        } else {
            (self.per_member / far) / c * c
        };
        let mut out = vec![];
        let mut d = dev;
        let mut off = stripe * c;
        let mut firsts = vec![];
        for _ in 0..near {
            firsts.push((d, off));
            out.push((d as usize, off));
            d += 1;
            if d >= n {
                d = 0;
                off += c;
            }
        }
        for f in 1..far {
            for &(dv, of) in &firsts {
                out.push((((dv + near * f) % n) as usize, of + stride * f));
            }
        }
        out
    }

    fn raid6_rebuild(&self, row: u64, d: usize, off: u64, buf: &mut [u8]) -> io::Result<()> {
        let n = self.p.members.len();
        let dd = n - 2;
        let (pd, qd, _) = self.raid6_map(row, 0);
        let len = buf.len();
        // Estado de cada dado da linha.
        let disks: Vec<usize> = (0..dd).map(|k| self.raid6_map(row, k).2).collect();
        let missing_data: Vec<usize> = (0..dd)
            .filter(|&k| self.p.members[disks[k]].is_none())
            .collect();
        let p_ok = self.p.members[pd].is_some();
        let q_ok = self.p.members[qd].is_some();
        let mut data: Vec<Option<Vec<u8>>> = vec![None; dd];
        for k in 0..dd {
            if self.p.members[disks[k]].is_some() {
                let mut v = vec![0u8; len];
                self.read_member(disks[k], off, &mut v)?;
                data[k] = Some(v);
            }
        }
        let rd = |disk: usize| -> io::Result<Vec<u8>> {
            let mut v = vec![0u8; len];
            self.read_member(disk, off, &mut v)?;
            Ok(v)
        };
        if missing_data.len() == 1 && p_ok {
            let p = rd(pd)?;
            let mut acc = p;
            for v in data.iter().flatten() {
                for (a, b) in acc.iter_mut().zip(v) {
                    *a ^= *b;
                }
            }
            buf.copy_from_slice(&acc);
            return Ok(());
        }
        if missing_data.len() == 1 && q_ok {
            let q = rd(qd)?;
            let x = missing_data[0];
            let mut acc = q;
            for (k, v) in data.iter().enumerate() {
                if let Some(v) = v {
                    let g = gf_pow2(k);
                    for (a, b) in acc.iter_mut().zip(v) {
                        *a ^= gf_mul(g, *b);
                    }
                }
            }
            let inv = gf_inv(gf_pow2(x));
            for (o, a) in buf.iter_mut().zip(&acc) {
                *o = gf_mul(inv, *a);
            }
            return Ok(());
        }
        if missing_data.len() == 2 && p_ok && q_ok {
            let (x, y) = (missing_data[0], missing_data[1]);
            let p = rd(pd)?;
            let q = rd(qd)?;
            let (mut pxy, mut qxy) = (p, q);
            for (k, v) in data.iter().enumerate() {
                if let Some(v) = v {
                    let g = gf_pow2(k);
                    for i in 0..len {
                        pxy[i] ^= v[i];
                        qxy[i] ^= gf_mul(g, v[i]);
                    }
                }
            }
            // Dx ^ Dy = pxy ; g^x Dx ^ g^y Dy = qxy
            let (gx, gy) = (gf_pow2(x), gf_pow2(y));
            let denom_inv = gf_inv(gx ^ gy);
            let target = if d == x { x } else { y };
            for i in 0..len {
                // Dx = (qxy ^ gy*pxy) / (gx ^ gy)
                let dx = gf_mul(denom_inv, qxy[i] ^ gf_mul(gy, pxy[i]));
                buf[i] = if target == x { dx } else { pxy[i] ^ dx };
            }
            return Ok(());
        }
        Err(io::Error::other(
            "RAID 6 sem redundância suficiente nesta linha",
        ))
    }
}

fn sorted2(a: usize, b: usize) -> [usize; 2] {
    if a < b {
        [a, b]
    } else {
        [b, a]
    }
}

impl Source for RaidSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((self.size - offset) as usize);
        let mut done = 0usize;
        if self.p.level == Level::Linear {
            let mut base = 0u64;
            for m in self.p.members.iter().flatten() {
                let len = m.src.size().saturating_sub(m.data_offset);
                if offset + done as u64 >= base && offset + (done as u64) < base + len {
                    let rel = offset + done as u64 - base;
                    let n = ((len - rel) as usize).min(want - done);
                    let got = m
                        .src
                        .read_at(m.data_offset + rel, &mut buf[done..done + n])?;
                    if got == 0 {
                        break;
                    }
                    done += got;
                    if done >= want {
                        break;
                    }
                }
                base += len;
            }
            return Ok(done);
        }
        let c = self.p.chunk;
        while done < want {
            let pos = offset + done as u64;
            let (idx, within) = (pos / c, pos % c);
            let n = ((c - within) as usize).min(want - done);
            self.read_in_chunk(idx, within, &mut buf[done..done + n])?;
            done += n;
        }
        Ok(done)
    }
}

// ============================================================ metadados do mdadm

#[derive(Clone, Debug)]
pub struct MdSuper {
    pub version: String,
    pub uuid: [u8; 16],
    pub name: String,
    pub level: Level,
    pub layout: u32,
    pub chunk: u64,
    pub raid_disks: u32,
    /// Papel deste disco no array (None = reserva/falho).
    pub role: Option<u32>,
    pub data_offset: u64,
    /// Bytes de dados por membro.
    pub data_size: u64,
    pub events: u64,
    pub array_size: u64,
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn level_from(v: i32) -> Option<Level> {
    Some(match v {
        -1 => Level::Linear,
        0 => Level::R0,
        1 => Level::R1,
        4 => Level::R4,
        5 => Level::R5,
        6 => Level::R6,
        10 => Level::R10,
        _ => return None,
    })
}

/// Procura um superbloco md nas posições possíveis de `src` (um disco ou partição).
pub fn read_md_super(src: &dyn Source) -> Option<MdSuper> {
    let size = src.size();
    // 1.1 no início, 1.2 a 4 KiB, 1.0 a 8 KiB do fim.
    let mut cands: Vec<(u64, &str)> = vec![(0, "1.1"), (4096, "1.2")];
    if size > 16384 {
        cands.push(((size & !4095) - 8192, "1.0"));
    }
    for (off, ver) in cands {
        let Ok(b) = read_vec(src, off, 4096) else {
            continue;
        };
        if b.len() < 4096 || le32(&b, 0) != 0xA92B_4EFC || le32(&b, 4) != 1 {
            continue;
        }
        let level = level_from(le32(&b, 72) as i32)?;
        let max_dev = le32(&b, 220) as usize;
        let dev_number = le32(&b, 160) as usize;
        let role = if dev_number < max_dev.min(1024) && 256 + dev_number * 2 + 2 <= b.len() {
            let r = u16::from_le_bytes([b[256 + dev_number * 2], b[256 + dev_number * 2 + 1]]);
            (r < 0xFFFE).then_some(r as u32)
        } else {
            None
        };
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&b[16..32]);
        let name = String::from_utf8_lossy(&b[32..64])
            .trim_end_matches('\0')
            .to_string();
        let data_offset = le64(&b, 128) * 512;
        let data_size = le64(&b, 136) * 512;
        let raid_disks = le32(&b, 92);
        let chunk = le32(&b, 88) as u64 * 512;
        let used = le64(&b, 80) * 512;
        let array_size = match level {
            Level::R0 => used * raid_disks as u64,
            Level::R5 | Level::R4 => used * (raid_disks as u64).saturating_sub(1),
            Level::R6 => used * (raid_disks as u64).saturating_sub(2),
            _ => used,
        };
        return Some(MdSuper {
            version: ver.into(),
            uuid,
            name,
            level,
            layout: le32(&b, 76),
            chunk,
            raid_disks,
            role,
            data_offset,
            data_size,
            events: le64(&b, 200),
            array_size,
        });
    }
    // 0.90: no fim, alinhado a 64 KiB.
    if size > 131072 {
        let off = (size & !65535) - 65536;
        if let Ok(b) = read_vec(src, off, 4096) {
            if b.len() >= 1024 && le32(&b, 0) == 0xA92B_4EFC && le32(&b, 4) == 0 {
                let level = level_from(le32(&b, 28) as i32)?;
                let mut uuid = [0u8; 16];
                uuid[0..4].copy_from_slice(&b[20..24]);
                uuid[4..16].copy_from_slice(&b[52..64]);
                let role = {
                    let r = le32(&b, 768 + 12);
                    (r < 0xFFFE).then_some(r)
                };
                let raid_disks = le32(&b, 40);
                let used = le32(&b, 32) as u64 * 1024;
                return Some(MdSuper {
                    version: "0.90".into(),
                    uuid,
                    name: String::new(),
                    level,
                    layout: le32(&b, 256),
                    chunk: le32(&b, 260) as u64,
                    raid_disks,
                    role,
                    data_offset: 0,
                    data_size: used,
                    events: ((le32(&b, 39 * 4) as u64) << 32) | le32(&b, 40 * 4) as u64,
                    array_size: match level {
                        Level::R0 => used * raid_disks as u64,
                        Level::R5 | Level::R4 => used * (raid_disks as u64).saturating_sub(1),
                        Level::R6 => used * (raid_disks as u64).saturating_sub(2),
                        _ => used,
                    },
                });
            }
        }
    }
    None
}

fn uuid_str(u: &[u8; 16]) -> String {
    u.iter().map(|b| format!("{b:02x}")).collect()
}

/// Um array reconhecido pelos superblocos dos discos fornecidos.
pub struct FoundArray {
    pub uuid: String,
    pub name: String,
    pub info: MdSuper,
    pub source: Arc<RaidSource>,
    pub missing: usize,
}

/// Agrupa membros por UUID e monta cada array (degradado se for possível).
pub fn assemble_md(members: Vec<(Arc<dyn Source>, MdSuper)>) -> Vec<Result<FoundArray, String>> {
    let mut groups: Vec<(String, Vec<(Arc<dyn Source>, MdSuper)>)> = vec![];
    for (s, m) in members {
        let key = uuid_str(&m.uuid);
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => g.1.push((s, m)),
            None => groups.push((key, vec![(s, m)])),
        }
    }
    let mut out = vec![];
    for (uuid, mut list) in groups {
        // O membro com mais eventos manda; membros desatualizados são tratados como ausentes.
        list.sort_by_key(|(_, m)| std::cmp::Reverse(m.events));
        let top = list[0].1.clone();
        let n = top.raid_disks as usize;
        let mut slots: Vec<Option<Member>> = vec![None; n];
        let mut stale = 0;
        for (s, m) in &list {
            if let Some(r) = m.role {
                if (r as usize) < n && slots[r as usize].is_none() {
                    if m.events + 1 < top.events && top.events > 0 {
                        stale += 1;
                        continue;
                    }
                    slots[r as usize] = Some(Member {
                        src: s.clone(),
                        data_offset: m.data_offset,
                    });
                }
            }
        }
        let missing = slots.iter().filter(|s| s.is_none()).count();
        let mut p = RaidParams::simple(
            top.level,
            top.chunk.max(512),
            Layout::from_md(top.layout),
            slots,
        );
        if top.level == Level::R10 {
            p.near = top.layout & 0xff;
            p.far = (top.layout >> 8) & 0xff;
            p.far_offset = top.layout & 0x10000 != 0;
        }
        if top.data_size > 0 {
            p.per_member = top.data_size;
        }
        match RaidSource::new(p) {
            Ok(src) => out.push(Ok(FoundArray {
                uuid: uuid.clone(),
                name: top.name.clone(),
                info: top,
                source: Arc::new(src),
                missing: missing + stale,
            })),
            Err(e) => out.push(Err(format!("array {uuid}: {e}"))),
        }
    }
    out
}
