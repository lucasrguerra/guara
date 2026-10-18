//! Leitor de bancos SQLite feito para arquivos recuperados.
//!
//! Lê as páginas B-tree diretamente (sem a biblioteca do SQLite), então funciona
//! em bancos danificados ou truncados, para os quais a biblioteca oficial desiste.
//! Também procura **registros apagados** nas áreas livres das páginas, onde o
//! SQLite deixa o conteúdo antigo até que outro dado o sobrescreva.

use crate::source::Source;
use serde::Serialize;
use std::collections::HashSet;

const MAGIC: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", content = "v", rename_all = "lowercase")]
pub enum Cell {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    /// Tamanho e os primeiros bytes (hex) para pré-visualizar; `mime` se reconhecido.
    Blob {
        len: usize,
        head: String,
        mime: Option<&'static str>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct DbColumn {
    pub name: String,
    pub decl_type: String,
    pub pk: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbTable {
    pub name: String,
    pub rootpage: u32,
    pub sql: String,
    pub columns: Vec<DbColumn>,
    pub without_rowid: bool,
    pub virtual_table: bool,
    /// Coluna `INTEGER PRIMARY KEY` (alias do rowid), se houver.
    #[serde(skip)]
    pub rowid_alias: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbSchema {
    pub page_size: u32,
    pub pages: u32,
    pub file_pages: u64,
    pub encoding: String,
    pub tables: Vec<DbTable>,
    pub indexes: Vec<String>,
    pub views: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbRow {
    pub rowid: Option<i64>,
    pub cells: Vec<Cell>,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RowsPage {
    pub columns: Vec<String>,
    pub rows: Vec<DbRow>,
    pub total: u64,
    pub warnings: Vec<String>,
}

pub struct Db<'a> {
    src: &'a dyn Source,
    ps: usize,
    usable: usize,
    pages: u32,
    enc: u32,
}

type Res<T> = Result<T, String>;

// ------------------------------------------------------------------ utilidades

fn varint(b: &[u8]) -> Option<(i64, usize)> {
    let mut v: u64 = 0;
    for i in 0..9 {
        let c = *b.get(i)?;
        if i == 8 {
            return Some((((v << 8) | c as u64) as i64, 9));
        }
        v = (v << 7) | (c & 0x7F) as u64;
        if c & 0x80 == 0 {
            return Some((v as i64, i + 1));
        }
    }
    None
}

fn be16(b: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*b.get(o)?, *b.get(o + 1)?]) as usize)
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}

fn serial_len(t: i64) -> Option<usize> {
    Some(match t {
        0 | 8 | 9 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 | 7 => 8,
        10 | 11 => return None,
        n if n >= 12 => ((n - 12) / 2) as usize,
        _ => return None,
    })
}

fn blob_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn decode_text(b: &[u8], enc: u32) -> String {
    match enc {
        2 => String::from_utf16_lossy(
            &b.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        ),
        3 => String::from_utf16_lossy(
            &b.chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        ),
        _ => String::from_utf8_lossy(b).into_owned(),
    }
}

/// Decodifica um registro (cabeçalho de tipos + corpo).
fn decode_record(p: &[u8], enc: u32) -> Option<Vec<Cell>> {
    let (hlen, n) = varint(p)?;
    let hlen = hlen as usize;
    if hlen < n || hlen > p.len() || hlen > 4096 {
        return None;
    }
    let mut types = vec![];
    let mut o = n;
    while o < hlen {
        let (t, m) = varint(&p[o..hlen])?;
        types.push(t);
        o += m;
    }
    let mut cells = Vec::with_capacity(types.len());
    let mut o = hlen;
    for t in types {
        let l = serial_len(t)?;
        let b = p.get(o..o + l)?;
        o += l;
        cells.push(match t {
            0 => Cell::Null,
            1..=6 => {
                let mut v: i64 = if b[0] & 0x80 != 0 { -1 } else { 0 };
                for &x in b {
                    v = (v << 8) | x as i64;
                }
                Cell::Int(v)
            }
            7 => Cell::Real(f64::from_be_bytes(b.try_into().ok()?)),
            8 => Cell::Int(0),
            9 => Cell::Int(1),
            n if n % 2 == 1 => Cell::Text(decode_text(b, enc)),
            _ => Cell::Blob {
                len: b.len(),
                head: b.iter().take(24).map(|x| format!("{x:02x}")).collect(),
                mime: blob_mime(b),
            },
        });
    }
    Some(cells)
}

// ------------------------------------------------------------------ banco

impl<'a> Db<'a> {
    pub fn open(src: &'a dyn Source) -> Res<Self> {
        let mut h = [0u8; 100];
        let n = src.read_at(0, &mut h).map_err(|e| e.to_string())?;
        if n < 100 || &h[0..16] != MAGIC {
            return Err("Não é um banco SQLite (cabeçalho ausente). Se o arquivo é um banco, ele pode estar criptografado (SQLCipher) ou ter o início sobrescrito.".into());
        }
        let ps = match u16::from_be_bytes([h[16], h[17]]) {
            1 => 65536usize,
            v if (512..=32768).contains(&v) && v.is_power_of_two() => v as usize,
            _ => return Err("Tamanho de página inválido no cabeçalho do banco.".into()),
        };
        let reserved = h[20] as usize;
        let mut pages = u32::from_be_bytes([h[28], h[29], h[30], h[31]]);
        let file_pages = (src.size() / ps as u64) as u32;
        if pages == 0 || pages > file_pages.max(1) * 4 + 1 {
            pages = file_pages;
        }
        let enc = u32::from_be_bytes([h[56], h[57], h[58], h[59]]);
        Ok(Self {
            src,
            ps,
            usable: ps - reserved,
            pages: pages.max(1),
            enc: if (1..=3).contains(&enc) { enc } else { 1 },
        })
    }

    fn page(&self, n: u32) -> Res<Vec<u8>> {
        if n == 0 {
            return Err("página 0 inválida".into());
        }
        let off = (n as u64 - 1) * self.ps as u64;
        if off + self.ps as u64 > self.src.size() {
            return Err(format!("página {n} fora do arquivo (banco truncado)"));
        }
        let mut b = vec![0u8; self.ps];
        let mut got = 0;
        while got < b.len() {
            match self.src.read_at(off + got as u64, &mut b[got..]) {
                Ok(0) => return Err(format!("página {n} incompleta")),
                Ok(k) => got += k,
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(b)
    }

    fn enc_name(&self) -> &'static str {
        match self.enc {
            2 => "UTF-16 LE",
            3 => "UTF-16 BE",
            _ => "UTF-8",
        }
    }

    /// Lê o payload completo de uma célula (segue as páginas de overflow).
    fn payload(
        &self,
        page: &[u8],
        cell_off: usize,
        p: usize,
        index: bool,
        hdr: usize,
    ) -> Option<Vec<u8>> {
        let u = self.usable;
        let x = if index {
            ((u - 12) * 64 / 255).saturating_sub(23)
        } else {
            u - 35
        };
        let m = ((u - 12) * 32 / 255).saturating_sub(23);
        let local = if p <= x {
            p
        } else {
            let k = m + (p - m) % (u - 4);
            if k <= x {
                k
            } else {
                m
            }
        };
        let start = cell_off + hdr;
        let mut out = page.get(start..start + local)?.to_vec();
        if p > local {
            let mut next = be32(page, start + local)?;
            let mut guard = 0;
            while out.len() < p && next != 0 && guard < 100_000 {
                guard += 1;
                let ov = self.page(next).ok()?;
                next = be32(&ov, 0)?;
                let take = (p - out.len()).min(u - 4);
                out.extend_from_slice(ov.get(4..4 + take)?);
            }
            if out.len() < p {
                return None;
            }
        }
        Some(out)
    }

    /// Percorre as folhas de uma árvore de tabela chamando `f(rowid, payload)`.
    /// `f` devolve false para parar. Devolve os avisos de corrupção encontrados.
    fn walk_table(&self, root: u32, f: &mut dyn FnMut(i64, &[u8]) -> bool) -> Vec<String> {
        let mut warnings = vec![];
        let mut stack = vec![(root, 0u32)];
        let mut seen = HashSet::new();
        while let Some((pn, depth)) = stack.pop() {
            if depth > 40 || !seen.insert(pn) {
                continue;
            }
            let pg = match self.page(pn) {
                Ok(p) => p,
                Err(e) => {
                    if warnings.len() < 5 {
                        warnings.push(e);
                    }
                    continue;
                }
            };
            let hb = if pn == 1 { 100 } else { 0 };
            let Some(&ty) = pg.get(hb) else { continue };
            let ncell = be16(&pg, hb + 3).unwrap_or(0);
            match ty {
                0x05 => {
                    let mut kids = vec![];
                    for i in 0..ncell {
                        let Some(co) = be16(&pg, hb + 12 + i * 2) else {
                            break;
                        };
                        if let Some(c) = be32(&pg, co) {
                            kids.push(c);
                        }
                    }
                    if let Some(r) = be32(&pg, hb + 8) {
                        kids.push(r);
                    }
                    for k in kids.into_iter().rev() {
                        stack.push((k, depth + 1));
                    }
                }
                0x0D => {
                    for i in 0..ncell {
                        let Some(co) = be16(&pg, hb + 8 + i * 2) else {
                            break;
                        };
                        let Some(tail) = pg.get(co..) else { continue };
                        let Some((plen, a)) = varint(tail) else {
                            continue;
                        };
                        let Some((rowid, b)) = varint(&tail[a..]) else {
                            continue;
                        };
                        if plen < 0 || plen as usize > 1 << 28 {
                            continue;
                        }
                        if let Some(pl) = self.payload(&pg, co, plen as usize, false, a + b) {
                            if !f(rowid, &pl) {
                                return warnings;
                            }
                        } else if warnings.len() < 5 {
                            warnings.push(format!("célula ilegível na página {pn}"));
                        }
                    }
                }
                _ => {
                    if warnings.len() < 5 {
                        warnings.push(format!("página {pn} de tipo inesperado (0x{ty:02x})"));
                    }
                }
            }
        }
        warnings
    }

    fn leaf_pages(&self, root: u32) -> Vec<u32> {
        let mut leaves = vec![];
        let mut stack = vec![(root, 0u32)];
        let mut seen = HashSet::new();
        while let Some((pn, depth)) = stack.pop() {
            if depth > 40 || !seen.insert(pn) {
                continue;
            }
            let Ok(pg) = self.page(pn) else { continue };
            let hb = if pn == 1 { 100 } else { 0 };
            match pg.get(hb) {
                Some(0x05) => {
                    let n = be16(&pg, hb + 3).unwrap_or(0);
                    for i in 0..n {
                        if let Some(c) = be16(&pg, hb + 12 + i * 2).and_then(|co| be32(&pg, co)) {
                            stack.push((c, depth + 1));
                        }
                    }
                    if let Some(r) = be32(&pg, hb + 8) {
                        stack.push((r, depth + 1));
                    }
                }
                Some(0x0D) => leaves.push(pn),
                _ => {}
            }
        }
        leaves
    }

    // ---------------------------------------------------------- esquema

    pub fn schema(&self) -> DbSchema {
        let mut s = DbSchema {
            page_size: self.ps as u32,
            pages: self.pages,
            file_pages: self.src.size() / self.ps as u64,
            encoding: self.enc_name().into(),
            tables: vec![],
            indexes: vec![],
            views: vec![],
            warnings: vec![],
        };
        if s.file_pages < self.pages as u64 {
            s.warnings.push(format!(
                "Arquivo truncado: o cabeçalho indica {} páginas, mas só há {}. Parte dos dados pode estar faltando.",
                self.pages, s.file_pages
            ));
        }
        let mut rows: Vec<Vec<Cell>> = vec![];
        let w = self.walk_table(1, &mut |_, p| {
            if let Some(c) = decode_record(p, self.enc) {
                rows.push(c);
            }
            true
        });
        s.warnings.extend(w);
        let text = |c: &Cell| {
            if let Cell::Text(t) = c {
                t.clone()
            } else {
                String::new()
            }
        };
        for r in rows {
            if r.len() < 5 {
                continue;
            }
            let (ty, name, rootpage, sql) = (
                text(&r[0]),
                text(&r[1]),
                if let Cell::Int(i) = r[3] { i as u32 } else { 0 },
                text(&r[4]),
            );
            match ty.as_str() {
                "table" => {
                    let (columns, rowid_alias, without_rowid) = parse_create(&sql);
                    s.tables.push(DbTable {
                        virtual_table: sql.to_uppercase().starts_with("CREATE VIRTUAL"),
                        name,
                        rootpage,
                        sql,
                        columns,
                        without_rowid,
                        rowid_alias,
                    });
                }
                "index" => s.indexes.push(name),
                "view" => s.views.push(name),
                _ => {}
            }
        }
        if s.tables.is_empty() {
            s.warnings.push(
                "Nenhuma tabela encontrada no esquema. A primeira página pode estar danificada."
                    .into(),
            );
        }
        s
    }

    fn table(&self, name: &str) -> Res<DbTable> {
        self.schema()
            .tables
            .into_iter()
            .find(|t| t.name == name)
            .ok_or_else(|| format!("tabela {name} não encontrada"))
    }

    /// Conta as linhas da tabela (sem decodificar o conteúdo).
    pub fn count_rows(&self, t: &DbTable) -> u64 {
        if t.rootpage == 0 || t.virtual_table || t.without_rowid {
            return 0;
        }
        let mut n = 0u64;
        for leaf in self.leaf_pages(t.rootpage) {
            if let Ok(pg) = self.page(leaf) {
                n += be16(&pg, if leaf == 1 { 103 } else { 3 }).unwrap_or(0) as u64;
            }
        }
        n
    }

    // ---------------------------------------------------------- linhas

    pub fn rows(&self, table: &str, offset: usize, limit: usize) -> Res<RowsPage> {
        let t = self.table(table)?;
        let mut page = RowsPage {
            columns: t.columns.iter().map(|c| c.name.clone()).collect(),
            ..Default::default()
        };
        if t.virtual_table || t.rootpage == 0 {
            page.warnings
                .push("Tabela virtual: o conteúdo não fica armazenado como linhas comuns.".into());
            return Ok(page);
        }
        if t.without_rowid {
            page.warnings.push(
                "Tabela WITHOUT ROWID: as linhas ficam numa árvore de índice, ainda não exibida."
                    .into(),
            );
            return Ok(page);
        }
        let mut seen = 0usize;
        let ncols = t.columns.len();
        page.warnings = self.walk_table(t.rootpage, &mut |rowid, p| {
            seen += 1;
            if seen <= offset {
                return true;
            }
            if page.rows.len() >= limit {
                return false;
            }
            if let Some(mut cells) = decode_record(p, self.enc) {
                if let Some(a) = t.rowid_alias {
                    if a < cells.len() && matches!(cells[a], Cell::Null) {
                        cells[a] = Cell::Int(rowid);
                    }
                }
                cells.resize(cells.len().max(ncols), Cell::Null);
                apply_affinity(&mut cells, &t.columns);
                page.rows.push(DbRow {
                    rowid: Some(rowid),
                    cells,
                    deleted: false,
                });
            }
            true
        });
        page.total = self.count_rows(&t);
        Ok(page)
    }

    // ---------------------------------------------------------- registros apagados

    /// Procura registros apagados nas áreas livres das páginas da tabela.
    /// São "prováveis": o SQLite não marca o que foi apagado, então validamos pela forma
    /// do registro (mesmo número de colunas, tipos e textos coerentes).
    pub fn deleted_rows(&self, table: &str) -> Res<RowsPage> {
        let t = self.table(table)?;
        let n = t.columns.len();
        let mut out = RowsPage {
            columns: t.columns.iter().map(|c| c.name.clone()).collect(),
            ..Default::default()
        };
        if n < 2 || t.rootpage == 0 || t.virtual_table || t.without_rowid {
            return Ok(out);
        }
        for leaf in self.leaf_pages(t.rootpage) {
            let Ok(pg) = self.page(leaf) else { continue };
            let hb = if leaf == 1 { 100 } else { 0 };
            let ncell = be16(&pg, hb + 3).unwrap_or(0);
            let content = match be16(&pg, hb + 5) {
                Some(0) => 65536,
                Some(c) => c,
                None => continue,
            };
            let ptr_end = hb + 8 + ncell * 2;
            let mut regions: Vec<(usize, usize)> = vec![];
            if content > ptr_end {
                regions.push((ptr_end, content));
            }
            // cadeia de blocos livres
            let mut fb = be16(&pg, hb + 1).unwrap_or(0);
            let mut guard = 0;
            while fb != 0 && guard < 2000 {
                guard += 1;
                let (Some(next), Some(size)) = (be16(&pg, fb), be16(&pg, fb + 2)) else {
                    break;
                };
                if fb + size <= self.usable && size >= 4 {
                    regions.push((fb, fb + size));
                }
                fb = next;
            }
            for (a, b) in regions {
                let end = b.min(pg.len()).min(self.usable);
                if a >= end {
                    continue;
                }
                let mut p = a;
                while p + 3 < end {
                    if let Some((mut cells, used)) = self.try_deleted(&pg[p..end], &t) {
                        apply_affinity(&mut cells, &t.columns);
                        out.rows.push(DbRow {
                            rowid: None,
                            cells,
                            deleted: true,
                        });
                        p += used.max(1);
                    } else {
                        p += 1;
                    }
                }
            }
            if out.rows.len() >= 5000 {
                break;
            }
        }
        out.total = out.rows.len() as u64;
        Ok(out)
    }

    /// Tenta ler um registro apagado no início de `b`, em três formas:
    /// (a) cabeçalho íntegro; (b) tamanho do cabeçalho perdido (o SQLite sobrescreve os 4
    /// primeiros bytes do bloco livre); (c) como (b), mas sem o primeiro tipo (coluna que é
    /// alias do rowid, sempre NULL).
    fn try_deleted(&self, b: &[u8], t: &DbTable) -> Option<(Vec<Cell>, usize)> {
        let n = t.columns.len();
        // (a)
        if let Some(r) = self.record_with_types(b, 1, n, None, t) {
            return Some(r);
        }
        // (b)
        if let Some(r) = self.record_with_types(b, 0, n, None, t) {
            return Some(r);
        }
        // (c)
        if t.rowid_alias == Some(0) {
            return self.record_with_types(b, 0, n - 1, Some(0), t);
        }
        None
    }

    /// Lê `ntypes` tipos seriais a partir de `b[start..]` (com um tipo inicial opcional já
    /// conhecido) e o corpo logo depois do cabeçalho.
    fn record_with_types(
        &self,
        b: &[u8],
        start: usize,
        ntypes: usize,
        first: Option<i64>,
        t: &DbTable,
    ) -> Option<(Vec<Cell>, usize)> {
        let n = t.columns.len();
        let mut types: Vec<i64> = first.into_iter().collect();
        let mut o = start;
        let mut hdr_bytes = 1usize; // o próprio byte do tamanho do cabeçalho
        for _ in 0..ntypes {
            let (ty, m) = varint(b.get(o..)?)?;
            types.push(ty);
            o += m;
            hdr_bytes += m;
        }
        if first.is_some() {
            hdr_bytes += 1;
        }
        if start == 1 {
            // forma (a): o primeiro byte declara o tamanho do cabeçalho e precisa bater
            if *b.first()? as usize != o {
                return None;
            }
        }
        if types.len() != n {
            return None;
        }
        for (ty, col) in types.iter().zip(&t.columns) {
            if !type_ok(&col.decl_type, *ty) {
                return None;
            }
        }
        let body: usize = types
            .iter()
            .map(|&x| serial_len(x))
            .sum::<Option<usize>>()?;
        if body == 0 || o + body > b.len() {
            return None;
        }
        // monta um registro canônico para reaproveitar o decodificador
        let mut rec = vec![];
        let mut hdr = vec![];
        for &ty in &types {
            put_varint(&mut hdr, ty);
        }
        put_varint(&mut rec, (hdr.len() + 1) as i64);
        rec.extend(hdr);
        rec.extend_from_slice(&b[o..o + body]);
        let cells = decode_record(&rec, self.enc)?;
        let mut texts = 0;
        for c in &cells {
            if let Cell::Text(s) = c {
                if s.contains('\u{FFFD}')
                    || s.chars()
                        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
                {
                    return None;
                }
                texts += 1;
            }
        }
        let non_null = cells.iter().filter(|c| !matches!(c, Cell::Null)).count();
        if non_null < 2 || (texts == 0 && n < 3) {
            return None;
        }
        let _ = hdr_bytes;
        Some((cells, o + body))
    }
}

/// O tipo serial é compatível com o tipo declarado da coluna (afinidade do SQLite)?
fn type_ok(decl: &str, ty: i64) -> bool {
    if ty == 10 || ty == 11 || ty < 0 {
        return false;
    }
    if ty == 0 {
        return true;
    }
    let d = decl.to_uppercase();
    let is_text = ty >= 13 && ty % 2 == 1;
    let is_blob = ty >= 12 && ty % 2 == 0;
    if d.contains("INT") {
        ty <= 9 || is_text
    } else if d.contains("CHAR") || d.contains("CLOB") || d.contains("TEXT") {
        is_text || is_blob || ty <= 9
    } else if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") {
        ty <= 9
    } else {
        true
    }
}

fn put_varint(out: &mut Vec<u8>, v: i64) {
    let v = v as u64;
    if v <= 0x7F {
        out.push(v as u8);
        return;
    }
    let mut tmp = vec![];
    let mut x = v;
    while x > 0 {
        tmp.push((x & 0x7F) as u8);
        x >>= 7;
    }
    tmp.reverse();
    let last = tmp.len() - 1;
    for (i, b) in tmp.iter().enumerate() {
        out.push(if i == last { *b } else { *b | 0x80 });
    }
}

/// O SQLite guarda REAIS inteiros como inteiros; devolve-os como REAL quando a coluna é REAL.
fn apply_affinity(cells: &mut [Cell], cols: &[DbColumn]) {
    for (c, col) in cells.iter_mut().zip(cols) {
        let d = col.decl_type.to_uppercase();
        if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") {
            if let Cell::Int(i) = c {
                *c = Cell::Real(*i as f64);
            }
        }
    }
}

// ------------------------------------------------------------------ CREATE TABLE

fn unquote(s: &str) -> String {
    let t = s.trim();
    let b = t.as_bytes();
    if b.len() >= 2
        && matches!(
            (b[0], b[b.len() - 1]),
            (b'"', b'"') | (b'`', b'`') | (b'[', b']') | (b'\'', b'\'')
        )
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// Extrai colunas, alias de rowid e WITHOUT ROWID de um `CREATE TABLE`.
fn parse_create(sql: &str) -> (Vec<DbColumn>, Option<usize>, bool) {
    let upper = sql.to_uppercase();
    let without_rowid = upper.trim_end().ends_with("WITHOUT ROWID");
    let Some(open) = sql.find('(') else {
        return (vec![], None, without_rowid);
    };
    // fecha o parêntese correspondente
    let mut depth = 0;
    let mut close = sql.len();
    let mut in_q: Option<char> = None;
    for (i, ch) in sql.char_indices().skip_while(|(i, _)| *i < open) {
        match (in_q, ch) {
            (Some(q), c) if c == q => in_q = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => in_q = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => {
                depth -= 1;
                if depth == 0 {
                    close = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &sql[open + 1..close.min(sql.len())];
    // separa por vírgulas de nível 0
    let mut parts = vec![];
    let (mut d, mut start, mut q) = (0, 0, None::<char>);
    for (i, ch) in body.char_indices() {
        match (q, ch) {
            (Some(c), x) if c == x => q = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => q = Some(ch),
            (None, '(') => d += 1,
            (None, ')') => d -= 1,
            (None, ',') if d == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    let mut cols = vec![];
    let mut alias = None;
    for part in parts {
        let p = part.trim();
        let up = p.to_uppercase();
        if ["PRIMARY", "UNIQUE", "CHECK", "FOREIGN", "CONSTRAINT"]
            .iter()
            .any(|k| up.starts_with(k))
        {
            continue;
        }
        let (name, rest) = if p.starts_with(['"', '`', '[']) {
            let close = match p.as_bytes()[0] {
                b'[' => ']',
                b'`' => '`',
                _ => '"',
            };
            let end = p[1..].find(close).map(|e| e + 2).unwrap_or(p.len());
            (unquote(&p[..end]), p[end..].trim())
        } else {
            match p.split_once(char::is_whitespace) {
                Some((a, b)) => (a.to_string(), b.trim()),
                None => (p.to_string(), ""),
            }
        };
        if name.is_empty() {
            continue;
        }
        let ru = rest.to_uppercase();
        let decl: String = rest
            .split_whitespace()
            .take_while(|w| {
                ![
                    "PRIMARY",
                    "NOT",
                    "NULL",
                    "UNIQUE",
                    "DEFAULT",
                    "CHECK",
                    "REFERENCES",
                    "COLLATE",
                    "CONSTRAINT",
                    "GENERATED",
                    "AS",
                ]
                .contains(&w.to_uppercase().as_str())
            })
            .collect::<Vec<_>>()
            .join(" ");
        let pk = ru.contains("PRIMARY KEY");
        if pk && decl.eq_ignore_ascii_case("INTEGER") && !ru.contains("DESC") {
            alias = Some(cols.len());
        }
        cols.push(DbColumn {
            name,
            decl_type: decl,
            pk,
        });
    }
    (cols, alias, without_rowid)
}

/// O arquivo começa com o cabeçalho de um banco SQLite?
pub fn is_sqlite(head: &[u8]) -> bool {
    head.len() >= 16 && &head[0..16] == MAGIC
}
