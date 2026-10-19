//! AFF4 (Advanced Forensic Format 4, padrão v1.0): contêiner ZIP com metadados em Turtle.
//!
//! Leitura de imagens lineares e mapeadas (inclusive esparsas), com blocos comprimidos por
//! snappy, deflate, LZ4 ou sem compressão. Gravação de uma imagem linear com blocos snappy.
//! Conjuntos em vários volumes (spanned/striped) não são suportados.

use crate::error::{Error, Result};
use crate::hashing::MultiHasher;
use crate::image::ImageReport;
use crate::source::{read_exact_at, Source};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Mutex;

pub fn is_aff4(head: &[u8]) -> bool {
    // ZIP local file header + primeiro nome "container.description" ou "version.txt".
    head.len() > 50
        && head.starts_with(b"PK\x03\x04")
        && (find(&head[30..head.len().min(80)], b"container.description")
            || find(&head[30..head.len().min(80)], b"version.txt"))
}

fn find(h: &[u8], n: &[u8]) -> bool {
    h.windows(n.len()).any(|w| w == n)
}

const NS: &str = "http://aff4.org/Schema#";

// ------------------------------------------------------------------ Turtle

#[derive(Debug, Default, Clone)]
struct Resource {
    types: Vec<String>,
    props: HashMap<String, Vec<String>>,
}

impl Resource {
    fn get(&self, k: &str) -> Option<&str> {
        self.props
            .get(k)
            .and_then(|v| v.first())
            .map(|s| s.as_str())
    }
    fn num(&self, k: &str) -> Option<u64> {
        self.get(k)?.parse().ok()
    }
    fn is(&self, t: &str) -> bool {
        self.types.iter().any(|x| x == t)
    }
}

struct Turtle<'a> {
    b: &'a [u8],
    i: usize,
    prefixes: HashMap<String, String>,
}

impl Turtle<'_> {
    fn ws(&mut self) {
        loop {
            while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
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
    fn expand(&self, tok: &str) -> String {
        if let Some((p, l)) = tok.split_once(':') {
            if let Some(base) = self.prefixes.get(p) {
                return format!("{base}{l}");
            }
        }
        tok.to_string()
    }
    /// Um termo: <iri>, "literal"[^^tipo], nome:prefixado ou `a`.
    fn term(&mut self) -> Option<String> {
        self.ws();
        let c = *self.b.get(self.i)?;
        match c {
            b'<' => {
                let s = self.i + 1;
                while self.i < self.b.len() && self.b[self.i] != b'>' {
                    self.i += 1;
                }
                let v = String::from_utf8_lossy(&self.b[s..self.i]).to_string();
                self.i += 1;
                Some(v)
            }
            b'"' => {
                self.i += 1;
                let mut out = Vec::new();
                while self.i < self.b.len() && self.b[self.i] != b'"' {
                    if self.b[self.i] == b'\\' && self.i + 1 < self.b.len() {
                        self.i += 1;
                    }
                    out.push(self.b[self.i]);
                    self.i += 1;
                }
                self.i += 1;
                if self.b[self.i..].starts_with(b"^^") {
                    self.i += 2;
                    let _ = self.term();
                } else if self.b.get(self.i) == Some(&b'@') {
                    while self.i < self.b.len() && !self.b[self.i].is_ascii_whitespace() {
                        self.i += 1;
                    }
                }
                Some(String::from_utf8_lossy(&out).to_string())
            }
            _ => {
                let s = self.i;
                while self.i < self.b.len()
                    && !self.b[self.i].is_ascii_whitespace()
                    && !matches!(self.b[self.i], b';' | b',')
                {
                    self.i += 1;
                }
                // ponto final de sentença colado ao termo
                let mut e = self.i;
                if e > s && self.b[e - 1] == b'.' {
                    e -= 1;
                    self.i = e;
                }
                let tok = String::from_utf8_lossy(&self.b[s..e]).to_string();
                Some(if tok == "a" { tok } else { self.expand(&tok) })
            }
        }
    }
}

fn parse_turtle(text: &str) -> HashMap<String, Resource> {
    let mut t = Turtle {
        b: text.as_bytes(),
        i: 0,
        prefixes: HashMap::new(),
    };
    let mut out: HashMap<String, Resource> = HashMap::new();
    loop {
        t.ws();
        if t.i >= t.b.len() {
            break;
        }
        if t.b[t.i..].starts_with(b"@prefix") {
            t.i += 7;
            t.ws();
            let s = t.i;
            while t.i < t.b.len() && t.b[t.i] != b':' {
                t.i += 1;
            }
            let name = String::from_utf8_lossy(&t.b[s..t.i]).trim().to_string();
            t.i += 1;
            let iri = t.term().unwrap_or_default();
            t.ws();
            if t.b.get(t.i) == Some(&b'.') {
                t.i += 1;
            }
            t.prefixes.insert(name, iri);
            continue;
        }
        // sujeito
        let Some(subj) =
            (if t.b[t.i] == b':' && t.b.get(t.i + 1).map_or(true, |c| c.is_ascii_whitespace()) {
                t.i += 1;
                Some(t.prefixes.get("").cloned().unwrap_or_default())
            } else {
                t.term()
            })
        else {
            break;
        };
        let res = out.entry(subj).or_default();
        loop {
            let Some(pred) = t.term() else { return out };
            let mut objs = vec![];
            loop {
                let Some(o) = t.term() else { return out };
                objs.push(o);
                t.ws();
                if t.b.get(t.i) == Some(&b',') {
                    t.i += 1;
                    continue;
                }
                break;
            }
            if pred == "a" {
                res.types.extend(objs);
            } else {
                res.props.entry(pred).or_default().extend(objs);
            }
            t.ws();
            match t.b.get(t.i) {
                Some(b';') => {
                    t.i += 1;
                    t.ws();
                    // ";" final antes de "."
                    if t.b.get(t.i) == Some(&b'.') {
                        t.i += 1;
                        break;
                    }
                }
                Some(b'.') => {
                    t.i += 1;
                    break;
                }
                _ => break,
            }
        }
    }
    out
}

// ------------------------------------------------------------------ leitor

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Stored,
    Deflate,
    Zlib,
    Snappy,
    Lz4,
}

#[derive(Clone)]
struct ImageStream {
    urn: String,
    chunk: u64,
    per_bevy: u64,
    method: Method,
    size: u64,
}

#[derive(Clone, Copy)]
struct MapEntry {
    start: u64,
    len: u64,
    target_off: u64,
    target: u32,
}

#[derive(Clone)]
enum Target {
    Stream(usize),
    Zero,
    Fill(u8),
}

#[derive(Clone, Copy)]
struct Loc {
    data_start: u64,
    size: u64,
}

pub struct Aff4Source {
    file: File,
    entries: HashMap<String, Loc>,
    streams: Vec<ImageStream>,
    targets: Vec<Target>,
    map: Vec<MapEntry>,
    size: u64,
    /// Metadados legíveis: caso, examinador, notas, hashes.
    pub info: HashMap<String, String>,
    cache: Mutex<Option<(usize, u64, Vec<u8>)>>,
    index_cache: Mutex<HashMap<(usize, u64), std::sync::Arc<Vec<u8>>>>,
}

fn enc(urn: &str) -> String {
    urn.replace(':', "%3A").replace('/', "%2F")
}

fn method_of(uri: &str) -> Method {
    let u = uri.to_ascii_lowercase();
    if u.contains("snappy") {
        Method::Snappy
    } else if u.contains("lz4") {
        Method::Lz4
    } else if u.contains("rfc1951") {
        Method::Deflate
    } else if u.contains("rfc1950") {
        Method::Zlib
    } else {
        Method::Stored
    }
}

impl Aff4Source {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut zip = zip::ZipArchive::new(file.try_clone()?)
            .map_err(|e| Error::msg(format!("AFF4 inválido (ZIP): {e}")))?;
        let mut entries = HashMap::new();
        for i in 0..zip.len() {
            let f = zip.by_index_raw(i).map_err(|e| Error::msg(e.to_string()))?;
            entries.insert(
                f.name().to_string(),
                Loc {
                    data_start: f.data_start(),
                    size: f.compressed_size(),
                },
            );
        }
        let turtle = {
            let mut f = zip
                .by_name("information.turtle")
                .map_err(|_| Error::msg("AFF4 sem information.turtle"))?;
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            s
        };
        let res = parse_turtle(&turtle);
        let img_t = format!("{NS}Image");
        let disk_t = format!("{NS}DiskImage");
        let ds = format!("{NS}dataStream");
        let image = res
            .values()
            .find(|r| (r.is(&img_t) || r.is(&disk_t)) && r.get(&ds).is_some())
            .ok_or_else(|| {
                Error::msg("AFF4 sem objeto de imagem (aff4:Image com aff4:dataStream)")
            })?;
        let mut info = HashMap::new();
        if let Some(v) = image.get(&format!("{NS}diskModel")) {
            info.insert("Modelo do disco".to_string(), v.to_string());
        }
        for (k, label) in [
            ("caseNumber", "Número do caso"),
            ("examiner", "Examinador"),
            ("notes", "Notas"),
            ("evidenceNumber", "Número da evidência"),
            ("caseName", "Nome do caso"),
        ] {
            if let Some(v) = res.values().find_map(|r| r.get(&format!("{NS}{k}"))) {
                info.insert(label.to_string(), v.to_string());
            }
        }
        let data_urn = image.get(&ds).unwrap().to_string();
        // Hashes gravados no fluxo de dados (ou no mapa).
        for r in res
            .values()
            .filter(|r| r.get(&format!("{NS}hash")).is_some())
        {
            for h in r.props.get(&format!("{NS}hash")).into_iter().flatten() {
                match h.len() {
                    32 if h.chars().all(|c| c.is_ascii_hexdigit()) => {
                        info.entry("md5".to_string()).or_insert_with(|| h.clone());
                    }
                    40 if h.chars().all(|c| c.is_ascii_hexdigit()) => {
                        info.entry("sha1".to_string()).or_insert_with(|| h.clone());
                    }
                    _ => {}
                }
            }
        }
        let mut me = Self {
            file,
            entries,
            streams: vec![],
            targets: vec![],
            map: vec![],
            size: 0,
            info,
            cache: Mutex::new(None),
            index_cache: Mutex::new(HashMap::new()),
        };
        let top = res
            .get(&data_urn)
            .ok_or_else(|| Error::msg("AFF4: fluxo de dados da imagem não encontrado"))?;
        let map_t = format!("{NS}Map");
        if top.is(&map_t) {
            me.size = top.num(&format!("{NS}size")).unwrap_or(0);
            let mp = enc(&data_urn);
            let idx_txt = me.read_entry(&format!("{mp}/idx"))?;
            for line in String::from_utf8_lossy(&idx_txt).lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some(sym) = line.strip_prefix(&format!("{NS}SymbolicStream")) {
                    let b = u8::from_str_radix(sym, 16)
                        .map_err(|_| Error::msg("AFF4: fluxo simbólico inválido"))?;
                    me.targets.push(Target::Fill(b));
                } else if line == format!("{NS}Zero") {
                    me.targets.push(Target::Zero);
                } else if line.starts_with(NS) {
                    me.targets.push(Target::Zero);
                } else {
                    let s = me.load_stream(&res, line)?;
                    me.streams.push(s);
                    me.targets.push(Target::Stream(me.streams.len() - 1));
                }
            }
            let m = me.read_entry(&format!("{mp}/map"))?;
            for c in m.chunks_exact(28) {
                me.map.push(MapEntry {
                    start: u64::from_le_bytes(c[0..8].try_into().unwrap()),
                    len: u64::from_le_bytes(c[8..16].try_into().unwrap()),
                    target_off: u64::from_le_bytes(c[16..24].try_into().unwrap()),
                    target: u32::from_le_bytes(c[24..28].try_into().unwrap()),
                });
            }
            me.map.sort_by_key(|e| e.start);
        } else {
            let s = me.load_stream(&res, &data_urn)?;
            me.size = s.size;
            me.streams.push(s);
            me.targets.push(Target::Stream(0));
            me.map = vec![MapEntry {
                start: 0,
                len: me.size,
                target_off: 0,
                target: 0,
            }];
        }
        if me.size == 0 {
            me.size = image.num(&format!("{NS}size")).unwrap_or(0);
        }
        Ok(me)
    }

    fn load_stream(&self, res: &HashMap<String, Resource>, urn: &str) -> Result<ImageStream> {
        let r = res
            .get(urn)
            .ok_or_else(|| Error::msg(format!("AFF4: fluxo {urn} não descrito")))?;
        Ok(ImageStream {
            urn: urn.to_string(),
            chunk: r.num(&format!("{NS}chunkSize")).unwrap_or(32768),
            per_bevy: r.num(&format!("{NS}chunksInSegment")).unwrap_or(2048),
            method: r
                .get(&format!("{NS}compressionMethod"))
                .map(method_of)
                .unwrap_or(Method::Stored),
            size: r.num(&format!("{NS}size")).unwrap_or(0),
        })
    }

    /// MD5 do primeiro fluxo de blocos (o que o AFF4 grava em `aff4:hash`), para conferir a integridade.
    pub fn verify_stream_md5(&self) -> Result<Option<bool>> {
        let Some(want) = self.info.get("md5").cloned() else {
            return Ok(None);
        };
        let Some(st) = self.streams.first() else {
            return Ok(None);
        };
        let mut h = MultiHasher::new();
        let mut buf = vec![0u8; st.chunk as usize];
        let mut pos = 0u64;
        while pos < st.size {
            let n = (st.size - pos).min(st.chunk) as usize;
            self.read_stream(0, pos, &mut buf[..n])?;
            h.update(&buf[..n]);
            pos += n as u64;
        }
        Ok(Some(h.finalize().md5 == want))
    }

    fn read_entry(&self, name: &str) -> Result<Vec<u8>> {
        let loc = self
            .entries
            .get(name)
            .ok_or_else(|| Error::msg(format!("AFF4: entrada {name} ausente")))?;
        let mut v = vec![0u8; loc.size as usize];
        read_exact_at_file(&self.file, loc.data_start, &mut v)?;
        Ok(v)
    }

    fn chunk(&self, si: usize, chunk_no: u64) -> io::Result<Vec<u8>> {
        {
            let c = self.cache.lock().unwrap();
            if let Some((s, n, d)) = c.as_ref() {
                if *s == si && *n == chunk_no {
                    return Ok(d.clone());
                }
            }
        }
        let st = &self.streams[si];
        let (bevy, within) = (chunk_no / st.per_bevy, chunk_no % st.per_bevy);
        let base = format!("{}/{:08}", enc(&st.urn), bevy);
        let index = {
            let mut ic = self.index_cache.lock().unwrap();
            if let Some(i) = ic.get(&(si, bevy)) {
                i.clone()
            } else {
                let loc = self.entries.get(&format!("{base}.index")).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "índice do bevy ausente")
                })?;
                let mut v = vec![0u8; loc.size as usize];
                read_exact_at_file(&self.file, loc.data_start, &mut v)?;
                let a = std::sync::Arc::new(v);
                ic.insert((si, bevy), a.clone());
                a
            }
        };
        let e = index
            .get(within as usize * 12..within as usize * 12 + 12)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "bloco fora do índice"))?;
        let (off, len) = (
            u64::from_le_bytes(e[0..8].try_into().unwrap()),
            u32::from_le_bytes(e[8..12].try_into().unwrap()) as usize,
        );
        let loc = self
            .entries
            .get(&base)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "bevy ausente"))?;
        let mut raw = vec![0u8; len];
        read_exact_at_file(&self.file, loc.data_start + off, &mut raw)?;
        let want = st.chunk.min(st.size.saturating_sub(chunk_no * st.chunk)) as usize;
        let data = if len == st.chunk as usize || st.method == Method::Stored {
            raw
        } else {
            let bad = |m: String| io::Error::new(io::ErrorKind::InvalidData, m);
            match st.method {
                Method::Snappy => snap::raw::Decoder::new()
                    .decompress_vec(&raw)
                    .map_err(|e| bad(format!("snappy: {e}")))?,
                Method::Lz4 => lz4_flex::block::decompress(&raw, st.chunk as usize)
                    .map_err(|e| bad(format!("lz4: {e}")))?,
                Method::Deflate => {
                    let mut v = Vec::with_capacity(st.chunk as usize);
                    flate2::read::DeflateDecoder::new(&raw[..])
                        .read_to_end(&mut v)
                        .map_err(|e| bad(format!("deflate: {e}")))?;
                    v
                }
                Method::Zlib => {
                    let mut v = Vec::with_capacity(st.chunk as usize);
                    flate2::read::ZlibDecoder::new(&raw[..])
                        .read_to_end(&mut v)
                        .map_err(|e| bad(format!("zlib: {e}")))?;
                    v
                }
                Method::Stored => unreachable!(),
            }
        };
        let mut data = data;
        data.resize(want.max(data.len().min(st.chunk as usize)), 0);
        *self.cache.lock().unwrap() = Some((si, chunk_no, data.clone()));
        Ok(data)
    }

    fn read_stream(&self, si: usize, off: u64, buf: &mut [u8]) -> io::Result<()> {
        let cs = self.streams[si].chunk;
        let mut done = 0;
        while done < buf.len() {
            let pos = off + done as u64;
            let (cn, within) = (pos / cs, (pos % cs) as usize);
            let d = self.chunk(si, cn)?;
            if within >= d.len() {
                buf[done..].fill(0);
                return Ok(());
            }
            let n = (d.len() - within).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&d[within..within + n]);
            done += n;
        }
        Ok(())
    }
}

fn read_exact_at_file(f: &File, off: u64, buf: &mut [u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(f, buf, off)
    }
    #[cfg(windows)]
    {
        let mut done = 0;
        while done < buf.len() {
            let n =
                std::os::windows::fs::FileExt::seek_read(f, &mut buf[done..], off + done as u64)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "AFF4 truncado",
                ));
            }
            done += n;
        }
        Ok(())
    }
}

impl Source for Aff4Source {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((self.size - offset) as usize);
        let mut done = 0usize;
        while done < want {
            let pos = offset + done as u64;
            let i = self.map.partition_point(|e| e.start + e.len <= pos);
            let Some(e) = self.map.get(i).filter(|e| e.start <= pos) else {
                // vão do mapa: zeros até a próxima entrada
                let next = self.map.get(i).map(|e| e.start).unwrap_or(self.size);
                let n = ((next - pos) as usize).min(want - done);
                buf[done..done + n].fill(0);
                done += n;
                continue;
            };
            let n = ((e.start + e.len - pos) as usize).min(want - done);
            let toff = e.target_off + (pos - e.start);
            match self.targets.get(e.target as usize) {
                Some(Target::Stream(si)) => {
                    self.read_stream(*si, toff, &mut buf[done..done + n])?
                }
                Some(Target::Fill(b)) => buf[done..done + n].fill(*b),
                _ => buf[done..done + n].fill(0),
            }
            done += n;
        }
        Ok(done)
    }
}

// ------------------------------------------------------------------ gravação

#[derive(Debug, Clone)]
pub struct Aff4Options {
    pub case_name: String,
    pub examiner: String,
    pub notes: String,
    pub evidence_number: String,
    pub chunk_size: u32,
}

impl Default for Aff4Options {
    fn default() -> Self {
        Self {
            case_name: String::new(),
            examiner: String::new(),
            notes: String::new(),
            evidence_number: String::new(),
            chunk_size: 32768,
        }
    }
}

fn new_uuid() -> String {
    let mut b = [0u8; 16];
    let _ = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    if b == [0u8; 16] {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(1);
        b = (t ^ (std::process::id() as u128) << 64).to_le_bytes();
    }
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    )
}

fn iso_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Grava uma imagem AFF4 linear (um fluxo de blocos snappy) de `src`.
pub fn write_aff4(
    src: &dyn Source,
    dest: &Path,
    opts: &Aff4Options,
    cancelled: &(dyn Fn() -> bool + Sync),
    progress: &mut dyn FnMut(u64, u64),
) -> Result<ImageReport> {
    let out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| {
            Error::msg(format!(
                "não foi possível criar {}: {e} (o arquivo não pode existir)",
                dest.display()
            ))
        })?;
    crate::owner::give_to_user(dest);
    let total = src.size();
    let chunk = opts.chunk_size as u64;
    let per_bevy = 2048u64;
    let volume = format!("aff4://{}", new_uuid());
    let stream = format!("aff4://{}", new_uuid());
    let image = format!("aff4://{}", new_uuid());
    let mut zip = zip::ZipWriter::new(out);
    let stored = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    let result = (|| -> Result<ImageReport> {
        zip.start_file("container.description", stored)
            .map_err(zerr)?;
        zip.write_all(volume.as_bytes())?;
        zip.start_file("version.txt", stored).map_err(zerr)?;
        zip.write_all(format!("major=1\nminor=2\ntool=Guara {}\n", crate::VERSION).as_bytes())?;

        let mut rep = ImageReport::default();
        let mut hasher = MultiHasher::new();
        let mut buf = vec![0u8; chunk as usize];
        let mut pos = 0u64;
        let mut chunk_no = 0u64;
        let mut enc_snap = snap::raw::Encoder::new();
        while pos < total {
            let bevy = chunk_no / per_bevy;
            let name = format!("{}/{:08}", enc(&stream), bevy);
            zip.start_file(name.as_str(), stored).map_err(zerr)?;
            let mut index: Vec<u8> = vec![];
            let mut boff = 0u64;
            for _ in 0..per_bevy {
                if pos >= total {
                    break;
                }
                if cancelled() {
                    rep.cancelled = true;
                    break;
                }
                let want = (total - pos).min(chunk) as usize;
                let b = &mut buf[..want];
                if read_exact_at(src, pos, b).is_err() {
                    let mut o = 0;
                    while o < want {
                        let n = 512.min(want - o);
                        if read_exact_at(src, pos + o as u64, &mut b[o..o + n]).is_err() {
                            b[o..o + n].fill(0);
                            match rep.bad_ranges.last_mut() {
                                Some(l) if l.0 + l.1 == pos + o as u64 => l.1 += n as u64,
                                _ => rep.bad_ranges.push((pos + o as u64, n as u64)),
                            }
                        }
                        o += n;
                    }
                }
                hasher.update(b);
                // bloco curto (último) é guardado como está; comprimido só se ficar menor
                let comp = enc_snap
                    .compress_vec(b)
                    .map_err(|e| Error::msg(e.to_string()))?;
                // Só um bloco cheio pode ficar sem compressão (comprimento == tamanho do bloco).
                let data: &[u8] = if want as u64 == chunk && comp.len() >= want {
                    b
                } else {
                    &comp
                };
                zip.write_all(data)?;
                index.extend_from_slice(&boff.to_le_bytes());
                index.extend_from_slice(&(data.len() as u32).to_le_bytes());
                boff += data.len() as u64;
                pos += want as u64;
                chunk_no += 1;
                progress(pos, total);
            }
            zip.start_file(format!("{name}.index").as_str(), stored)
                .map_err(zerr)?;
            zip.write_all(&index)?;
            if rep.cancelled {
                break;
            }
        }
        let h = hasher.finalize();
        rep.bytes = pos;
        rep.md5 = h.md5.clone();
        rep.sha1 = h.sha1.clone();
        rep.sha256 = h.sha256.clone();

        let now = iso_now();
        let sectors = total.div_ceil(512);
        let ttl = format!(
            "@prefix aff4:  <http://aff4.org/Schema#> .\n@prefix xsd:   <http://www.w3.org/2001/XMLSchema#> .\n@prefix rdf:   <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\n\
<{stream}>\n        a                      aff4:ImageStream ;\n        aff4:chunkSize         \"{chunk}\"^^xsd:int ;\n        aff4:chunksInSegment   \"{per_bevy}\"^^xsd:int ;\n        aff4:compressionMethod <http://code.google.com/p/snappy/> ;\n        aff4:hash              \"{sha1}\"^^aff4:SHA1 , \"{md5}\"^^aff4:MD5 ;\n        aff4:size              \"{total}\"^^xsd:long ;\n        aff4:stored            <{volume}> ;\n        aff4:target            <{image}> ;\n        aff4:version           \"1\"^^xsd:int .\n\n\
<{image}>\n        a                aff4:ContiguousImage , aff4:DiskImage , aff4:Image ;\n        aff4:blockSize   \"512\"^^xsd:int ;\n        aff4:dataStream  <{stream}> ;\n        aff4:sectorCount \"{sectors}\"^^xsd:long ;\n        aff4:size        \"{total}\"^^xsd:long ;\n        aff4:stored      <{volume}> .\n\n\
<aff4://{notes_id}>\n        a                    aff4:CaseNotes ;\n        aff4:caseNumber      \"{case}\" ;\n        aff4:evidenceNumber  \"{ev}\" ;\n        aff4:examiner        \"{ex}\" ;\n        aff4:notes           \"{notes}\" ;\n        aff4:stored          <{volume}> ;\n        aff4:target          <{image}> ;\n        aff4:timestamp       \"{now}\"^^xsd:dateTime .\n\n\
<{volume}>\n        a                  aff4:ZipVolume ;\n        aff4:contains      <{stream}> , <{image}> ;\n        aff4:creationTime  \"{now}\"^^xsd:dateTime ;\n        aff4:interface     aff4:Volume ;\n        aff4:stored        \"{fname}\" .\n",
            sha1 = h.sha1,
            md5 = h.md5,
            notes_id = new_uuid(),
            case = esc(&opts.case_name),
            ev = esc(&opts.evidence_number),
            ex = esc(&opts.examiner),
            notes = esc(&opts.notes),
            fname = esc(&dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
        );
        zip.start_file("information.turtle", stored).map_err(zerr)?;
        zip.write_all(ttl.as_bytes())?;
        Ok(rep)
    })();
    match result {
        Ok(rep) => {
            zip.finish().map_err(zerr)?;
            crate::owner::write_owned(
                &dest.with_extension("map.json"),
                serde_json::to_vec_pretty(&rep)?,
            )?;
            Ok(rep)
        }
        Err(e) => {
            drop(zip);
            let _ = std::fs::remove_file(dest);
            Err(e)
        }
    }
}

fn zerr(e: zip::result::ZipError) -> Error {
    Error::msg(format!("AFF4/ZIP: {e}"))
}
