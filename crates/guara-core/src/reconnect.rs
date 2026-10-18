//! Reencontra o disco de uma sessão quando ele foi desconectado e voltou com
//! outro nome (por exemplo `/dev/sda` virando `/dev/sdb`).

use crate::device;
use crate::source::{fingerprint, open_path, Source};
use std::path::Path;
use std::sync::Arc;

/// A origem é compatível com o que a sessão espera?
/// `size == 0` ou `fp == None` significam "não sei" e não reprovam.
pub fn matches(src: &dyn Source, size: u64, fp: Option<&str>) -> bool {
    (size == 0 || src.size() == size) && fp.map_or(true, |f| fingerprint(src).as_deref() == Some(f))
}

/// Procura entre os discos conectados o que corresponde à sessão.
/// Com impressão digital, exige igualdade; sem ela, só aceita se houver um único disco do mesmo tamanho.
pub fn find_matching_source(
    size: u64,
    fp: Option<&str>,
    exclude: &[String],
) -> Option<(String, Arc<dyn Source>)> {
    if size == 0 && fp.is_none() {
        return None;
    }
    let mut found: Vec<(String, Arc<dyn Source>)> = vec![];
    for d in device::list_devices() {
        if !d.readable || exclude.contains(&d.path) || (size != 0 && d.size != size) {
            continue;
        }
        let Ok(src) = open_path(Path::new(&d.path)) else {
            continue;
        };
        if matches(src.as_ref(), size, fp) {
            found.push((d.path, src));
        }
    }
    if fp.is_some() {
        found.into_iter().next()
    } else if found.len() == 1 {
        found.pop()
    } else {
        None
    }
}
