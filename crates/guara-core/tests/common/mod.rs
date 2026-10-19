#![allow(dead_code)]
use guara_core::scan::{self, Outcome, Progress, Sink};
use guara_core::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Default)]
pub struct Collect {
    pub found: Vec<Found>,
    pub warnings: Vec<String>,
    pub layout: Option<DiskLayout>,
}

impl Sink for Collect {
    fn found(&mut self, f: Found) {
        self.found.push(f);
    }
    fn confirm(&mut self, id: u64, c: Confidence) {
        if let Some(f) = self.found.iter_mut().find(|f| f.id == id) {
            f.confidence = f.confidence.max(c);
        }
    }
    fn replace(&mut self, f: Found) {
        match self.found.iter_mut().find(|x| x.id == f.id) {
            Some(x) => *x = f,
            None => self.found.push(f),
        }
    }
    fn progress(&mut self, _: Progress) {}
    fn warning(&mut self, w: String) {
        self.warnings.push(w);
    }
    fn layout(&mut self, l: &DiskLayout) {
        self.layout = Some(l.clone());
    }
}

/// Isola as configurações/sessões automáticas dos testes numa pasta temporária.
pub fn init() {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let d = DIR.get_or_init(|| tempfile::tempdir().unwrap());
    std::env::set_var("GUARA_CONFIG_DIR", d.path());
}

pub fn scan_path(path: &Path, opts: &ScanOptions) -> Collect {
    let src = source::open_path(path).unwrap();
    let mut sink = Collect::default();
    let out = scan::run(src.as_ref(), opts, None, &[], &|| false, &mut sink);
    assert!(matches!(out, Outcome::Completed));
    sink
}

pub fn recover_all(path: &Path, found: &[Found], dest: &Path) -> recover::RecoverReport {
    let src = source::open_path(path).unwrap();
    let refs: Vec<&Found> = found.iter().collect();
    recover::recover(src.as_ref(), &refs, dest, &|| false, &mut |_, _, _| {}).unwrap()
}

pub fn sha256_file(p: &Path) -> String {
    sha256(&std::fs::read(p).unwrap())
}

pub fn sha256(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// Gera (uma vez) as imagens de teste; `None` se as ferramentas não existirem.
pub fn fixtures() -> Option<PathBuf> {
    static ONCE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    ONCE.get_or_init(fixtures_inner).clone()
}

fn fixtures_inner() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join("target/fixtures");
    if dir.join("ext4.img").exists() && dir.join("expected-ntfs.txt").exists() {
        return Some(dir);
    }
    let ok = Command::new("bash")
        .arg(root.join("scripts/make-fixtures.sh"))
        .arg(&dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Some(dir)
    } else if std::env::var("GUARA_REQUIRE_FIXTURES").is_ok() {
        panic!("não foi possível gerar os fixtures");
    } else {
        eprintln!(
            "SKIP: ferramentas de fixture ausentes (mtools, ntfs-3g, e2fsprogs, imagemagick, zip)"
        );
        None
    }
}

pub fn expected(dir: &Path, fs: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(dir.join(format!("expected-{fs}.txt")))
        .unwrap()
        .lines()
        .map(|l| {
            let (h, p) = l.split_once(' ').unwrap();
            (h.to_string(), p.to_string())
        })
        .collect()
}

pub fn hashes_in(dir: &Path) -> HashMap<String, PathBuf> {
    let mut m = HashMap::new();
    fn walk(d: &Path, m: &mut HashMap<String, PathBuf>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, m);
            } else if !["guara-manifesto.json", "guara-diario.jsonl"]
                .contains(&p.file_name().unwrap().to_str().unwrap())
            {
                m.insert(sha256_file(&p), p);
            }
        }
    }
    walk(dir, &mut m);
    m
}
