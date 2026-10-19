//! Reconstrução de vídeo MP4/MOV sem o átomo `moov` (gravação interrompida de câmera ou drone,
//! arquivo cortado). Os quadros ficam no `mdat` como unidades NAL com prefixo de tamanho; o
//! índice (`moov`) é refeito a partir delas: tamanhos de amostra, quadros-chave, tempo e, para
//! H.264, a ordem de apresentação (POC) quando há quadros B. O áudio não é reconstruído.

use crate::source::{read_vec, Source};

fn be32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4)
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}

// ------------------------------------------------------------------ bits

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0 }
    }
    fn bit(&mut self) -> Option<u32> {
        let b = *self.d.get(self.pos / 8)?;
        let v = (b >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(v as u32)
    }
    fn u(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }
    fn ue(&mut self) -> Option<u32> {
        let mut z = 0;
        while self.bit()? == 0 {
            z += 1;
            if z > 31 {
                return None;
            }
        }
        Some(((1u64 << z) - 1 + self.u(z)? as u64) as u32)
    }
    fn se(&mut self) -> Option<i32> {
        let k = self.ue()?;
        Some(if k % 2 == 1 {
            (k / 2 + 1) as i32
        } else {
            -((k / 2) as i32)
        })
    }
    fn skip(&mut self, n: usize) {
        self.pos += n;
    }
}

/// Remove os bytes de "emulation prevention" (00 00 03 -> 00 00).
fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut z = 0;
    for &b in nal {
        if z >= 2 && b == 3 {
            z = 0;
            continue;
        }
        z = if b == 0 { z + 1 } else { 0 };
        out.push(b);
    }
    out
}

// ------------------------------------------------------------------ H.264

#[derive(Clone, Debug, Default)]
pub struct Sps264 {
    pub profile: u8,
    pub compat: u8,
    pub level: u8,
    pub width: u32,
    pub height: u32,
    pub log2_max_frame_num: u32,
    pub poc_type: u32,
    pub log2_max_poc_lsb: u32,
    pub frame_mbs_only: bool,
    /// (unidades por tique, escala de tempo)
    pub timing: Option<(u32, u32)>,
}

fn skip_scaling_list(b: &mut Bits, n: usize) -> Option<()> {
    let (mut last, mut next) = (8i32, 8i32);
    for _ in 0..n {
        if next != 0 {
            next = (last + b.se()? + 256) % 256;
        }
        last = if next == 0 { last } else { next };
    }
    Some(())
}

pub fn parse_sps264(nal: &[u8]) -> Option<Sps264> {
    if nal.len() < 5 || nal[0] & 0x1F != 7 {
        return None;
    }
    let rbsp = unescape(&nal[1..]);
    let mut b = Bits::new(&rbsp);
    let mut s = Sps264 {
        profile: b.u(8)? as u8,
        compat: b.u(8)? as u8,
        level: b.u(8)? as u8,
        ..Default::default()
    };
    b.ue()?; // sps_id
    let mut chroma = 1;
    if matches!(
        s.profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma = b.ue()?;
        if chroma == 3 {
            b.bit()?;
        }
        b.ue()?;
        b.ue()?;
        b.bit()?;
        if b.bit()? == 1 {
            for i in 0..(if chroma != 3 { 8 } else { 12 }) {
                if b.bit()? == 1 {
                    skip_scaling_list(&mut b, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    s.log2_max_frame_num = b.ue()? + 4;
    s.poc_type = b.ue()?;
    match s.poc_type {
        0 => s.log2_max_poc_lsb = b.ue()? + 4,
        1 => {
            b.bit()?;
            b.se()?;
            b.se()?;
            let n = b.ue()?;
            for _ in 0..n.min(255) {
                b.se()?;
            }
        }
        _ => {}
    }
    b.ue()?; // max_num_ref_frames
    b.bit()?;
    let w = b.ue()? + 1;
    let h = b.ue()? + 1;
    s.frame_mbs_only = b.bit()? == 1;
    if !s.frame_mbs_only {
        b.bit()?;
    }
    b.bit()?;
    let (mut cl, mut cr, mut ct, mut cb) = (0, 0, 0, 0);
    if b.bit()? == 1 {
        cl = b.ue()?;
        cr = b.ue()?;
        ct = b.ue()?;
        cb = b.ue()?;
    }
    let (sub_w, sub_h) = match chroma {
        0 => (1, 2 - s.frame_mbs_only as u32),
        1 => (2, 2 * (2 - s.frame_mbs_only as u32)),
        2 => (2, 2 - s.frame_mbs_only as u32),
        _ => (1, 2 - s.frame_mbs_only as u32),
    };
    s.width = (w * 16).checked_sub((cl + cr) * sub_w)?;
    s.height = ((2 - s.frame_mbs_only as u32) * h * 16).checked_sub((ct + cb) * sub_h)?;
    // VUI até o timing_info.
    if b.bit()? == 1 {
        if b.bit()? == 1 && b.u(8)? == 255 {
            b.skip(32);
        }
        if b.bit()? == 1 {
            b.bit()?;
        }
        if b.bit()? == 1 {
            b.skip(4);
            if b.bit()? == 1 {
                b.skip(24);
            }
        }
        if b.bit()? == 1 {
            b.ue()?;
            b.ue()?;
        }
        if b.bit()? == 1 {
            let units = b.u(32)?;
            let scale = b.u(32)?;
            if units > 0 && scale > 0 {
                s.timing = Some((units, scale));
            }
        }
    }
    Some(s)
}

/// (primeiro_mb == 0, tipo de fatia, pps_id, frame_num, idr, poc_lsb) do cabeçalho de fatia H.264.
struct SliceInfo {
    frame_num: u32,
    first_mb0: bool,
    idr: bool,
    poc_lsb: Option<u32>,
    bottom: bool,
}

fn slice264(nal: &[u8], sps: &Sps264, pic_order_present: bool) -> Option<SliceInfo> {
    let rbsp = unescape(&nal[1..nal.len().min(48)]);
    let mut b = Bits::new(&rbsp);
    let first = b.ue()?;
    b.ue()?; // slice_type
    b.ue()?; // pps_id
    let frame_num = b.u(sps.log2_max_frame_num)?;
    let mut bottom = false;
    if !sps.frame_mbs_only && b.bit()? == 1 {
        bottom = b.bit()? == 1;
    }
    let idr = nal[0] & 0x1F == 5;
    if idr {
        b.ue()?;
    }
    let mut poc_lsb = None;
    if sps.poc_type == 0 {
        poc_lsb = Some(b.u(sps.log2_max_poc_lsb)?);
        let _ = pic_order_present;
    }
    Some(SliceInfo {
        frame_num,
        first_mb0: first == 0,
        idr,
        poc_lsb,
        bottom,
    })
}

/// bottom_field_pic_order_in_frame_present_flag do PPS.
fn pps_pic_order_present(nal: &[u8]) -> bool {
    let rbsp = unescape(&nal[1..]);
    let mut b = Bits::new(&rbsp);
    let (Some(_), Some(_), Some(_)) = (b.ue(), b.ue(), b.bit()) else {
        return false;
    };
    b.bit() == Some(1)
}

// ------------------------------------------------------------------ HEVC

#[derive(Clone, Debug, Default)]
pub struct SpsHevc {
    pub width: u32,
    pub height: u32,
    pub ptl: [u8; 12],
    pub chroma: u32,
    pub bit_depth_luma: u32,
    pub bit_depth_chroma: u32,
    pub timing: Option<(u32, u32)>,
}

pub fn parse_sps_hevc(nal: &[u8]) -> Option<SpsHevc> {
    if nal.len() < 8 || (nal[0] >> 1) & 0x3F != 33 {
        return None;
    }
    let rbsp = unescape(&nal[2..]);
    let mut b = Bits::new(&rbsp);
    b.u(4)?; // vps id
    let max_sub = b.u(3)? as usize;
    b.bit()?;
    // profile_tier_level
    let mut s = SpsHevc::default();
    for (i, v) in s.ptl.iter_mut().enumerate() {
        *v = b.u(8)? as u8;
        let _ = i;
    }
    // general: 2+1+5 bits, 32 compat, 48 flags, 8 level => lemos 12 bytes (1+4+6+1)
    let mut sub_profile = vec![];
    let mut sub_level = vec![];
    for _ in 0..max_sub {
        sub_profile.push(b.bit()?);
        sub_level.push(b.bit()?);
    }
    if max_sub > 0 {
        for _ in max_sub..8 {
            b.u(2)?;
        }
    }
    for i in 0..max_sub {
        if sub_profile[i] == 1 {
            b.skip(88);
        }
        if sub_level[i] == 1 {
            b.skip(8);
        }
    }
    b.ue()?; // sps id
    s.chroma = b.ue()?;
    if s.chroma == 3 {
        b.bit()?;
    }
    s.width = b.ue()?;
    s.height = b.ue()?;
    if b.bit()? == 1 {
        let (l, r, t, bt) = (b.ue()?, b.ue()?, b.ue()?, b.ue()?);
        let (sw, sh) = match s.chroma {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        s.width = s.width.checked_sub((l + r) * sw)?;
        s.height = s.height.checked_sub((t + bt) * sh)?;
    }
    s.bit_depth_luma = b.ue()? + 8;
    s.bit_depth_chroma = b.ue()? + 8;
    Some(s)
}

// -------------------------------------------------------------- amostras

/// Configuração de codec (SPS/PPS/VPS) extraída de um vídeo íntegro da mesma câmera.
#[derive(Clone, Debug)]
pub struct MediaConfig {
    pub codec: Codec,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
    pub vps: Vec<Vec<u8>>,
}

/// Procura o `avcC`/`hvcC` de um MP4 completo (lê até `max` bytes a partir de `off`).
pub fn config_from_mp4(src: &dyn Source, off: u64, max: u64) -> Option<MediaConfig> {
    // O moov costuma estar no começo ou no fim: procura nos dois lugares.
    let size = src.size();
    let head = read_vec(src, off, max.min(4 << 20) as usize).ok()?;
    let mut hay: Vec<Vec<u8>> = vec![head];
    if max > (4 << 20) && off + max <= size {
        let tail_off = off + max - (4 << 20).min(max);
        hay.push(read_vec(src, tail_off, (off + max - tail_off) as usize).ok()?);
    }
    for h in hay {
        for i in 4..h.len().saturating_sub(12) {
            let tag = &h[i..i + 4];
            if tag == b"avcC" {
                let c = &h[i + 4..];
                if c.len() > 8 && c[0] == 1 {
                    let mut p = 6;
                    let mut sps = vec![];
                    for _ in 0..(c[5] & 0x1F) {
                        let l = u16::from_be_bytes([*c.get(p)?, *c.get(p + 1)?]) as usize;
                        sps.push(c.get(p + 2..p + 2 + l)?.to_vec());
                        p += 2 + l;
                    }
                    let np = *c.get(p)? as usize;
                    p += 1;
                    let mut pps = vec![];
                    for _ in 0..np {
                        let l = u16::from_be_bytes([*c.get(p)?, *c.get(p + 1)?]) as usize;
                        pps.push(c.get(p + 2..p + 2 + l)?.to_vec());
                        p += 2 + l;
                    }
                    if !sps.is_empty() && !pps.is_empty() {
                        return Some(MediaConfig {
                            codec: Codec::Avc,
                            sps,
                            pps,
                            vps: vec![],
                        });
                    }
                }
            } else if tag == b"hvcC" {
                let c = &h[i + 4..];
                if c.len() > 23 && c[0] == 1 {
                    let n = c[22] as usize;
                    let mut p = 23;
                    let (mut vps, mut sps, mut pps) = (vec![], vec![], vec![]);
                    for _ in 0..n {
                        let t = c.get(p)? & 0x3F;
                        let cnt = u16::from_be_bytes([*c.get(p + 1)?, *c.get(p + 2)?]) as usize;
                        p += 3;
                        for _ in 0..cnt {
                            let l = u16::from_be_bytes([*c.get(p)?, *c.get(p + 1)?]) as usize;
                            let nal = c.get(p + 2..p + 2 + l)?.to_vec();
                            match t {
                                32 => vps.push(nal),
                                33 => sps.push(nal),
                                34 => pps.push(nal),
                                _ => {}
                            }
                            p += 2 + l;
                        }
                    }
                    if !sps.is_empty() && !pps.is_empty() {
                        return Some(MediaConfig {
                            codec: Codec::Hevc,
                            sps,
                            pps,
                            vps,
                        });
                    }
                }
            }
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Avc,
    Hevc,
}

#[derive(Clone, Debug)]
struct Sample {
    /// Posição absoluta (na origem) do primeiro byte da amostra (prefixo de tamanho incluso).
    off: u64,
    size: u32,
    sync: bool,
    poc: Option<i64>,
    idr: bool,
}

pub struct Rebuilt {
    /// Bytes do arquivo original antes do `mdat` (ftyp etc.).
    pub pre_len: u64,
    /// Onde começam e terminam os dados dos quadros na origem.
    pub payload: (u64, u64),
    pub moov: Vec<u8>,
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub codec: Codec,
    pub fps: f64,
    pub skipped_bytes: u64,
}

impl Rebuilt {
    /// Cabeçalho `mdat` de 64 bits para o payload.
    pub fn mdat_header(&self) -> Vec<u8> {
        let size = 16 + (self.payload.1 - self.payload.0);
        let mut v = Vec::with_capacity(16);
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(b"mdat");
        v.extend_from_slice(&size.to_be_bytes());
        v
    }
}

fn avc_nal_ok(b: u8) -> bool {
    b & 0x80 == 0 && matches!(b & 0x1F, 1 | 5..=12)
}
fn hevc_nal_ok(b0: u8, b1: u8) -> bool {
    b0 & 0x81 == 0 && ((b0 >> 1) & 0x3F) <= 40 && b1 & 0x07 != 0 && (b1 >> 3) == 0
}

/// Cabeçalho do átomo `mdat` que começa em `at`: (início dos dados, tamanho declarado ou 0).
fn mdat_at(src: &dyn Source, at: u64) -> Option<(u64, u64)> {
    let h = read_vec(src, at, 16).ok()?;
    if h.len() < 8 || &h[4..8] != b"mdat" {
        return None;
    }
    let sz = be32(&h, 0) as u64;
    if sz == 1 && h.len() >= 16 {
        Some((at + 16, u64::from_be_bytes(h[8..16].try_into().unwrap())))
    } else {
        Some((at + 8, sz))
    }
}

/// Reconstrói o índice de um MP4 que começa em `off`, sem `moov`. `avail` limita a leitura.
pub fn rebuild(src: &dyn Source, off: u64, avail: u64, refs: &[MediaConfig]) -> Option<Rebuilt> {
    if let Some(r) = rebuild_with(src, off, avail, None) {
        return Some(r);
    }
    refs.iter()
        .find_map(|c| rebuild_with(src, off, avail, Some(c)))
}

fn rebuild_with(
    src: &dyn Source,
    off: u64,
    avail: u64,
    preset: Option<&MediaConfig>,
) -> Option<Rebuilt> {
    // Átomos antes do mdat.
    let mut p = 0u64;
    let mut pre_end = None;
    for _ in 0..16 {
        let h = read_vec(src, off + p, 16).ok()?;
        if h.len() < 8 {
            return None;
        }
        let ty = &h[4..8];
        if ty == b"mdat" {
            pre_end = Some(p);
            break;
        }
        if !matches!(
            ty,
            b"ftyp" | b"free" | b"skip" | b"wide" | b"uuid" | b"pnot" | b"junk" | b"styp"
        ) {
            return None;
        }
        let sz = be32(&h, 0) as u64;
        if sz < 8 {
            return None;
        }
        p += sz;
    }
    let pre_len = pre_end?;
    let (data_start, declared) = mdat_at(src, off + pre_len)?;
    let hard_end = off + avail.min(src.size() - off);
    let data_end_decl = if declared == 0 {
        hard_end
    } else {
        (off + pre_len + declared).min(hard_end)
    };

    // Lê o payload em blocos, interpretando NALs.
    let mut samples: Vec<Sample> = vec![];
    let mut cache: Vec<u8> = vec![];
    let mut cache_off = data_start;
    let read_at = |o: u64, n: usize, cache: &mut Vec<u8>, cache_off: &mut u64| -> Option<Vec<u8>> {
        if o < *cache_off || o + n as u64 > *cache_off + cache.len() as u64 {
            let want = (data_end_decl.saturating_sub(o) as usize)
                .min(8 << 20)
                .max(n)
                .min((hard_end - o) as usize);
            *cache = read_vec(src, o, want).ok()?;
            *cache_off = o;
        }
        let s = (o - *cache_off) as usize;
        cache.get(s..s + n).map(|x| x.to_vec())
    };
    // Detecta o codec: o que forma a cadeia mais longa de unidades NAL válidas.
    let chain = |c: Codec| -> usize {
        let Ok(head) = read_vec(src, data_start, 64 << 10) else {
            return 0;
        };
        let mut p = 0usize;
        let mut n = 0;
        while p + 6 <= head.len() {
            let l = be32(&head, p) as usize;
            let ok = (1..=8 << 20).contains(&l)
                && match c {
                    Codec::Avc => avc_nal_ok(head[p + 4]),
                    Codec::Hevc => hevc_nal_ok(head[p + 4], head[p + 5]),
                };
            if !ok {
                break;
            }
            n += 1;
            p += 4 + l;
        }
        n
    };
    let codec = match preset {
        Some(c) => Some(c.codec),
        None => {
            let (a, h) = (chain(Codec::Avc), chain(Codec::Hevc));
            if a == 0 && h == 0 {
                None
            } else if a >= h {
                Some(Codec::Avc)
            } else {
                Some(Codec::Hevc)
            }
        }
    };
    let codec = codec?;
    let mut pos = data_start;
    let mut payload_end = data_start;
    let mut sps_nals: Vec<Vec<u8>> = vec![];
    let mut pps_nals: Vec<Vec<u8>> = vec![];
    let mut vps_nals: Vec<Vec<u8>> = vec![];
    let mut sps264: Option<Sps264> = None;
    let mut sps265: Option<SpsHevc> = None;
    let mut pic_order_present = false;
    if let Some(c) = preset {
        sps_nals = c.sps.clone();
        pps_nals = c.pps.clone();
        vps_nals = c.vps.clone();
        match codec {
            Codec::Avc => {
                sps264 = c.sps.iter().find_map(|n| parse_sps264(n));
                pic_order_present = c
                    .pps
                    .first()
                    .map(|p| pps_pic_order_present(p))
                    .unwrap_or(false);
            }
            Codec::Hevc => sps265 = c.sps.iter().find_map(|n| parse_sps_hevc(n)),
        }
    }
    let mut idr_total = 0usize;
    let mut idr_bad = 0usize;
    let mut cur: Option<Sample> = None;
    let mut group_start: Option<u64> = None; // início do grupo de NALs não-VCL que precede um quadro
    let mut nal_count = 0usize;
    let mut max_len = 0u64;
    // POC (H.264, tipo 0)
    let mut prev_msb: i64 = 0;
    let mut prev_lsb: i64 = 0;

    'outer: while pos + 6 <= data_end_decl {
        let hdr = read_at(pos, 6, &mut cache, &mut cache_off)?;
        let len = be32(&hdr, 0) as u64;
        let ok = (1..=16 << 20).contains(&len)
            && pos + 4 + len <= data_end_decl
            && match codec {
                Codec::Avc => avc_nal_ok(hdr[4]),
                Codec::Hevc => hevc_nal_ok(hdr[4], hdr[5]),
            };
        let mut ok = ok;
        if ok && nal_count >= 3 && len > (max_len * 6).max(1 << 20) {
            // Um "tamanho" muito maior que os já vistos é lixo (áudio lido como vídeo).
            ok = false;
        }
        if ok {
            let head = read_at(pos + 4, (len as usize).min(48), &mut cache, &mut cache_off)?;
            ok = nal_plausible(codec, &head, len, sps264.as_ref(), pps_nals.len());
        }
        let valid = ok;
        if valid {
            max_len = max_len.max(len);
        }
        if !valid {
            // Pode ser áudio intercalado: procura a próxima unidade NAL plausível.
            let found = scan_nal(
                src,
                codec,
                pos + 1,
                (pos + (2 << 20)).min(data_end_decl),
                data_end_decl,
                sps264.as_ref(),
                pps_nals.len(),
                (max_len * 6).max(1 << 20),
            );
            match found {
                Some(q) if nal_count > 0 => {
                    // O que veio antes fecha o quadro atual.
                    if let Some(mut s) = cur.take() {
                        s.size = (group_start.unwrap_or(pos) - s.off) as u32;
                        samples.push(s);
                    }
                    group_start = None;
                    pos = q;
                    continue 'outer;
                }
                _ => break 'outer,
            }
        }
        let nal_start = pos + 4;
        let nal = read_at(
            nal_start,
            len.min(64 << 10) as usize,
            &mut cache,
            &mut cache_off,
        )?;
        let (is_vcl, first_of_pic, is_idr, ty) = match codec {
            Codec::Avc => {
                let t = nal[0] & 0x1F;
                match t {
                    1 | 2 | 5 => {
                        let first = sps264
                            .as_ref()
                            .and_then(|s| slice264(&nal, s, pic_order_present));
                        (
                            true,
                            first.as_ref().map(|f| f.first_mb0).unwrap_or(true),
                            t == 5,
                            t,
                        )
                    }
                    7 => {
                        if sps264.is_none() {
                            sps264 = parse_sps264(&nal);
                        }
                        if !sps_nals.contains(&nal) && sps_nals.len() < 4 {
                            sps_nals.push(nal.clone());
                        }
                        (false, false, false, t)
                    }
                    8 => {
                        if pps_nals.is_empty() {
                            pic_order_present = pps_pic_order_present(&nal);
                        }
                        if !pps_nals.contains(&nal) && pps_nals.len() < 4 {
                            pps_nals.push(nal.clone());
                        }
                        (false, false, false, t)
                    }
                    _ => (false, false, false, t),
                }
            }
            Codec::Hevc => {
                let t = (nal[0] >> 1) & 0x3F;
                match t {
                    0..=9 | 16..=21 => (
                        true,
                        nal.get(2).map(|b| b & 0x80 != 0).unwrap_or(true),
                        (16..=21).contains(&t),
                        t,
                    ),
                    32 => {
                        if !vps_nals.contains(&nal) && vps_nals.len() < 2 {
                            vps_nals.push(nal.clone());
                        }
                        (false, false, false, t)
                    }
                    33 => {
                        if sps265.is_none() {
                            sps265 = parse_sps_hevc(&nal);
                        }
                        if !sps_nals.contains(&nal) && sps_nals.len() < 4 {
                            sps_nals.push(nal.clone());
                        }
                        (false, false, false, t)
                    }
                    34 => {
                        if !pps_nals.contains(&nal) && pps_nals.len() < 4 {
                            pps_nals.push(nal.clone());
                        }
                        (false, false, false, t)
                    }
                    _ => (false, false, false, t),
                }
            }
        };
        let _ = ty;
        if is_vcl {
            if first_of_pic {
                // fecha o quadro anterior
                if let Some(mut s) = cur.take() {
                    // tamanho até o início do grupo atual (ou deste NAL)
                    let end = group_start.unwrap_or(pos);
                    s.size = (end - s.off) as u32;
                    samples.push(s);
                }
                let start = group_start.take().unwrap_or(pos);
                let mut poc = None;
                if codec == Codec::Avc {
                    if let Some(sp) = &sps264 {
                        if let Some(si) = slice264(&nal, sp, pic_order_present) {
                            if si.idr {
                                prev_msb = 0;
                                prev_lsb = 0;
                                idr_total += 1;
                                if si.frame_num != 0 {
                                    idr_bad += 1;
                                }
                            }
                            if let Some(lsb) = si.poc_lsb {
                                let max = 1i64 << sp.log2_max_poc_lsb;
                                let lsb = lsb as i64;
                                let msb = if lsb < prev_lsb && prev_lsb - lsb >= max / 2 {
                                    prev_msb + max
                                } else if lsb > prev_lsb && lsb - prev_lsb > max / 2 {
                                    prev_msb - max
                                } else {
                                    prev_msb
                                };
                                poc = Some(msb + lsb);
                                if nal[0] & 0x60 != 0 {
                                    prev_msb = msb;
                                    prev_lsb = lsb;
                                }
                                let _ = si.bottom;
                            }
                        }
                    }
                }
                cur = Some(Sample {
                    off: start,
                    size: 0,
                    sync: is_idr,
                    poc,
                    idr: is_idr,
                });
            }
            if !first_of_pic {
                group_start = None;
            }
        } else if cur.is_some() && group_start.is_none() {
            group_start = Some(pos);
        } else if cur.is_none() && group_start.is_none() {
            group_start = Some(pos);
        }
        pos = nal_start + len;
        nal_count += 1;
        payload_end = pos;
    }
    if let Some(mut s) = cur.take() {
        s.size = (payload_end - s.off) as u32;
        samples.push(s);
    }
    samples.retain(|s| s.size > 4);
    if samples.len() < 2 || (idr_total > 0 && idr_bad * 4 > idr_total) {
        return None;
    }
    let (width, height, timing, profile_info) = match codec {
        Codec::Avc => {
            let s = sps264.clone()?;
            (s.width, s.height, s.timing, None::<SpsHevc>)
        }
        Codec::Hevc => {
            let s = sps265.clone()?;
            (s.width, s.height, s.timing, Some(s))
        }
    };
    if width == 0 || height == 0 || sps_nals.is_empty() || pps_nals.is_empty() {
        return None;
    }
    let (num, den) = match timing {
        Some((units, scale)) if codec == Codec::Avc => (scale, 2 * units), // quadros por segundo = scale / (2 * units)
        Some((units, scale)) => (scale, units),
        None => (30, 1),
    };
    let fps = num as f64 / den as f64;
    let (num, den) = if !(1.0..=240.0).contains(&fps) {
        (30, 1)
    } else {
        (num, den)
    };
    let fps = num as f64 / den as f64;
    // Escala: tique do quadro = den, escala de tempo = num.
    let (timescale, dur) = (num, den);

    // Ordem de apresentação (POC) por trecho entre IDR.
    let mut ctts: Option<Vec<i64>> = None;
    if samples.iter().all(|s| s.poc.is_some()) {
        let mut offs = vec![0i64; samples.len()];
        let mut seg_start = 0;
        let mut any = false;
        let mut i = 0;
        while i <= samples.len() {
            if i == samples.len() || (i > seg_start && samples[i].idr) {
                let mut order: Vec<usize> = (seg_start..i).collect();
                order.sort_by_key(|&k| samples[k].poc.unwrap());
                for (rank, &k) in order.iter().enumerate() {
                    let d = rank as i64 - (k - seg_start) as i64;
                    offs[k] = d;
                    any |= d != 0;
                }
                seg_start = i;
            }
            i += 1;
        }
        if any {
            let depth = offs.iter().map(|&d| -d).max().unwrap_or(0).max(0);
            ctts = Some(offs.iter().map(|&d| (d + depth) * dur as i64).collect());
        }
    }
    let reorder_delay: i64 = ctts
        .as_ref()
        .map(|c| c.iter().copied().min().unwrap_or(0))
        .unwrap_or(0);

    let payload = (data_start, payload_end);
    let out_payload_start = pre_len + 16;
    let map_off = |abs: u64| -> u64 { out_payload_start + (abs - data_start) };
    let moov = build_moov(
        codec,
        width,
        height,
        &sps_nals,
        &pps_nals,
        &vps_nals,
        profile_info.as_ref(),
        &sps264,
        &samples,
        timescale,
        dur,
        ctts.as_deref(),
        reorder_delay,
        &map_off,
    );
    let used: u64 = samples.iter().map(|s| s.size as u64).sum();
    Some(Rebuilt {
        pre_len,
        payload,
        moov,
        frames: samples.len(),
        width,
        height,
        codec,
        fps,
        skipped_bytes: (payload_end - data_start).saturating_sub(used),
    })
}

// -------------------------------------------------------------------- boxes

fn bx(ty: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + body.len());
    v.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    v.extend_from_slice(ty);
    v.extend_from_slice(body);
    v
}

fn full(ty: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(ty, &b)
}

fn u32b(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

#[allow(clippy::too_many_arguments)]
fn build_moov(
    codec: Codec,
    width: u32,
    height: u32,
    sps: &[Vec<u8>],
    pps: &[Vec<u8>],
    vps: &[Vec<u8>],
    hevc: Option<&SpsHevc>,
    avc: &Option<Sps264>,
    samples: &[Sample],
    timescale: u32,
    dur: u32,
    ctts: Option<&[i64]>,
    reorder_delay: i64,
    map_off: &dyn Fn(u64) -> u64,
) -> Vec<u8> {
    let n = samples.len();
    let media_dur = n as u64 * dur as u64;
    let movie_ts = 1000u32;
    let movie_dur = media_dur * movie_ts as u64 / timescale as u64;

    // Entrada de amostra
    let mut entry = vec![];
    entry.extend_from_slice(&[0; 6]);
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&[0; 16]);
    entry.extend_from_slice(&(width as u16).to_be_bytes());
    entry.extend_from_slice(&(height as u16).to_be_bytes());
    entry.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    entry.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    entry.extend_from_slice(&0u32.to_be_bytes());
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&[0; 32]);
    entry.extend_from_slice(&0x0018u16.to_be_bytes());
    entry.extend_from_slice(&0xFFFFu16.to_be_bytes());
    let (fourcc, cfg) = match codec {
        Codec::Avc => {
            let s = avc.as_ref().unwrap();
            let mut c = vec![
                1,
                s.profile,
                s.compat,
                s.level,
                0xFF,
                0xE0 | sps.len() as u8,
            ];
            for x in sps {
                c.extend_from_slice(&(x.len() as u16).to_be_bytes());
                c.extend_from_slice(x);
            }
            c.push(pps.len() as u8);
            for x in pps {
                c.extend_from_slice(&(x.len() as u16).to_be_bytes());
                c.extend_from_slice(x);
            }
            (*b"avc1", bx(b"avcC", &c))
        }
        Codec::Hevc => {
            let h = hevc.unwrap();
            let mut c = vec![1];
            c.extend_from_slice(&h.ptl[..12.min(h.ptl.len())]); // espaço/tier/perfil, compat, restrições, nível
            c.truncate(13);
            while c.len() < 13 {
                c.push(0);
            }
            c.extend_from_slice(&0xF000u16.to_be_bytes()); // segmentação espacial
            c.push(0xFC); // paralelismo
            c.push(0xFC | h.chroma as u8);
            c.push(0xF8 | (h.bit_depth_luma - 8) as u8);
            c.push(0xF8 | (h.bit_depth_chroma - 8) as u8);
            c.extend_from_slice(&0u16.to_be_bytes()); // avgFrameRate
            c.push(0x0B); // constFrameRate=0, 1 camada temporal, lengthSizeMinusOne=3
            let arrays: [(u8, &[Vec<u8>]); 3] = [(32, vps), (33, sps), (34, pps)];
            let present: Vec<_> = arrays.iter().filter(|(_, v)| !v.is_empty()).collect();
            c.push(present.len() as u8);
            for (t, v) in present {
                c.push(0x80 | t);
                c.extend_from_slice(&(v.len() as u16).to_be_bytes());
                for x in v.iter() {
                    c.extend_from_slice(&(x.len() as u16).to_be_bytes());
                    c.extend_from_slice(x);
                }
            }
            (*b"hvc1", bx(b"hvcC", &c))
        }
    };
    entry.extend_from_slice(&cfg);
    let sample_entry = bx(&fourcc, &entry);
    let mut stsd_body = u32b(1).to_vec();
    stsd_body.extend_from_slice(&sample_entry);
    let stsd = full(b"stsd", 0, 0, &stsd_body);

    let stts = full(
        b"stts",
        0,
        0,
        &[u32b(1).as_slice(), &u32b(n as u32), &u32b(dur)].concat(),
    );
    let mut stsz_body = u32b(0).to_vec();
    stsz_body.extend_from_slice(&u32b(n as u32));
    for s in samples {
        stsz_body.extend_from_slice(&u32b(s.size));
    }
    let stsz = full(b"stsz", 0, 0, &stsz_body);
    let stsc = full(
        b"stsc",
        0,
        0,
        &[u32b(1).as_slice(), &u32b(1), &u32b(1), &u32b(1)].concat(),
    );
    let need64 = samples.iter().any(|s| map_off(s.off) > u32::MAX as u64);
    let stco = if need64 {
        let mut b = u32b(n as u32).to_vec();
        for s in samples {
            b.extend_from_slice(&map_off(s.off).to_be_bytes());
        }
        full(b"co64", 0, 0, &b)
    } else {
        let mut b = u32b(n as u32).to_vec();
        for s in samples {
            b.extend_from_slice(&u32b(map_off(s.off) as u32));
        }
        full(b"stco", 0, 0, &b)
    };
    let syncs: Vec<u32> = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.sync)
        .map(|(i, _)| i as u32 + 1)
        .collect();
    let mut stbl_parts = vec![stsd, stts];
    if let Some(c) = ctts {
        // entradas agrupadas
        let mut runs: Vec<(u32, u32)> = vec![];
        for &v in c {
            match runs.last_mut() {
                Some(l) if l.1 == v as u32 => l.0 += 1,
                _ => runs.push((1, v as u32)),
            }
        }
        let mut b = u32b(runs.len() as u32).to_vec();
        for (cnt, v) in runs {
            b.extend_from_slice(&u32b(cnt));
            b.extend_from_slice(&u32b(v));
        }
        stbl_parts.push(full(b"ctts", 0, 0, &b));
    }
    if !syncs.is_empty() && syncs.len() < n {
        let mut b = u32b(syncs.len() as u32).to_vec();
        for s in &syncs {
            b.extend_from_slice(&u32b(*s));
        }
        stbl_parts.push(full(b"stss", 0, 0, &b));
    }
    stbl_parts.push(stsc);
    stbl_parts.push(stsz);
    stbl_parts.push(stco);
    let stbl = bx(b"stbl", &stbl_parts.concat());

    let url = full(b"url ", 0, 1, &[]);
    let dref = full(b"dref", 0, 0, &[u32b(1).as_slice(), &url].concat());
    let dinf = bx(b"dinf", &dref);
    let vmhd = full(b"vmhd", 0, 1, &[0; 8]);
    let minf = bx(b"minf", &[vmhd, dinf, stbl].concat());
    let mut mdhd_body = vec![];
    mdhd_body.extend_from_slice(&[0; 8]); // criação/modificação
    mdhd_body.extend_from_slice(&u32b(timescale));
    mdhd_body.extend_from_slice(&u32b(media_dur as u32));
    mdhd_body.extend_from_slice(&0x55C4u16.to_be_bytes()); // "und"
    mdhd_body.extend_from_slice(&[0; 2]);
    let mdhd = full(b"mdhd", 0, 0, &mdhd_body);
    let mut hdlr_body = vec![0; 4];
    hdlr_body.extend_from_slice(b"vide");
    hdlr_body.extend_from_slice(&[0; 12]);
    hdlr_body.extend_from_slice(b"Guara\0");
    let hdlr = full(b"hdlr", 0, 0, &hdlr_body);
    let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());

    let matrix: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];
    let mut tkhd_body = vec![];
    tkhd_body.extend_from_slice(&[0; 8]);
    tkhd_body.extend_from_slice(&u32b(1)); // track id
    tkhd_body.extend_from_slice(&[0; 4]);
    tkhd_body.extend_from_slice(&u32b(movie_dur as u32));
    tkhd_body.extend_from_slice(&[0; 8]);
    tkhd_body.extend_from_slice(&[0; 4]); // camada, grupo
    tkhd_body.extend_from_slice(&[0; 4]); // volume, reservado
    for m in matrix {
        tkhd_body.extend_from_slice(&u32b(m));
    }
    tkhd_body.extend_from_slice(&u32b(width << 16));
    tkhd_body.extend_from_slice(&u32b(height << 16));
    let tkhd = full(b"tkhd", 0, 3, &tkhd_body);
    let mut trak_parts = vec![tkhd];
    if reorder_delay > 0 {
        let media_time = reorder_delay;
        let mut e = u32b(1).to_vec();
        e.extend_from_slice(&u32b(movie_dur as u32));
        e.extend_from_slice(&u32b(media_time as u32));
        e.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        trak_parts.push(bx(b"edts", &full(b"elst", 0, 0, &e)));
    }
    trak_parts.push(mdia);
    let trak = bx(b"trak", &trak_parts.concat());

    let mut mvhd_body = vec![];
    mvhd_body.extend_from_slice(&[0; 8]);
    mvhd_body.extend_from_slice(&u32b(movie_ts));
    mvhd_body.extend_from_slice(&u32b(movie_dur as u32));
    mvhd_body.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    mvhd_body.extend_from_slice(&0x0100u16.to_be_bytes());
    mvhd_body.extend_from_slice(&[0; 10]);
    for m in matrix {
        mvhd_body.extend_from_slice(&u32b(m));
    }
    mvhd_body.extend_from_slice(&[0; 24]);
    mvhd_body.extend_from_slice(&u32b(2));
    let mvhd = full(b"mvhd", 0, 0, &mvhd_body);
    bx(b"moov", &[mvhd, trak].concat())
}

/// A unidade NAL parece de verdade? Valida o cabeçalho de fatia e o tamanho de SPS/PPS/AUD.
fn nal_plausible(
    codec: Codec,
    head: &[u8],
    len: u64,
    sps: Option<&Sps264>,
    pps_count: usize,
) -> bool {
    match codec {
        Codec::Avc => {
            let t = head[0] & 0x1F;
            match t {
                1 | 2 | 5 => {
                    let rbsp = unescape(&head[1..]);
                    let mut b = Bits::new(&rbsp);
                    let (Some(first), Some(st), Some(pps)) = (b.ue(), b.ue(), b.ue()) else {
                        return false;
                    };
                    if st > 9
                        || pps > 255
                        || (pps_count > 0 && pps as usize >= pps_count.max(1) && pps > 3)
                    {
                        return false;
                    }
                    if t == 5 && !matches!(st, 2 | 4 | 7 | 9) {
                        return false;
                    }
                    if first > 8191 {
                        return false;
                    }
                    if let Some(sp) = sps {
                        // frame_num de IDR é sempre 0
                        if t == 5 {
                            if let Some(fnum) = b.u(sp.log2_max_frame_num) {
                                return fnum == 0;
                            }
                        }
                    }
                    true
                }
                9 => len == 2,
                7 => (4..=300).contains(&len),
                8 => (2..=300).contains(&len),
                6 => len >= 2,
                _ => true,
            }
        }
        Codec::Hevc => {
            let t = (head[0] >> 1) & 0x3F;
            match t {
                35 => len <= 4,
                32..=34 => (4..=1000).contains(&len),
                _ => true,
            }
        }
    }
}

/// Quantas unidades NAL válidas seguidas (sem vãos) começam em `at` (até `max`).
fn chain_len(
    buf: &[u8],
    base: u64,
    at: u64,
    codec: Codec,
    sps: Option<&Sps264>,
    pps_count: usize,
    cap: u64,
    max: usize,
) -> usize {
    let mut p = (at - base) as usize;
    let mut n = 0;
    while n < max && p + 8 <= buf.len() {
        let l = be32(buf, p) as u64;
        let ok = l >= 1
            && l <= cap
            && p as u64 + 4 + l <= buf.len() as u64 + 8
            && match codec {
                Codec::Avc => avc_nal_ok(buf[p + 4]),
                Codec::Hevc => hevc_nal_ok(buf[p + 4], buf[p + 5]),
            }
            && nal_plausible(
                codec,
                &buf[p + 4..(p + 4 + (l as usize).min(48)).min(buf.len())],
                l,
                sps,
                pps_count,
            );
        if !ok {
            break;
        }
        n += 1;
        p += 4 + l as usize;
    }
    n
}

/// Próxima posição em [from, limit) onde começa uma sequência de unidades NAL plausíveis.
/// Prefere o primeiro ponto com duas ou mais unidades seguidas; senão, uma unidade isolada.
fn scan_nal(
    src: &dyn Source,
    codec: Codec,
    from: u64,
    limit: u64,
    data_end: u64,
    sps: Option<&Sps264>,
    pps_count: usize,
    cap: u64,
) -> Option<u64> {
    if limit <= from + 8 {
        return None;
    }
    // Lê um pouco além do limite para poder seguir a cadeia de unidades.
    let want =
        ((limit - from) as usize + (cap as usize).min(4 << 20)).min((data_end - from) as usize);
    let buf = read_vec(src, from, want).ok()?;
    let mut single = None;
    let mut i = 0usize;
    while i + 8 < buf.len() && from + (i as u64) < limit {
        let q = from + i as u64;
        let l2 = be32(&buf, i) as u64;
        if l2 >= 2 && l2 <= cap && q + 4 + l2 <= data_end {
            let nb = buf[i + 4];
            let good_head = match codec {
                // Um quadro novo começa por parâmetros/AUD/SEI ou por fatia do macrobloco 0.
                Codec::Avc => {
                    avc_nal_ok(nb)
                        && (matches!(nb & 0x1F, 6..=9)
                            || (matches!(nb & 0x1F, 1 | 5) && buf[i + 5] & 0x80 != 0))
                }
                Codec::Hevc => hevc_nal_ok(nb, buf[i + 5]),
            };
            if good_head {
                let c = chain_len(&buf, from, q, codec, sps, pps_count, cap, 3);
                if c >= 2 {
                    return Some(q);
                }
                if c == 1 && single.is_none() {
                    single = Some(q);
                }
            }
        }
        i += 1;
    }
    single
}
