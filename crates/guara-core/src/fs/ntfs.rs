//! NTFS: percorre a MFT atrás de registros apagados, resolve caminhos e
//! reconstrói os data runs. Coleta também os extents vivos.

use super::{
    make_found, merge_extents, overlaps, sanitize_name, utf16le, Ctx, NewFound, VolumeScan,
};
use crate::source::{read_vec, Source};
use crate::types::{Confidence, DataLoc, Extent, Origin};
use std::collections::HashMap;

struct Run {
    /// Cluster lógico inicial no arquivo (VCN).
    vcn: u64,
    len: u64,
    /// None = esparso.
    lcn: Option<u64>,
}

struct Rec {
    in_use: bool,
    is_dir: bool,
    parent: u64,
    name: Option<(String, u8)>,
    mtime: Option<i64>,
    ctime: Option<i64>,
    atime: Option<i64>,
    data: Option<Data>,
    has_attr_list: bool,
}

enum Data {
    Resident(Vec<u8>),
    NonResident {
        runs: Vec<Run>,
        real: u64,
        unsupported: bool,
    },
}

pub fn scan(ctx: &mut Ctx, base: u64) -> VolumeScan {
    let mut res = VolumeScan::default();
    let Some(boot) = read_vec(ctx.src, base, 512).ok().filter(|b| b.len() == 512) else {
        res.warnings.push("NTFS: setor de boot ilegível.".into());
        return res;
    };
    let bps = u16::from_le_bytes([boot[11], boot[12]]) as u64;
    let spc = boot[13] as u64;
    let cl = bps * spc;
    let total_secs = u64::from_le_bytes(boot[40..48].try_into().unwrap());
    let mft_lcn = u64::from_le_bytes(boot[48..56].try_into().unwrap());
    let mirr_lcn = u64::from_le_bytes(boot[56..64].try_into().unwrap());
    let rs = boot[64] as i8;
    let rec_size = if rs > 0 {
        rs as u64 * cl
    } else {
        1u64 << (-(rs as i32))
    };
    if cl == 0 || !(256..=65536).contains(&rec_size) {
        res.warnings
            .push("NTFS: parâmetros de boot inválidos.".into());
        return res;
    }

    // Registro 0 ($MFT) define onde está toda a MFT.
    let mut mft_runs: Option<(Vec<Run>, u64)> = None;
    for lcn in [mft_lcn, mirr_lcn] {
        if let Some(mut r) = read_vec(ctx.src, base + lcn * cl, rec_size as usize)
            .ok()
            .filter(|r| r.len() as u64 == rec_size)
        {
            if let Some(rec) = parse_record(&mut r, bps, base, cl) {
                if let Some(Data::NonResident { runs, real, .. }) = rec.data {
                    mft_runs = Some((runs, real));
                    break;
                }
            }
        }
        res.warnings.push(
            "NTFS: $MFT principal corrompida, tentando o espelho (só cobre registros iniciais)."
                .into(),
        );
    }
    let Some((runs, mft_len)) = mft_runs else {
        res.warnings
            .push("NTFS: não foi possível ler a MFT; só varredura por assinatura.".into());
        return res;
    };

    let mut live: Vec<Extent> = vec![Extent::new(base, 0)];
    for r in &runs {
        if let Some(l) = r.lcn {
            live.push(Extent::new(base + l * cl, r.len * cl));
        }
    }
    let n_records = mft_len / rec_size;
    let mut dirs: HashMap<u64, (u64, String)> = HashMap::new();
    let mut deleted: Vec<(u64, Rec)> = vec![];

    // Lê a MFT em blocos grandes seguindo os runs.
    let chunk_recs = (1 << 20) / rec_size.max(1);
    let mut idx = 0u64;
    let mut buf: Vec<u8>;
    while idx < n_records {
        if (ctx.cancelled)() {
            break;
        }
        let nrec = chunk_recs.min(n_records - idx);
        buf = read_mft_range(ctx.src, base, cl, &runs, idx * rec_size, nrec * rec_size);
        for k in 0..nrec {
            let s = (k * rec_size) as usize;
            let e = s + rec_size as usize;
            if e > buf.len() {
                break;
            }
            let rn = idx + k;
            let rec_buf = &mut buf[s..e];
            let Some(rec) = parse_record(rec_buf, bps, base, cl) else {
                continue;
            };
            if rec.in_use {
                if rec.is_dir {
                    if let Some((n, _)) = &rec.name {
                        dirs.insert(rn, (rec.parent, n.clone()));
                    }
                } else if let Some(Data::NonResident { runs, .. }) = &rec.data {
                    for r in runs {
                        if let Some(l) = r.lcn {
                            live.push(Extent::new(base + l * cl, r.len * cl));
                        }
                    }
                }
            } else if rec.is_dir {
                if let Some((n, _)) = &rec.name {
                    dirs.entry(rn).or_insert((rec.parent, n.clone()));
                }
            } else if rec.name.is_some() && rec.data.is_some() {
                deleted.push((rn, rec));
            }
        }
        idx += nrec;
        (ctx.progress)(idx, n_records);
    }
    let live = merge_extents(live);
    let _ = total_secs;

    for (rn, rec) in deleted {
        if (ctx.cancelled)() {
            break;
        }
        let name = rec.name.as_ref().map(|n| n.0.clone()).unwrap_or_default();
        let dir = resolve_path(&dirs, rec.parent, rn);
        let (data, size, conf, note): (DataLoc, u64, Confidence, Option<&str>) =
            match rec.data.unwrap() {
                Data::Resident(b) => {
                    let n = b.len() as u64;
                    (DataLoc::Resident { bytes: b }, n, Confidence::High, None)
                }
                Data::NonResident {
                    runs,
                    real,
                    unsupported,
                } => {
                    if unsupported {
                        continue; // comprimido/cifrado (EFS): fora do escopo da v1
                    }
                    let mut ex = vec![];
                    for r in &runs {
                        match r.lcn {
                            Some(l) => ex.push(Extent::new(base + l * cl, r.len * cl)),
                            None => ex.push(Extent {
                                offset: 0,
                                len: r.len * cl,
                                sparse: true,
                            }),
                        }
                    }
                    let ex = super::fat::trim_extents(ex, real);
                    let reused = ex
                        .iter()
                        .any(|e| !e.sparse && overlaps(&live, e.offset, e.len));
                    let (c, n) = if reused {
                        (
                            Confidence::Low,
                            Some("clusters já reutilizados por outros arquivos"),
                        )
                    } else {
                        (Confidence::High, None)
                    };
                    (DataLoc::Extents { extents: ex }, real, c, n)
                }
            };
        if size == 0 {
            continue;
        }
        let f = make_found(
            ctx,
            NewFound {
                name: sanitize_name(&name),
                dir,
                size,
                origin: Origin::Ntfs,
                data,
                modified: rec.mtime,
                created: rec.ctime,
                accessed: rec.atime,
                base: conf,
                note: if rec.has_attr_list {
                    Some("registro com lista de atributos: pode estar incompleto")
                } else {
                    note
                },
            },
        );
        let mut f = f;
        if rec.has_attr_list {
            f.confidence = f.confidence.min(Confidence::Medium);
        }
        (ctx.emit)(f);
    }
    res.live = live;
    res
}

fn resolve_path(dirs: &HashMap<u64, (u64, String)>, mut parent: u64, own: u64) -> Option<String> {
    let mut parts = vec![];
    let mut guard = 0;
    while parent != 5 && guard < 64 {
        match dirs.get(&parent) {
            Some((p, n)) => {
                parts.push(n.clone());
                if *p == parent {
                    break;
                }
                parent = *p;
            }
            None => {
                parts.push(format!("[pasta perdida {parent}]"));
                break;
            }
        }
        guard += 1;
    }
    let _ = own;
    parts.reverse();
    let s = parts.join("/");
    (!s.is_empty()).then_some(s)
}

fn read_mft_range(
    src: &dyn Source,
    base: u64,
    cl: u64,
    runs: &[Run],
    mut off: u64,
    mut len: u64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(len as usize);
    for r in runs {
        if len == 0 {
            break;
        }
        let rs = r.vcn * cl;
        let re = rs + r.len * cl;
        if off >= re {
            continue;
        }
        let within = off - rs.min(off);
        let take = (re - off).min(len);
        match r.lcn {
            Some(l) => {
                let v = read_vec(src, base + l * cl + within, take as usize).unwrap_or_default();
                let got = v.len();
                out.extend_from_slice(&v);
                out.resize(out.len() + (take as usize - got), 0);
            }
            None => out.resize(out.len() + take as usize, 0),
        }
        off += take;
        len -= take;
    }
    out
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Interpreta um registro de arquivo da MFT (aplica fixups no próprio buffer).
fn parse_record(r: &mut [u8], bps: u64, _base: u64, _cl: u64) -> Option<Rec> {
    if r.len() < 48 || &r[0..4] != b"FILE" {
        return None;
    }
    let usa_off = le16(r, 4) as usize;
    let usa_n = le16(r, 6) as usize;
    if usa_n < 2 || usa_off + usa_n * 2 > r.len() {
        return None;
    }
    let usn = [r[usa_off], r[usa_off + 1]];
    for i in 1..usa_n {
        let end = i * bps as usize;
        if end > r.len() {
            break;
        }
        if r[end - 2..end] != usn {
            return None; // registro rasgado
        }
        r[end - 2] = r[usa_off + i * 2];
        r[end - 1] = r[usa_off + i * 2 + 1];
    }
    let flags = le16(r, 22);
    let base_ref = le64(r, 32) & 0xFFFF_FFFF_FFFF;
    if base_ref != 0 {
        return None; // registro de extensão: tratado só via lista de atributos (não suportado)
    }
    let mut rec = Rec {
        in_use: flags & 1 != 0,
        is_dir: flags & 2 != 0,
        parent: 0,
        name: None,
        mtime: None,
        ctime: None,
        atime: None,
        data: None,
        has_attr_list: false,
    };
    let mut p = le16(r, 20) as usize;
    let used = (le32(r, 24) as usize).min(r.len());
    while p + 16 <= used {
        let ty = le32(r, p);
        if ty == 0xFFFF_FFFF {
            break;
        }
        let alen = le32(r, p + 4) as usize;
        if alen < 16 || p + alen > r.len() {
            break;
        }
        let nonres = r[p + 8] != 0;
        let name_len = r[p + 9];
        let aflags = le16(r, p + 12);
        match ty {
            0x10 if !nonres => {
                let vo = le16(r, p + 20) as usize;
                if p + vo + 16 <= r.len() {
                    rec.ctime = filetime_to_unix(le64(r, p + vo));
                    rec.mtime = filetime_to_unix(le64(r, p + vo + 8));
                    if p + vo + 32 <= r.len() {
                        rec.atime = filetime_to_unix(le64(r, p + vo + 24));
                    }
                }
            }
            0x20 => rec.has_attr_list = true,
            0x30 if !nonres => {
                let vo = p + le16(r, p + 20) as usize;
                if vo + 0x42 <= r.len() {
                    let ns = r[vo + 0x41];
                    let nl = r[vo + 0x40] as usize;
                    if vo + 0x42 + nl * 2 <= r.len() && ns != 2 {
                        let name = utf16le(&r[vo + 0x42..vo + 0x42 + nl * 2]);
                        let better = rec
                            .name
                            .as_ref()
                            .map(|(_, old)| ns == 1 || ns == 3 || *old == 2)
                            .unwrap_or(true);
                        if better {
                            rec.parent = le64(r, vo) & 0xFFFF_FFFF_FFFF;
                            rec.name = Some((name, ns));
                        }
                    }
                }
            }
            0x80 if name_len == 0 => {
                if !nonres {
                    let vl = le32(r, p + 16) as usize;
                    let vo = p + le16(r, p + 20) as usize;
                    if vo + vl <= r.len() {
                        rec.data = Some(Data::Resident(r[vo..vo + vl].to_vec()));
                    }
                } else if p + 64 <= r.len() {
                    let start_vcn = le64(r, p + 16);
                    let ro = p + le16(r, p + 32) as usize;
                    let real = le64(r, p + 48);
                    if start_vcn == 0 && ro < p + alen {
                        let runs = parse_runs(&r[ro..p + alen]);
                        rec.data = Some(Data::NonResident {
                            runs,
                            real,
                            unsupported: aflags & 0x4001 != 0,
                        });
                    }
                }
            }
            _ => {}
        }
        p += alen;
    }
    Some(rec)
}

fn parse_runs(mut d: &[u8]) -> Vec<Run> {
    let mut out = vec![];
    let mut vcn = 0u64;
    let mut lcn = 0i64;
    while let Some(&h) = d.first() {
        if h == 0 {
            break;
        }
        let ll = (h & 0xF) as usize;
        let lo = (h >> 4) as usize;
        if ll == 0 || ll > 8 || lo > 8 || d.len() < 1 + ll + lo {
            break;
        }
        let mut len = 0u64;
        for i in 0..ll {
            len |= (d[1 + i] as u64) << (8 * i);
        }
        let lcn_opt = if lo == 0 {
            None
        } else {
            let mut v = 0i64;
            for i in 0..lo {
                v |= (d[1 + ll + i] as i64) << (8 * i);
            }
            if d[ll + lo] & 0x80 != 0 && lo < 8 {
                v |= -1i64 << (8 * lo);
            }
            lcn += v;
            if lcn < 0 {
                break;
            }
            Some(lcn as u64)
        };
        out.push(Run {
            vcn,
            len,
            lcn: lcn_opt,
        });
        vcn += len;
        d = &d[1 + ll + lo..];
    }
    out
}

fn filetime_to_unix(ft: u64) -> Option<i64> {
    if ft == 0 {
        return None;
    }
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    (secs > 0).then_some(secs)
}
