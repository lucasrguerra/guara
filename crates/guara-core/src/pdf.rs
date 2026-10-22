//! Gerador mínimo de PDF (A4, texto e tabelas) para o relatório de recuperação.
//!
//! Usa só as fontes padrão do PDF (Helvetica, Helvetica-Bold e Courier) com codificação
//! WinAnsi, então o arquivo é autocontido e não depende de fontes instaladas. Caracteres fora
//! do Latin-1/WinAnsi viram `?`.

use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;

const W: f64 = 595.28;
const H: f64 = 841.89;
const MARGIN: f64 = 40.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Font {
    Regular,
    Bold,
    Mono,
}

impl Font {
    fn res(self) -> &'static str {
        match self {
            Font::Regular => "F1",
            Font::Bold => "F2",
            Font::Mono => "F3",
        }
    }
}

// Larguras AFM (milésimos de em) dos caracteres ASCII 32..=126.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const HELV_B: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// Letra base de um caractere acentuado do Latin-1 (a largura é a da letra sem acento).
fn base_letter(c: char) -> char {
    match c {
        'À'..='Å' => 'A',
        'Ç' => 'C',
        'È'..='Ë' => 'E',
        'Ì'..='Ï' => 'I',
        'Ñ' => 'N',
        'Ò'..='Ö' | 'Ø' => 'O',
        'Ù'..='Ü' => 'U',
        'Ý' => 'Y',
        'à'..='å' => 'a',
        'ç' => 'c',
        'è'..='ë' => 'e',
        'ì'..='ï' => 'i',
        'ñ' => 'n',
        'ò'..='ö' | 'ø' => 'o',
        'ù'..='ü' => 'u',
        'ý' | 'ÿ' => 'y',
        _ => '?',
    }
}

fn char_width(c: char, f: Font) -> f64 {
    if f == Font::Mono {
        return 0.6;
    }
    let t = if f == Font::Bold { &HELV_B } else { &HELV };
    let c = if (' '..='~').contains(&c) {
        c
    } else {
        base_letter(c)
    };
    match c {
        ' '..='~' => t[c as usize - 32] as f64 / 1000.0,
        _ => 0.556,
    }
}

pub fn text_width(s: &str, size: f64, f: Font) -> f64 {
    s.chars().map(|c| char_width(c, f)).sum::<f64>() * size
}

/// Byte WinAnsi de um caractere (ou `?`).
fn win_ansi(c: char) -> u8 {
    let n = c as u32;
    match c {
        '€' => 0x80,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '…' => 0x85,
        _ if (0x20..=0x7E).contains(&n) || (0xA0..=0xFF).contains(&n) => n as u8,
        _ => b'?',
    }
}

fn escape(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match win_ansi(c) {
            b'(' => o.push_str("\\("),
            b')' => o.push_str("\\)"),
            b'\\' => o.push_str("\\\\"),
            b if (0x20..0x7F).contains(&b) => o.push(b as char),
            b => o.push_str(&format!("\\{b:03o}")),
        }
    }
    o
}

/// Quebra `s` em linhas de no máximo `width` pontos. Sem espaço onde quebrar (caminhos,
/// hashes), corta no meio da palavra.
pub fn wrap(s: &str, size: f64, f: Font, width: f64) -> Vec<String> {
    let mut lines = vec![];
    for para in s.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0.0;
        for word in para.split(' ') {
            let ww = text_width(word, size, f);
            let sp = if cur.is_empty() {
                0.0
            } else {
                char_width(' ', f) * size
            };
            if cur_w + sp + ww <= width {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(word);
                cur_w += sp + ww;
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0.0;
            }
            if ww <= width {
                cur = word.to_string();
                cur_w = ww;
            } else {
                for c in word.chars() {
                    let cw = char_width(c, f) * size;
                    if cur_w + cw > width && !cur.is_empty() {
                        lines.push(std::mem::take(&mut cur));
                        cur_w = 0.0;
                    }
                    cur.push(c);
                    cur_w += cw;
                }
            }
        }
        lines.push(cur);
    }
    lines
}

pub struct Col {
    pub title: &'static str,
    pub width: f64,
    pub font: Font,
    pub right: bool,
}

pub struct Pdf {
    title: String,
    footer: String,
    pages: Vec<String>,
    cur: String,
    y: f64,
}

type Rgb = (f64, f64, f64);
pub const TEAL: Rgb = (0.059, 0.463, 0.431);
pub const GRAY: Rgb = (0.36, 0.43, 0.42);
pub const BLACK: Rgb = (0.06, 0.12, 0.12);

impl Pdf {
    pub fn new(title: &str, footer: &str) -> Self {
        Self {
            title: title.into(),
            footer: footer.into(),
            pages: vec![],
            cur: String::new(),
            y: H - MARGIN,
        }
    }

    pub fn content_width() -> f64 {
        W - 2.0 * MARGIN
    }

    fn new_page(&mut self) {
        if !self.cur.is_empty() || self.y != H - MARGIN {
            self.pages.push(std::mem::take(&mut self.cur));
        }
        self.y = H - MARGIN;
    }

    fn ensure(&mut self, h: f64) {
        if self.y - h < MARGIN + 14.0 {
            self.new_page();
        }
    }

    fn put(&mut self, x: f64, y: f64, s: &str, size: f64, f: Font, c: Rgb) {
        if s.is_empty() {
            return;
        }
        self.cur.push_str(&format!(
            "BT /{} {size:.1} Tf {:.3} {:.3} {:.3} rg {x:.2} {y:.2} Td ({}) Tj ET\n",
            f.res(),
            c.0,
            c.1,
            c.2,
            escape(s)
        ));
    }

    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, c: Rgb) {
        self.cur.push_str(&format!(
            "{:.3} {:.3} {:.3} rg {x:.2} {y:.2} {w:.2} {h:.2} re f\n",
            c.0, c.1, c.2
        ));
    }

    fn line(&mut self, x1: f64, x2: f64, y: f64, c: Rgb, w: f64) {
        self.cur.push_str(&format!(
            "{:.3} {:.3} {:.3} RG {w} w {x1:.2} {y:.2} m {x2:.2} {y:.2} l S\n",
            c.0, c.1, c.2
        ));
    }

    pub fn space(&mut self, h: f64) {
        self.y -= h;
    }

    pub fn title(&mut self, s: &str, sub: &str) {
        self.ensure(60.0);
        self.put(MARGIN, self.y - 20.0, s, 20.0, Font::Bold, BLACK);
        self.put(MARGIN, self.y - 36.0, sub, 9.0, Font::Regular, GRAY);
        self.line(MARGIN, W - MARGIN, self.y - 44.0, TEAL, 2.0);
        self.y -= 58.0;
    }

    pub fn heading(&mut self, s: &str) {
        self.ensure(50.0);
        self.y -= 14.0;
        self.put(MARGIN, self.y - 11.0, s, 12.0, Font::Bold, TEAL);
        self.y -= 20.0;
    }

    pub fn paragraph(&mut self, s: &str, size: f64, f: Font, c: Rgb) {
        for l in wrap(s, size, f, Self::content_width()) {
            self.ensure(size * 1.4);
            self.put(MARGIN, self.y - size, &l, size, f, c);
            self.y -= size * 1.4;
        }
        self.y -= 3.0;
    }

    /// Aviso destacado com barra lateral.
    pub fn note(&mut self, s: &str) {
        let size = 9.0;
        let lines = wrap(s, size, Font::Regular, Self::content_width() - 16.0);
        let h = lines.len() as f64 * size * 1.4 + 8.0;
        self.ensure(h + 4.0);
        let top = self.y;
        self.rect(MARGIN, top - h, Self::content_width(), h, (1.0, 0.97, 0.88));
        self.rect(MARGIN, top - h, 3.0, h, (0.88, 0.63, 0.0));
        for (i, l) in lines.iter().enumerate() {
            self.put(
                MARGIN + 10.0,
                top - 4.0 - size * 1.4 * (i as f64 + 0.8),
                l,
                size,
                Font::Regular,
                BLACK,
            );
        }
        self.y -= h + 6.0;
    }

    /// Linhas rótulo/valor; o valor pode ser monoespaçado.
    pub fn kv(&mut self, rows: &[(&str, &str, Font)]) {
        let lw = 140.0;
        let size = 9.5;
        for (k, v, f) in rows {
            let lines = wrap(v, size, *f, Self::content_width() - lw - 8.0);
            let h = lines.len() as f64 * size * 1.4 + 6.0;
            self.ensure(h);
            let top = self.y;
            self.put(MARGIN + 4.0, top - size - 3.0, k, size, Font::Regular, GRAY);
            for (i, l) in lines.iter().enumerate() {
                self.put(
                    MARGIN + lw,
                    top - size - 3.0 - i as f64 * size * 1.4,
                    l,
                    size,
                    *f,
                    BLACK,
                );
            }
            self.line(MARGIN, W - MARGIN, top - h, (0.89, 0.93, 0.93), 0.5);
            self.y -= h;
        }
        self.y -= 4.0;
    }

    /// Tabela com cabeçalho repetido a cada página e linhas que quebram texto.
    pub fn table(&mut self, cols: &[Col], rows: &[Vec<String>]) {
        let size = 8.0;
        let pad = 3.0;
        let header = |p: &mut Pdf| {
            let h = size * 1.4 + 2.0 * pad;
            p.ensure(h + 2.0 * size * 1.4);
            let top = p.y;
            p.rect(
                MARGIN,
                top - h,
                Self::content_width(),
                h,
                (0.906, 0.953, 0.949),
            );
            let mut x = MARGIN;
            for c in cols {
                let tx = if c.right {
                    x + c.width - pad - text_width(c.title, size, Font::Bold)
                } else {
                    x + pad
                };
                p.put(
                    tx,
                    top - pad - size,
                    c.title,
                    size,
                    Font::Bold,
                    (0.043, 0.31, 0.29),
                );
                x += c.width;
            }
            p.y -= h;
        };
        header(self);
        for row in rows {
            let cells: Vec<Vec<String>> = cols
                .iter()
                .zip(row)
                .map(|(c, t)| wrap(t, size, c.font, c.width - 2.0 * pad))
                .collect();
            let n = cells.iter().map(|c| c.len()).max().unwrap_or(1).max(1);
            let h = n as f64 * size * 1.35 + 2.0 * pad;
            if self.y - h < MARGIN + 14.0 {
                self.new_page();
                header(self);
            }
            let top = self.y;
            let mut x = MARGIN;
            for (c, lines) in cols.iter().zip(&cells) {
                for (i, l) in lines.iter().enumerate() {
                    let tx = if c.right {
                        x + c.width - pad - text_width(l, size, c.font)
                    } else {
                        x + pad
                    };
                    self.put(
                        tx,
                        top - pad - size - i as f64 * size * 1.35,
                        l,
                        size,
                        c.font,
                        BLACK,
                    );
                }
                x += c.width;
            }
            self.line(MARGIN, W - MARGIN, top - h, (0.89, 0.93, 0.93), 0.5);
            self.y -= h;
        }
        self.y -= 6.0;
    }

    /// Fecha o documento. `date` é a data de criação em UTC (`AAAAMMDDHHMMSS`).
    pub fn finish(mut self, date: &str) -> Vec<u8> {
        if !self.cur.is_empty() || self.pages.is_empty() {
            self.pages.push(std::mem::take(&mut self.cur));
        }
        let total = self.pages.len();
        for (i, p) in self.pages.iter_mut().enumerate() {
            let label = format!("{}  ·  página {} de {total}", self.footer, i + 1);
            let x = W - MARGIN - text_width(&label, 8.0, Font::Regular);
            p.push_str(&format!(
                "BT /F1 8 Tf {:.3} {:.3} {:.3} rg {x:.2} 24 Td ({}) Tj ET\n",
                GRAY.0,
                GRAY.1,
                GRAY.2,
                escape(&label)
            ));
        }
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets: Vec<usize> = vec![];
        let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        };
        // 1 catálogo, 2 páginas, 3-5 fontes, 6 info, depois (página, conteúdo) por página
        obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
        let kids: String = (0..total)
            .map(|i| format!("{} 0 R", 7 + 2 * i))
            .collect::<Vec<_>>()
            .join(" ");
        obj(
            &mut out,
            format!("<< /Type /Pages /Kids [{kids}] /Count {total} >>").as_bytes(),
        );
        for name in ["Helvetica", "Helvetica-Bold", "Courier"] {
            obj(
                &mut out,
                format!(
                    "<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding >>"
                )
                .as_bytes(),
            );
        }
        let title: String = format!("\u{feff}{}", self.title)
            .encode_utf16()
            .map(|u| format!("{u:04X}"))
            .collect();
        obj(
            &mut out,
            format!(
                "<< /Title <{title}> /Producer (Guara {}) /CreationDate (D:{date}Z) >>",
                env!("CARGO_PKG_VERSION")
            )
            .as_bytes(),
        );
        for (i, p) in self.pages.iter().enumerate() {
            obj(
                &mut out,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> >> /Contents {} 0 R >>",
                    8 + 2 * i
                )
                .as_bytes(),
            );
            let mut z = ZlibEncoder::new(vec![], Compression::default());
            z.write_all(p.as_bytes()).unwrap();
            let z = z.finish().unwrap();
            let mut body =
                format!("<< /Length {} /Filter /FlateDecode >>\nstream\n", z.len()).into_bytes();
            body.extend_from_slice(&z);
            body.extend_from_slice(b"\nendstream");
            obj(&mut out, &body);
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for o in &offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}
