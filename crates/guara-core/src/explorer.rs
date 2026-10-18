//! Navegação estilo gerenciador de arquivos: filtros, ordenação, facetas
//! (tipos, datas, pastas, câmeras) e cache de miniaturas.

use crate::types::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    #[serde(default)]
    pub category: Option<Category>,
    #[serde(default)]
    pub min_confidence: Option<Confidence>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Pasta (prefixo), "@carved" (achados por assinatura) ou "@raiz".
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub month: Option<u32>,
    /// Só arquivos sem data conhecida.
    #[serde(default)]
    pub undated: Option<bool>,
    #[serde(default)]
    pub camera: Option<String>,
    #[serde(default)]
    pub origin: Option<Origin>,
    #[serde(default)]
    pub min_size: Option<u64>,
    #[serde(default)]
    pub max_size: Option<u64>,
    /// Megapixels mínimos.
    #[serde(default)]
    pub min_mp: Option<f64>,
    #[serde(default)]
    pub has_gps: Option<bool>,
    /// Mostra só a primeira ocorrência de cada conteúdo repetido.
    #[serde(default)]
    pub dedupe: Option<bool>,
}

fn default_limit() -> usize {
    200
}

impl Default for Query {
    fn default() -> Self {
        serde_json::from_str("{}").unwrap()
    }
}

impl Query {
    pub fn needle(&self) -> Option<String> {
        self.query
            .as_deref()
            .map(|x| x.to_lowercase())
            .filter(|x| !x.is_empty())
    }

    /// Itens que atendem ao filtro (na ordem de achado), já sem duplicadas se pedido.
    pub fn select<'a>(&self, found: &'a [Found]) -> Vec<&'a Found> {
        let needle = self.needle();
        let mut v: Vec<&Found> = found.iter().filter(|f| self.matches(f, &needle)).collect();
        if self.dedupe == Some(true) {
            let mut seen = std::collections::HashSet::new();
            v.retain(|f| f.sig.map(|g| seen.insert((g, f.size))).unwrap_or(true));
        }
        v
    }

    pub fn matches(&self, f: &Found, needle: &Option<String>) -> bool {
        if self.category.is_some_and(|c| f.category != c) {
            return false;
        }
        if self.min_confidence.is_some_and(|c| f.confidence < c) {
            return false;
        }
        if self.origin.is_some_and(|o| f.origin != o) {
            return false;
        }
        if self.min_size.is_some_and(|m| f.size < m) || self.max_size.is_some_and(|m| f.size > m) {
            return false;
        }
        if let Some(n) = needle {
            let hay =
                format!("{} {}", f.full_path(), f.camera.as_deref().unwrap_or("")).to_lowercase();
            if !hay.contains(n.as_str()) {
                return false;
            }
        }
        if let Some(folder) = &self.folder {
            let ok = match folder.as_str() {
                "@carved" => f.origin == Origin::Carved,
                "@raiz" => {
                    f.origin != Origin::Carved
                        && f.dir.as_deref().map(str::is_empty).unwrap_or(true)
                }
                p => f
                    .dir
                    .as_deref()
                    .is_some_and(|d| d == p || d.starts_with(&format!("{p}/"))),
            };
            if !ok {
                return false;
            }
        }
        if self.year.is_some() || self.month.is_some() {
            let Some(t) = f.when() else { return false };
            let (y, m, _) = crate::meta::ymd(t);
            if self.year.is_some_and(|yy| yy != y) || self.month.is_some_and(|mm| mm != m) {
                return false;
            }
        }
        if self.undated == Some(true) && f.when().is_some() {
            return false;
        }
        if let Some(c) = &self.camera {
            if f.camera.as_deref() != Some(c.as_str()) {
                return false;
            }
        }
        if let Some(mp) = self.min_mp {
            match f.px {
                Some((w, h)) if (w as f64 * h as f64) / 1e6 >= mp => {}
                _ => return false,
            }
        }
        if self.has_gps == Some(true) && !f.gps {
            return false;
        }
        true
    }
}

pub fn sort_items(items: &mut [&Found], sort: Option<&str>) {
    match sort {
        Some("name") => items.sort_by_key(|f| f.name.to_lowercase()),
        Some("size") => items.sort_by_key(|f| std::cmp::Reverse(f.size)),
        Some("date") => items.sort_by(|a, b| b.when().cmp(&a.when()).then(a.id.cmp(&b.id))),
        Some("date_asc") => items.sort_by(|a, b| match (a.when(), b.when()) {
            (Some(x), Some(y)) => x.cmp(&y).then(a.id.cmp(&b.id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => a.id.cmp(&b.id),
        }),
        Some("confidence") => {
            items.sort_by(|a, b| b.confidence.cmp(&a.confidence).then(b.size.cmp(&a.size)))
        }
        _ => {}
    }
}

pub fn item_json(f: &Found) -> Value {
    json!({
        "id": f.id, "name": f.name, "dir": f.dir, "ext": f.ext, "category": f.category,
        "size": f.size, "origin": f.origin, "confidence": f.confidence,
        "modified": f.modified, "created": f.created, "accessed": f.accessed,
        "taken": f.taken, "when": f.when(), "camera": f.camera, "px": f.px, "gps": f.gps,
        "note": f.note,
    })
}

/// Contagens para a navegação lateral.
pub fn facets(found: &[Found]) -> Value {
    let mut cats: HashMap<&'static str, usize> = HashMap::new();
    let mut years: BTreeMap<i64, (usize, BTreeMap<u32, usize>)> = BTreeMap::new();
    let mut cams: BTreeMap<String, usize> = BTreeMap::new();
    let mut folders: BTreeMap<String, usize> = BTreeMap::new();
    let (mut undated, mut carved, mut root, mut gps) = (0usize, 0usize, 0usize, 0usize);
    let mut conf = [0usize; 3];
    let mut seen_sig = std::collections::HashSet::new();
    let mut dups = 0usize;
    for f in found {
        if let Some(g) = f.sig {
            if !seen_sig.insert((g, f.size)) {
                dups += 1;
            }
        }
        *cats.entry(cat_key(f.category)).or_default() += 1;
        conf[f.confidence as usize] += 1;
        match f.when() {
            Some(t) => {
                let (y, m, _) = crate::meta::ymd(t);
                let e = years.entry(y).or_default();
                e.0 += 1;
                *e.1.entry(m).or_default() += 1;
            }
            None => undated += 1,
        }
        if let Some(c) = &f.camera {
            *cams.entry(c.clone()).or_default() += 1;
        }
        if f.gps {
            gps += 1;
        }
        if f.origin == Origin::Carved {
            carved += 1;
        } else if let Some(d) = f.dir.as_deref().filter(|d| !d.is_empty()) {
            let mut acc = String::new();
            for part in d.split('/') {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(part);
                *folders.entry(acc.clone()).or_default() += 1;
            }
        } else {
            root += 1;
        }
    }
    let cams_v: Vec<Value> = {
        let mut v: Vec<_> = cams.into_iter().collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        v.into_iter()
            .map(|(n, c)| json!({ "name": n, "count": c }))
            .collect()
    };
    json!({
        "total": found.len(),
        "categories": cats,
        "confidence": { "low": conf[0], "medium": conf[1], "high": conf[2] },
        "years": years.iter().rev().map(|(y, (n, ms))| json!({
            "year": y, "count": n,
            "months": ms.iter().rev().map(|(m, c)| json!({"month": m, "count": c})).collect::<Vec<_>>()
        })).collect::<Vec<_>>(),
        "undated": undated,
        "cameras": cams_v,
        "folders": folders.iter().map(|(p, c)| json!({
            "path": p, "name": p.rsplit('/').next().unwrap_or(p), "depth": p.matches('/').count(), "count": c
        })).collect::<Vec<_>>(),
        "carved": carved,
        "root": root,
        "gps": gps,
        "duplicates": dups,
    })
}

pub fn cat_key(c: Category) -> &'static str {
    match c {
        Category::Image => "image",
        Category::Document => "document",
        Category::Video => "video",
        Category::Audio => "audio",
        Category::Archive => "archive",
        Category::Database => "database",
        Category::Other => "other",
    }
}

// ------------------------------------------------------------------ cache de miniaturas

type Key = (String, u64, u32);

pub struct ThumbCache {
    map: HashMap<Key, Arc<Vec<u8>>>,
    order: VecDeque<Key>,
    bytes: usize,
}

impl ThumbCache {
    const CAP: usize = 128 * 1024 * 1024;
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        }
    }
    pub fn get(&self, k: &Key) -> Option<Arc<Vec<u8>>> {
        self.map.get(k).cloned()
    }
    pub fn put(&mut self, k: Key, v: Arc<Vec<u8>>) {
        self.bytes += v.len();
        if self.map.insert(k.clone(), v).is_none() {
            self.order.push_back(k);
        }
        while self.bytes > Self::CAP {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(v) = self.map.remove(&old) {
                self.bytes -= v.len();
            }
        }
    }
}

impl Default for ThumbCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Limita quantas miniaturas são decodificadas ao mesmo tempo (memória/CPU).
pub struct Gate {
    n: Mutex<usize>,
    cv: Condvar,
    max: usize,
}

pub struct Permit<'a>(&'a Gate);

impl Gate {
    pub fn new(max: usize) -> Self {
        Self {
            n: Mutex::new(0),
            cv: Condvar::new(),
            max: max.max(1),
        }
    }
    pub fn acquire(&self) -> Permit<'_> {
        let mut n = self.n.lock().unwrap();
        while *n >= self.max {
            n = self.cv.wait(n).unwrap();
        }
        *n += 1;
        Permit(self)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.n.lock().unwrap() -= 1;
        self.0.cv.notify_one();
    }
}

/// Assinatura rápida: tamanho + FNV-1a do início (32 KiB) e do fim (32 KiB).
pub fn quick_sig(src: &dyn crate::source::Source, f: &Found) -> Option<u64> {
    use crate::recover::read_found_at;
    if f.size == 0 {
        return None;
    }
    let mut h: u64 = 0xcbf29ce484222325 ^ f.size;
    let mut feed = |off: u64| {
        let mut buf = vec![0u8; 32 * 1024];
        let mut got = 0;
        while got < buf.len() {
            match read_found_at(src, f, off + got as u64, &mut buf[got..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        for &b in &buf[..got] {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        got
    };
    let a = feed(0);
    if a == 0 {
        return None;
    }
    if f.size > 64 * 1024 {
        feed(f.size - 32 * 1024);
    }
    Some(h)
}
