//! Decodificador de entropia de JPEG sequencial (Huffman, 8 bits), sem IDCT.
//!
//! Serve para validar se os bytes de um JPEG são realmente contínuos e para reconstruir arquivos
//! fragmentados: o fluxo é decodificado MCU a MCU, guardando o DC de luminância (uma miniatura
//! 1/8 da imagem) e pontos de retomada. Depois de uma quebra os bytes viram lixo, e a
//! descontinuidade do DC (ou um erro de código) denuncia onde.

#[derive(Clone, Debug)]
struct Huff {
    // decodificação canônica
    mincode: [i32; 17],
    maxcode: [i32; 18],
    valptr: [i32; 17],
    vals: Vec<u8>,
}

impl Huff {
    fn new(counts: &[u8; 16], vals: Vec<u8>) -> Huff {
        let mut h = Huff {
            mincode: [0; 17],
            maxcode: [-1; 18],
            valptr: [0; 17],
            vals,
        };
        let mut code = 0i32;
        let mut k = 0i32;
        for l in 1..=16 {
            h.valptr[l] = k;
            h.mincode[l] = code;
            code += counts[l - 1] as i32;
            k += counts[l - 1] as i32;
            h.maxcode[l] = if counts[l - 1] > 0 { code - 1 } else { -1 };
            code <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        h
    }
}

#[derive(Clone, Debug)]
pub struct Comp {
    pub h: u8,
    pub v: u8,
    pub tq: usize,
    pub dc_tbl: usize,
    pub ac_tbl: usize,
}

#[derive(Clone, Debug)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub comps: Vec<Comp>,
    pub restart: u32,
    pub mcus_x: u32,
    pub mcus_y: u32,
    /// Onde começam os dados entropy-coded (depois do SOS).
    pub scan_start: usize,
    /// Tabelas de quantização (ordem zigzag).
    pub qt: [[u16; 64]; 4],
    dc: [Option<Huff>; 4],
    ac: [Option<Huff>; 4],
}

impl Header {
    pub fn total_mcus(&self) -> u32 {
        self.mcus_x * self.mcus_y
    }
}

/// Lê os segmentos até o SOS. `None` se não for JPEG sequencial suportado.
pub fn parse_header(d: &[u8]) -> Option<Header> {
    if d.len() < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        return None;
    }
    let mut p = 2;
    let mut dc: [Option<Huff>; 4] = Default::default();
    let mut ac: [Option<Huff>; 4] = Default::default();
    let mut sof: Option<(u32, u32, Vec<(u8, u8, usize)>)> = None;
    let mut qt = [[1u16; 64]; 4];
    let mut restart = 0u32;
    while p + 4 <= d.len() {
        if d[p] != 0xFF {
            return None;
        }
        let m = d[p + 1];
        if m == 0xFF {
            p += 1;
            continue;
        }
        if (0xD0..=0xD8).contains(&m) || m == 0x01 {
            p += 2;
            continue;
        }
        let len = u16::from_be_bytes([d[p + 2], d[p + 3]]) as usize;
        if len < 2 || p + 2 + len > d.len() {
            return None;
        }
        let seg = &d[p + 4..p + 2 + len];
        match m {
            0xC0 | 0xC1 => {
                if seg.len() < 6 || seg[0] != 8 {
                    return None;
                }
                let h = u16::from_be_bytes([seg[1], seg[2]]) as u32;
                let w = u16::from_be_bytes([seg[3], seg[4]]) as u32;
                let n = seg[5] as usize;
                if n == 0 || n > 4 || seg.len() < 6 + n * 3 || w == 0 || h == 0 {
                    return None;
                }
                let comps = (0..n)
                    .map(|i| {
                        (
                            seg[7 + i * 3] >> 4,
                            seg[7 + i * 3] & 15,
                            seg[8 + i * 3] as usize & 3,
                        )
                    })
                    .collect();
                sof = Some((w, h, comps));
            }
            0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None, // progressivo, aritmético etc.
            0xC4 => {
                let mut q = 0;
                while q + 17 <= seg.len() {
                    let (cls, id) = (seg[q] >> 4, (seg[q] & 15) as usize);
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&seg[q + 1..q + 17]);
                    let n: usize = counts.iter().map(|&c| c as usize).sum();
                    if id > 3 || q + 17 + n > seg.len() || n > 256 {
                        return None;
                    }
                    let h = Huff::new(&counts, seg[q + 17..q + 17 + n].to_vec());
                    if cls == 0 {
                        dc[id] = Some(h);
                    } else {
                        ac[id] = Some(h);
                    }
                    q += 17 + n;
                }
            }
            0xDB => {
                let mut q = 0;
                while q < seg.len() {
                    let (prec, id) = (seg[q] >> 4, (seg[q] & 15) as usize);
                    q += 1;
                    let n = if prec == 0 { 64 } else { 128 };
                    if id > 3 || q + n > seg.len() {
                        return None;
                    }
                    for i in 0..64 {
                        qt[id][i] = if prec == 0 {
                            seg[q + i] as u16
                        } else {
                            u16::from_be_bytes([seg[q + 2 * i], seg[q + 2 * i + 1]])
                        };
                    }
                    q += n;
                }
            }
            0xDD => {
                if seg.len() >= 2 {
                    restart = u16::from_be_bytes([seg[0], seg[1]]) as u32;
                }
            }
            0xDA => {
                let (w, h, comps) = sof?;
                let ns = *seg.first()? as usize;
                if ns != comps.len() || seg.len() < 1 + ns * 2 + 3 {
                    return None; // varredura não intercalada
                }
                let mut cs = vec![];
                for i in 0..ns {
                    let t = seg[2 + i * 2];
                    cs.push(Comp {
                        h: comps[i].0,
                        v: comps[i].1,
                        tq: comps[i].2,
                        dc_tbl: (t >> 4) as usize & 3,
                        ac_tbl: (t & 15) as usize & 3,
                    });
                }
                let hmax = cs.iter().map(|c| c.h).max()? as u32;
                let vmax = cs.iter().map(|c| c.v).max()? as u32;
                if hmax == 0 || vmax == 0 || hmax > 4 || vmax > 4 {
                    return None;
                }
                for c in &cs {
                    if c.h == 0 || c.v == 0 || dc[c.dc_tbl].is_none() || ac[c.ac_tbl].is_none() {
                        return None;
                    }
                }
                return Some(Header {
                    width: w,
                    height: h,
                    comps: cs,
                    restart,
                    mcus_x: w.div_ceil(8 * hmax),
                    mcus_y: h.div_ceil(8 * vmax),
                    scan_start: p + 2 + len,
                    qt,
                    dc,
                    ac,
                });
            }
            _ => {}
        }
        p += 2 + len;
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Todos os MCUs foram decodificados.
    Done,
    /// Marcador inesperado (inclusive EOI) na posição dada.
    Marker(u8, usize),
    /// Código inválido ou coeficiente fora do limite.
    Error(usize),
    /// Faltam bytes.
    Truncated,
}

/// Estado completo do decodificador num limite de byte, para retomar com outros dados.
#[derive(Clone, Debug)]
pub struct State {
    /// Próximo byte a buscar no fluxo.
    pub pos: usize,
    bitbuf: u32,
    bitcnt: u32,
    pred: [i32; 4],
    pub mcu: u32,
    rst_next: u8,
    mcus_since_rst: u32,
}

impl State {
    /// Último DC de luminância (preditor) no ponto de retomada.
    pub fn luma_pred(&self) -> i32 {
        self.pred[0]
    }
    pub fn start(h: &Header) -> State {
        State {
            pos: h.scan_start,
            bitbuf: 0,
            bitcnt: 0,
            pred: [0; 4],
            mcu: 0,
            rst_next: 0,
            mcus_since_rst: 0,
        }
    }
}

pub struct Decoded {
    /// DC de luminância (absoluto) por MCU decodificado.
    pub dc: Vec<i32>,
    /// Estados guardados em intervalos (para retomada): (MCU, estado ao fim dele).
    pub snaps: Vec<State>,
    pub stop: Stop,
    pub state: State,
}

struct Bits<'a> {
    d: &'a [u8],
    st: State,
    marker: Option<(u8, usize)>,
}

impl Bits<'_> {
    fn fill(&mut self) {
        while self.st.bitcnt <= 24 && self.marker.is_none() {
            let Some(&b) = self.d.get(self.st.pos) else {
                break;
            };
            if b == 0xFF {
                match self.d.get(self.st.pos + 1) {
                    Some(0) => {
                        self.st.bitbuf |= 0xFF << (24 - self.st.bitcnt);
                        self.st.bitcnt += 8;
                        self.st.pos += 2;
                    }
                    Some(&m) => {
                        self.marker = Some((m, self.st.pos));
                    }
                    None => break,
                }
            } else {
                self.st.bitbuf |= (b as u32) << (24 - self.st.bitcnt);
                self.st.bitcnt += 8;
                self.st.pos += 1;
            }
        }
    }
    fn get(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if self.st.bitcnt < n {
            self.fill();
            if self.st.bitcnt < n {
                return None;
            }
        }
        let v = self.st.bitbuf >> (32 - n);
        self.st.bitbuf <<= n;
        self.st.bitcnt -= n;
        Some(v)
    }
    fn huff(&mut self, h: &Huff) -> Option<u8> {
        self.fill();
        let mut code = 0i32;
        for l in 1..=16u32 {
            if self.st.bitcnt < l {
                return None;
            }
            code = (self.st.bitbuf >> (32 - l)) as i32;
            if h.maxcode[l as usize] >= 0
                && code <= h.maxcode[l as usize]
                && code >= h.mincode[l as usize]
            {
                self.st.bitbuf <<= l;
                self.st.bitcnt -= l;
                let idx = h.valptr[l as usize] + code - h.mincode[l as usize];
                return h.vals.get(idx as usize).copied();
            }
        }
        let _ = code;
        None
    }
}

fn extend(v: u32, t: u32) -> i32 {
    if t == 0 {
        0
    } else if v < (1 << (t - 1)) {
        v as i32 - (1 << t) + 1
    } else {
        v as i32
    }
}

/// Decodifica até `max_mcus` MCUs a partir de `state`, sobre `data` (fluxo bruto com FF00).
/// `snap_every` > 0 guarda estados a cada tantos bytes consumidos.
pub fn decode(h: &Header, data: &[u8], state: State, max_mcus: u32, snap_every: usize) -> Decoded {
    decode_hook(h, data, state, max_mcus, snap_every, None)
}

/// Como `decode`, mas entrega os coeficientes (zigzag) dos blocos de luminância de cada MCU.
pub fn decode_hook(
    h: &Header,
    data: &[u8],
    state: State,
    max_mcus: u32,
    snap_every: usize,
    mut hook: Option<&mut dyn FnMut(u32, &[[i32; 64]])>,
) -> Decoded {
    let mut lblocks: Vec<[i32; 64]> = Vec::new();
    let mut b = Bits {
        d: data,
        st: state,
        marker: None,
    };
    let total = h.total_mcus();
    let mut dc_out = Vec::new();
    let mut snaps = Vec::new();
    let mut last_snap = b.st.pos;
    let mut stop = Stop::Done;
    let mut decoded = 0u32;
    'mcu: while b.st.mcu < total && decoded < max_mcus {
        // Marcador de reinício entre MCUs.
        if h.restart > 0 && b.st.mcus_since_rst == h.restart {
            // descarta bits de preenchimento e espera RSTn
            b.st.bitbuf = 0;
            b.st.bitcnt = 0;
            let p = b.st.pos;
            match (data.get(p), data.get(p + 1)) {
                (Some(0xFF), Some(&m)) if m == 0xD0 + b.st.rst_next => {
                    b.st.pos += 2;
                    b.st.rst_next = (b.st.rst_next + 1) & 7;
                    b.st.mcus_since_rst = 0;
                    b.st.pred = [0; 4];
                    b.marker = None;
                }
                (Some(0xFF), Some(&m)) => {
                    stop = Stop::Marker(m, p);
                    break 'mcu;
                }
                (None, _) | (_, None) => {
                    stop = Stop::Truncated;
                    break 'mcu;
                }
                _ => {
                    stop = Stop::Error(p);
                    break 'mcu;
                }
            }
        }
        let mut luma_dc = 0;
        lblocks.clear();
        for (ci, c) in h.comps.iter().enumerate() {
            let dct = h.dc[c.dc_tbl].as_ref().unwrap();
            let act = h.ac[c.ac_tbl].as_ref().unwrap();
            for blk in 0..(c.h as u32 * c.v as u32) {
                let Some(t) = b.huff(dct) else {
                    stop = fail(&b, data);
                    break 'mcu;
                };
                if t > 11 {
                    stop = Stop::Error(b.st.pos);
                    break 'mcu;
                }
                let Some(bits) = b.get(t as u32) else {
                    stop = fail(&b, data);
                    break 'mcu;
                };
                b.st.pred[ci] += extend(bits, t as u32);
                if ci == 0 && blk == 0 {
                    luma_dc = b.st.pred[0];
                }
                let mut coef = [0i32; 64];
                if ci == 0 {
                    coef[0] = b.st.pred[0];
                }
                let mut k = 1;
                while k < 64 {
                    let Some(rs) = b.huff(act) else {
                        stop = fail(&b, data);
                        break 'mcu;
                    };
                    let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
                    if s == 0 {
                        if r == 15 {
                            k += 16;
                            continue;
                        }
                        break;
                    }
                    k += r;
                    if k > 63 {
                        stop = Stop::Error(b.st.pos);
                        break 'mcu;
                    }
                    let Some(vbits) = b.get(s) else {
                        stop = fail(&b, data);
                        break 'mcu;
                    };
                    if ci == 0 && hook.is_some() {
                        coef[k] = extend(vbits, s);
                    }
                    k += 1;
                }
                if ci == 0 && hook.is_some() {
                    lblocks.push(coef);
                }
            }
        }
        if let Some(hk) = hook.as_mut() {
            hk(b.st.mcu, &lblocks);
        }
        dc_out.push(luma_dc);
        b.st.mcu += 1;
        b.st.mcus_since_rst += 1;
        decoded += 1;
        if snap_every > 0 && b.st.pos >= last_snap + snap_every {
            snaps.push(b.st.clone());
            last_snap = b.st.pos;
        }
    }
    Decoded {
        dc: dc_out,
        snaps,
        stop,
        state: b.st,
    }
}

fn fail(b: &Bits, data: &[u8]) -> Stop {
    if let Some((m, p)) = b.marker {
        Stop::Marker(m, p)
    } else if b.st.pos >= data.len() {
        Stop::Truncated
    } else {
        Stop::Error(b.st.pos)
    }
}

/// Primeiro MCU suspeito: janela de `win` MCUs cuja diferença média de DC é anormalmente alta.
/// Devolve o índice do MCU onde a janela começa.
pub fn first_discontinuity(h: &Header, dc: &[i32]) -> Option<usize> {
    let mx = h.mcus_x as usize;
    let d: Vec<f64> = (0..dc.len())
        .map(|i| {
            if i % mx > 0 {
                (dc[i] - dc[i - 1]).abs() as f64
            } else if i >= mx {
                (dc[i] - dc[i - mx]).abs() as f64
            } else {
                0.0
            }
        })
        .collect();
    let win = 24usize;
    if d.len() < win * 2 {
        return None;
    }
    // Referência: mediana das diferenças vistas até aqui (histograma, robusta a bordas).
    let mut hist = vec![0u32; 2048];
    let mut seen = 0u32;
    let mut sum: f64 = d[..win].iter().sum();
    for x in &d[..win] {
        hist[(*x as usize).min(2047)] += 1;
        seen += 1;
    }
    for i in win..d.len() {
        sum += d[i] - d[i - win];
        if i >= 64 + win && i % 8 == 0 {
            let mut acc = 0u32;
            let mut med = 0usize;
            for (v, &c) in hist.iter().enumerate() {
                acc += c;
                if acc * 2 >= seen {
                    med = v;
                    break;
                }
            }
            if sum / win as f64 > (6.0 * med as f64).max(40.0) {
                return Some(i + 1 - win);
            }
        }
        // Só entra na referência o que ainda parece normal.
        if i < win * 3 || d[i] < 300.0 {
            hist[(d[i] as usize).min(2047)] += 1;
            seen += 1;
        }
    }
    None
}

// --------------------------------------------------------------- pixels

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

fn cos_table() -> &'static [[f32; 8]; 8] {
    static T: std::sync::OnceLock<[[f32; 8]; 8]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [[0f32; 8]; 8];
        for x in 0..8 {
            for u in 0..8 {
                let cu = if u == 0 {
                    std::f32::consts::FRAC_1_SQRT_2
                } else {
                    1.0
                };
                t[x][u] = 0.5
                    * cu
                    * (((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI) / 16.0).cos();
            }
        }
        t
    })
}

/// IDCT de um bloco (coeficientes em zigzag, quantizados) para 8x8 pixels de luminância.
pub fn idct_block(coef: &[i32; 64], qt: &[u16; 64]) -> [u8; 64] {
    let mut f = [[0f32; 8]; 8];
    for z in 0..64 {
        let n = ZIGZAG[z];
        f[n / 8][n % 8] = (coef[z] * qt[z] as i32) as f32;
    }
    let t = cos_table();
    // linhas
    let mut tmp = [[0f32; 8]; 8];
    for v in 0..8 {
        for x in 0..8 {
            let mut s = 0.0;
            for u in 0..8 {
                s += t[x][u] * f[v][u];
            }
            tmp[v][x] = s;
        }
    }
    let mut out = [0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0.0;
            for v in 0..8 {
                s += t[y][v] * tmp[v][x];
            }
            out[y * 8 + x] = (s + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Luminância de um MCU (blocos `h` x `v` da primeira componente) como imagem (8h x 8v).
pub fn mcu_luma(h: &Header, blocks: &[[i32; 64]]) -> Vec<u8> {
    let c = &h.comps[0];
    let (bw, bh) = (c.h as usize, c.v as usize);
    let w = bw * 8;
    let mut out = vec![0u8; w * bh * 8];
    for (i, blk) in blocks.iter().enumerate().take(bw * bh) {
        let px = idct_block(blk, &h.qt[c.tq]);
        let (bx, by) = (i % bw, i / bw);
        for y in 0..8 {
            out[(by * 8 + y) * w + bx * 8..(by * 8 + y) * w + bx * 8 + 8]
                .copy_from_slice(&px[y * 8..y * 8 + 8]);
        }
    }
    out
}

/// Plano de luminância do que já foi decodificado.
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
    pub mw: usize,
    pub mh: usize,
    pub have: u32,
}

impl Plane {
    pub fn new(h: &Header) -> Plane {
        let c = &h.comps[0];
        let (mw, mh) = (c.h as usize * 8, c.v as usize * 8);
        Plane {
            w: mw * h.mcus_x as usize,
            h: mh * h.mcus_y as usize,
            px: vec![0; mw * h.mcus_x as usize * mh * h.mcus_y as usize],
            mw,
            mh,
            have: 0,
        }
    }
    pub fn put(&mut self, h: &Header, mcu: u32, luma: &[u8]) {
        let (mx, my) = ((mcu % h.mcus_x) as usize, (mcu / h.mcus_x) as usize);
        for y in 0..self.mh {
            let dst = (my * self.mh + y) * self.w + mx * self.mw;
            self.px[dst..dst + self.mw].copy_from_slice(&luma[y * self.mw..(y + 1) * self.mw]);
        }
        self.have = self.have.max(mcu + 1);
    }
}

/// Decodifica e preenche o plano com as luminâncias.
pub fn decode_plane(
    h: &Header,
    data: &[u8],
    state: State,
    max_mcus: u32,
    plane: &mut Plane,
) -> Decoded {
    let mut hook = |mcu: u32, blocks: &[[i32; 64]]| {
        let l = mcu_luma(h, blocks);
        plane.put(h, mcu, &l);
    };
    decode_hook(h, data, state, max_mcus, 512, Some(&mut hook))
}
