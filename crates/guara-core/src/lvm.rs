//! LVM2: lê o rótulo do volume físico (PV) e o texto de metadados, e remonta os volumes
//! lógicos (lineares, em faixas, espelhados e RAID 0/1/5/6/10 do LVM).

use crate::raid::{Layout, Level, Member, RaidParams, RaidSource};
use crate::source::{read_vec, Source};
use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

const MDA_MAGIC: &[u8; 16] = b" LVM2 x[5A%r0N*>";

#[derive(Clone, Debug)]
pub struct PvInfo {
    /// UUID de 32 caracteres, sem hífens.
    pub uuid: String,
    pub size: u64,
    pub seqno: u64,
    pub vg_name: String,
    pub text: String,
}

/// Lê rótulo + metadados de um PV.
pub fn read_pv(src: &dyn Source) -> Option<PvInfo> {
    let head = read_vec(src, 0, 2048).ok()?;
    if head.len() < 2048 {
        return None;
    }
    let sector = (0..4).find(|&s| &head[s * 512..s * 512 + 8] == b"LABELONE")?;
    let lab = &head[sector * 512..(sector + 1) * 512];
    if &lab[24..32] != b"LVM2 001" {
        return None;
    }
    let off = u32::from_le_bytes(lab[20..24].try_into().ok()?) as usize;
    let pv = lab.get(off..)?;
    let uuid = String::from_utf8_lossy(pv.get(..32)?).to_string();
    let size = u64::from_le_bytes(pv.get(32..40)?.try_into().ok()?);
    // Lista de áreas de dados, depois de áreas de metadados; cada lista termina em (0,0).
    let mut o = 40;
    let rd = |o: usize| -> Option<(u64, u64)> {
        Some((
            u64::from_le_bytes(pv.get(o..o + 8)?.try_into().ok()?),
            u64::from_le_bytes(pv.get(o + 8..o + 16)?.try_into().ok()?),
        ))
    };
    loop {
        let (a, b) = rd(o)?;
        o += 16;
        if a == 0 && b == 0 {
            break;
        }
    }
    let mut mdas = vec![];
    loop {
        let (a, b) = rd(o)?;
        o += 16;
        if a == 0 && b == 0 {
            break;
        }
        mdas.push((a, b));
    }
    let mut best: Option<PvInfo> = None;
    for (mda_off, mda_size) in mdas {
        let Ok(h) = read_vec(src, mda_off, 512) else {
            continue;
        };
        if h.len() < 512 || &h[4..20] != MDA_MAGIC {
            continue;
        }
        // raw_locn[0]
        let loc_off = u64::from_le_bytes(h[40..48].try_into().unwrap());
        let loc_size = u64::from_le_bytes(h[48..56].try_into().unwrap());
        if loc_size == 0 || loc_size > 16 << 20 || mda_size < 1024 {
            continue;
        }
        let mut text = vec![];
        if loc_off + loc_size <= mda_size {
            if let Ok(t) = read_vec(src, mda_off + loc_off, loc_size as usize) {
                text = t;
            }
        } else {
            // O texto dá a volta no anel.
            let first = mda_size - loc_off;
            if let (Ok(a), Ok(b)) = (
                read_vec(src, mda_off + loc_off, first as usize),
                read_vec(src, mda_off + 512, (loc_size - first) as usize),
            ) {
                text = [a, b].concat();
            }
        }
        let s = String::from_utf8_lossy(&text)
            .trim_end_matches('\0')
            .to_string();
        let Some(tree) = parse(&s) else { continue };
        let Some((vg_name, vg)) = tree
            .sections
            .iter()
            .find(|(_, sec)| sec.values.contains_key("extent_size"))
        else {
            continue;
        };
        let seqno = vg.int("seqno").unwrap_or(0);
        let cand = PvInfo {
            uuid: uuid.clone(),
            size,
            seqno,
            vg_name: vg_name.clone(),
            text: s.clone(),
        };
        if best.as_ref().map(|b| cand.seqno > b.seqno).unwrap_or(true) {
            best = Some(cand);
        }
    }
    best
}

// ------------------------------------------------------------------ texto

#[derive(Clone, Debug)]
pub enum Val {
    Int(i64),
    Str(String),
    List(Vec<Val>),
}

#[derive(Clone, Debug, Default)]
pub struct Section {
    pub values: BTreeMap<String, Val>,
    /// Subseções na ordem do arquivo.
    pub sections: Vec<(String, Section)>,
}

impl Section {
    pub fn int(&self, k: &str) -> Option<u64> {
        match self.values.get(k)? {
            Val::Int(i) => Some(*i as u64),
            _ => None,
        }
    }
    pub fn str(&self, k: &str) -> Option<&str> {
        match self.values.get(k)? {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn list(&self, k: &str) -> Option<&[Val]> {
        match self.values.get(k)? {
            Val::List(l) => Some(l),
            _ => None,
        }
    }
    pub fn sub(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
    }
}

struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn skip(&mut self) {
        loop {
            while self.i < self.b.len()
                && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b',')
            {
                self.i += 1;
            }
            if self.i < self.b.len() && self.b[self.i] == b'#' {
                while self.i < self.b.len() && self.b[self.i] != b'\n' {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }
    fn ident(&mut self) -> Option<String> {
        self.skip();
        let s = self.i;
        while self.i < self.b.len()
            && !self.b[self.i].is_ascii_whitespace()
            && !matches!(
                self.b[self.i],
                b'=' | b'{' | b'}' | b'[' | b']' | b'"' | b'#'
            )
        {
            self.i += 1;
        }
        (self.i > s).then(|| String::from_utf8_lossy(&self.b[s..self.i]).to_string())
    }
    fn value(&mut self) -> Option<Val> {
        self.skip();
        match *self.b.get(self.i)? {
            b'"' => {
                self.i += 1;
                let s = self.i;
                while self.i < self.b.len() && self.b[self.i] != b'"' {
                    if self.b[self.i] == b'\\' {
                        self.i += 1;
                    }
                    self.i += 1;
                }
                let v = String::from_utf8_lossy(&self.b[s..self.i.min(self.b.len())]).to_string();
                self.i += 1;
                Some(Val::Str(v))
            }
            b'[' => {
                self.i += 1;
                let mut l = vec![];
                loop {
                    self.skip();
                    if *self.b.get(self.i)? == b']' {
                        self.i += 1;
                        break;
                    }
                    l.push(self.value()?);
                }
                Some(Val::List(l))
            }
            _ => {
                let s = self.i;
                while self.i < self.b.len()
                    && (self.b[self.i].is_ascii_digit() || self.b[self.i] == b'-')
                {
                    self.i += 1;
                }
                if self.i == s {
                    return None;
                }
                std::str::from_utf8(&self.b[s..self.i])
                    .ok()?
                    .parse()
                    .ok()
                    .map(Val::Int)
            }
        }
    }
    fn section(&mut self, top: bool) -> Option<Section> {
        let mut sec = Section::default();
        loop {
            self.skip();
            if self.i >= self.b.len() {
                return top.then_some(sec);
            }
            if self.b[self.i] == b'}' {
                self.i += 1;
                return (!top).then_some(sec);
            }
            let name = self.ident()?;
            self.skip();
            match *self.b.get(self.i)? {
                b'=' => {
                    self.i += 1;
                    let v = self.value()?;
                    sec.values.insert(name, v);
                }
                b'{' => {
                    self.i += 1;
                    let s = self.section(false)?;
                    sec.sections.push((name, s));
                }
                _ => return None,
            }
        }
    }
}

pub fn parse(text: &str) -> Option<Section> {
    Lexer {
        b: text.as_bytes(),
        i: 0,
    }
    .section(true)
}

// ------------------------------------------------------------- montagem

fn dashed(u: &str) -> String {
    if u.len() != 32 {
        return u.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}-{}-{}",
        &u[0..6],
        &u[6..10],
        &u[10..14],
        &u[14..18],
        &u[18..22],
        &u[22..26],
        &u[26..32]
    )
}

#[derive(Clone)]
enum Map {
    /// Lê de `src` a partir de `base` (linear).
    Linear {
        src: Option<Arc<dyn Source>>,
        base: u64,
    },
    Striped {
        srcs: Vec<(Option<Arc<dyn Source>>, u64)>,
        stripe: u64,
    },
    Mirror {
        legs: Vec<(Option<Arc<dyn Source>>, u64)>,
    },
    Raid(Arc<RaidSource>),
}

struct Seg {
    start: u64,
    len: u64,
    map: Map,
}

pub struct LvSource {
    segs: Vec<Seg>,
    size: u64,
}

impl Source for LvSource {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((self.size - offset) as usize);
        let mut done = 0;
        while done < want {
            let pos = offset + done as u64;
            let i = self.segs.partition_point(|s| s.start + s.len <= pos);
            let Some(s) = self.segs.get(i) else { break };
            let rel = pos - s.start;
            let mut n = ((s.len - rel) as usize).min(want - done);
            let dst = &mut buf[done..done + n];
            match &s.map {
                Map::Linear { src, base } => match src {
                    Some(src) => n = read_full(src.as_ref(), base + rel, dst)?,
                    None => dst.fill(0),
                },
                Map::Striped { srcs, stripe } => {
                    let ns = srcs.len() as u64;
                    let (idx, within) = (rel / stripe, rel % stripe);
                    n = n.min((stripe - within) as usize);
                    let (src, base) = &srcs[(idx % ns) as usize];
                    let off = base + (idx / ns) * stripe + within;
                    match src {
                        Some(src) => n = read_full(src.as_ref(), off, &mut dst[..n])?,
                        None => dst[..n].fill(0),
                    }
                }
                Map::Mirror { legs } => {
                    let mut ok = false;
                    for (src, base) in legs {
                        if let Some(src) = src {
                            if let Ok(g) = read_full(src.as_ref(), base + rel, dst) {
                                n = g;
                                ok = true;
                                break;
                            }
                        }
                    }
                    if !ok {
                        dst.fill(0);
                    }
                }
                Map::Raid(r) => n = read_full(r.as_ref(), rel, dst)?,
            }
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }
}

fn read_full(src: &dyn Source, off: u64, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        let n = src.read_at(off + got as u64, &mut buf[got..])?;
        if n == 0 {
            buf[got..].fill(0);
            return Ok(buf.len());
        }
        got += n;
    }
    Ok(got)
}

pub struct VgResult {
    pub name: String,
    pub lvs: Vec<(String, Arc<dyn Source>)>,
    pub warnings: Vec<String>,
}

fn mk_stripes(list: &[Val]) -> Vec<(String, u64)> {
    let mut out = vec![];
    for pair in list.chunks(2) {
        if let [Val::Str(n), Val::Int(e)] = pair {
            out.push((n.clone(), *e as u64));
        }
    }
    out
}

struct Ctx<'a> {
    ext: u64,
    pvs: BTreeMap<String, (Option<Arc<dyn Source>>, u64)>, // nome -> (origem, pe_start em bytes)
    lvs: &'a Section,
    cache: BTreeMap<String, Option<Arc<dyn Source>>>,
    warnings: Vec<String>,
}

impl Ctx<'_> {
    fn build(&mut self, name: &str, depth: u32) -> Option<Arc<dyn Source>> {
        if let Some(c) = self.cache.get(name) {
            return c.clone();
        }
        if depth > 6 {
            return None;
        }
        let lv = self.lvs.sub(name)?.clone();
        let nsegs = lv.int("segment_count").unwrap_or(0);
        let mut segs: Vec<Seg> = vec![];
        for i in 1..=nsegs {
            let Some(seg) = lv.sub(&format!("segment{i}")) else {
                self.warnings.push(format!("{name}: segmento {i} ausente"));
                return None;
            };
            let start = seg.int("start_extent").unwrap_or(0) * self.ext;
            let len = seg.int("extent_count").unwrap_or(0) * self.ext;
            let ty = seg.str("type").unwrap_or("").to_string();
            let map = match ty.as_str() {
                "striped" => {
                    let st = mk_stripes(seg.list("stripes").unwrap_or(&[]));
                    let srcs: Vec<(Option<Arc<dyn Source>>, u64)> = st
                        .iter()
                        .map(|(pv, e)| match self.pvs.get(pv) {
                            Some((s, pe)) => (s.clone(), pe + e * self.ext),
                            None => (None, 0),
                        })
                        .collect();
                    if srcs.iter().any(|(s, _)| s.is_none()) {
                        self.warnings.push(format!(
                            "{name}: falta um volume físico; as áreas dele aparecem zeradas"
                        ));
                    }
                    if srcs.len() == 1 {
                        let (src, base) = srcs.into_iter().next().unwrap();
                        Map::Linear { src, base }
                    } else {
                        Map::Striped {
                            srcs,
                            stripe: seg.int("stripe_size").unwrap_or(128) * 512,
                        }
                    }
                }
                "mirror" => {
                    let st = mk_stripes(seg.list("mirrors").unwrap_or(&[]));
                    let legs = st
                        .iter()
                        .filter_map(|(pv, e)| {
                            self.pvs
                                .get(pv)
                                .map(|(s, pe)| (s.clone(), pe + e * self.ext))
                        })
                        .collect();
                    Map::Mirror { legs }
                }
                t if t.starts_with("raid") => {
                    let raids = seg.list("raids").unwrap_or(&[]);
                    let imgs: Vec<String> = raids
                        .iter()
                        .filter_map(|v| {
                            if let Val::Str(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    // Pares (rimage, rmeta) ou lista só de imagens.
                    let data_lvs: Vec<&String> =
                        imgs.iter().filter(|n| n.contains("_rimage_")).collect();
                    let mut members: Vec<Option<Member>> = vec![];
                    for n in &data_lvs {
                        members.push(self.build(n, depth + 1).map(|src| Member {
                            src,
                            data_offset: 0,
                        }));
                    }
                    let chunk = seg.int("stripe_size").unwrap_or(128) * 512;
                    let (level, layout) = match t {
                        "raid1" => (Level::R1, Layout::LeftSymmetric),
                        "raid0" | "raid0_meta" => (Level::R0, Layout::LeftSymmetric),
                        "raid4" | "raid5_n" => (Level::R4, Layout::ParityLast),
                        "raid5" | "raid5_ls" => (Level::R5, Layout::LeftSymmetric),
                        "raid5_la" => (Level::R5, Layout::LeftAsymmetric),
                        "raid5_rs" => (Level::R5, Layout::RightSymmetric),
                        "raid5_ra" => (Level::R5, Layout::RightAsymmetric),
                        "raid6" | "raid6_zr" | "raid6_ls_6" => (Level::R6, Layout::LeftSymmetric),
                        "raid6_nr" => (Level::R6, Layout::LeftAsymmetric),
                        "raid6_nc" | "raid6_n_6" => (Level::R6, Layout::ParityLast),
                        "raid10" => (Level::R10, Layout::LeftSymmetric),
                        other => {
                            self.warnings
                                .push(format!("{name}: tipo {other} não suportado"));
                            return None;
                        }
                    };
                    let mut p = RaidParams::simple(level, chunk.max(512), layout, members);
                    p.near = 2;
                    p.far = 1;
                    match RaidSource::new(p) {
                        Ok(r) => Map::Raid(Arc::new(r)),
                        Err(e) => {
                            self.warnings.push(format!("{name}: {e}"));
                            return None;
                        }
                    }
                }
                other => {
                    self.warnings.push(format!(
                        "{name}: tipo de segmento \"{other}\" não suportado"
                    ));
                    return None;
                }
            };
            segs.push(Seg { start, len, map });
        }
        segs.sort_by_key(|s| s.start);
        let size = segs.iter().map(|s| s.start + s.len).max().unwrap_or(0);
        let src: Arc<dyn Source> = Arc::new(LvSource { segs, size });
        self.cache.insert(name.to_string(), Some(src.clone()));
        Some(src)
    }
}

/// Monta os volumes lógicos de cada grupo de volumes presente nos PVs fornecidos.
pub fn assemble_vgs(pvs: Vec<(Arc<dyn Source>, PvInfo)>) -> Vec<VgResult> {
    let mut by_vg: BTreeMap<String, Vec<(Arc<dyn Source>, PvInfo)>> = BTreeMap::new();
    for (s, p) in pvs {
        by_vg.entry(p.vg_name.clone()).or_default().push((s, p));
    }
    let mut out = vec![];
    for (vg_name, list) in by_vg {
        let newest = list.iter().max_by_key(|(_, p)| p.seqno).unwrap();
        let Some(tree) = parse(&newest.1.text) else {
            continue;
        };
        let Some(vg) = tree.sub(&vg_name) else {
            continue;
        };
        let ext = vg.int("extent_size").unwrap_or(8192) * 512;
        let mut pvmap = BTreeMap::new();
        let mut warnings = vec![];
        if let Some(pvsec) = vg.sub("physical_volumes") {
            for (pvname, def) in &pvsec.sections {
                let id = def.str("id").unwrap_or("").to_string();
                let found = list
                    .iter()
                    .find(|(_, p)| dashed(&p.uuid) == id)
                    .map(|(s, _)| s.clone());
                if found.is_none() {
                    warnings.push(format!("volume físico {pvname} ({id}) não foi fornecido"));
                }
                pvmap.insert(
                    pvname.clone(),
                    (found, def.int("pe_start").unwrap_or(2048) * 512),
                );
            }
        }
        let Some(lvsec) = vg.sub("logical_volumes") else {
            continue;
        };
        let mut ctx = Ctx {
            ext,
            pvs: pvmap,
            lvs: lvsec,
            cache: BTreeMap::new(),
            warnings,
        };
        let mut lvs = vec![];
        for (name, def) in &lvsec.sections {
            // Imagens internas do RAID e metadados não são volumes de usuário.
            let hidden = def
                .list("status")
                .map(|l| !l.iter().any(|v| matches!(v, Val::Str(s) if s == "VISIBLE")))
                .unwrap_or(false);
            if hidden {
                continue;
            }
            if let Some(src) = ctx.build(name, 0) {
                lvs.push((name.clone(), src));
            }
        }
        out.push(VgResult {
            name: vg_name,
            lvs,
            warnings: ctx.warnings,
        });
    }
    out
}
