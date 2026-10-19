//! Tabelas de partição (MBR/GPT), detecção de sistemas de arquivos,
//! volumes cifrados e busca de partições perdidas.

use crate::source::{read_vec, Source};
use crate::types::{DiskLayout, FsKind, Gap, PartInfo};

const SECTOR: u64 = 512;
const MIN_GAP: u64 = 1024 * 1024;

/// Identifica o sistema de arquivos que começa em `start` e (se possível) seu tamanho.
pub fn detect_fs(src: &dyn Source, start: u64) -> Option<(FsKind, Option<u64>)> {
    let b = read_vec(src, start, 4096).ok()?;
    if b.len() < 1100 {
        return None;
    }
    if let Some(r) = detect_fs_in(&b) {
        return Some(r);
    }
    // Assinaturas mais adiante no volume: Btrfs (64 KiB) e RAID mdadm 1.2 (4 KiB).
    if let Ok(h) = read_vec(src, start + 0x10000, 0x100) {
        if h.len() >= 0x80 && &h[0x40..0x48] == b"_BHRfS_M" {
            return Some((FsKind::Btrfs, Some(u64::from_le_bytes(h[0x70..0x78].try_into().unwrap()))));
        }
    }
    if let Ok(h) = read_vec(src, start + 4096, 16) {
        if h.len() >= 4 && h[..4] == [0xFC, 0x4E, 0x2B, 0xA9] {
            return Some((FsKind::Raid, None));
        }
    }
    None
}

/// Mesma detecção sobre um buffer que começa no setor candidato (>= 1100 bytes).
pub fn detect_fs_in(b: &[u8]) -> Option<(FsKind, Option<u64>)> {
    if b.len() < 1100 {
        return None;
    }
    if &b[3..11] == b"NTFS    " {
        let bps = u16::from_le_bytes([b[11], b[12]]) as u64;
        let total = u64::from_le_bytes(b[40..48].try_into().unwrap());
        if bps.is_power_of_two() && (512..=4096).contains(&bps) && b[13] != 0 {
            return Some((FsKind::Ntfs, Some((total + 1) * bps)));
        }
    }
    if &b[3..11] == b"EXFAT   " {
        let shift = b[108] as u32;
        if (9..=12).contains(&shift) {
            let vol = u64::from_le_bytes(b[72..80].try_into().unwrap());
            return Some((FsKind::Exfat, Some(vol << shift)));
        }
    }
    if b.len() >= 520 && &b[512..520] == b"LABELONE" {
        return Some((FsKind::Lvm, None));
    }
    if b[..4] == [0xFC, 0x4E, 0x2B, 0xA9] {
        return Some((FsKind::Raid, None));
    }
    if &b[0..4] == b"XFSB" {
        let bs = u32::from_be_bytes(b[4..8].try_into().unwrap()) as u64;
        let blocks = u64::from_be_bytes(b[8..16].try_into().unwrap());
        if bs.is_power_of_two() && (512..=65536).contains(&bs) {
            return Some((FsKind::Xfs, Some(bs * blocks)));
        }
    }
    if &b[3..7] == b"ReFS" && &b[16..20] == b"FSRS" {
        let sectors = u64::from_le_bytes(b[24..32].try_into().unwrap());
        let bps = u32::from_le_bytes(b[32..36].try_into().unwrap()) as u64;
        return Some((FsKind::Refs, Some(sectors * bps)));
    }
    if crate::crypt::filevault::detect(b) {
        return Some((FsKind::Corestorage, Some(u64::from_le_bytes(b[64..72].try_into().unwrap()))));
    }
    if crate::crypt::bitlocker::detect(b) {
        return Some((FsKind::Bitlocker, None));
    }
    if b.starts_with(b"LUKS\xBA\xBE") {
        return Some((FsKind::Luks, None));
    }
    if &b[32..36] == b"NXSB" {
        let bs = u32::from_le_bytes(b[36..40].try_into().unwrap()) as u64;
        let cnt = u64::from_le_bytes(b[40..48].try_into().unwrap());
        return Some((FsKind::Apfs, Some(bs * cnt)));
    }
    if b[1024..1026] == *b"H+" || b[1024..1026] == *b"HX" {
        return Some((FsKind::HfsPlus, None));
    }
    if b[1080] == 0x53 && b[1081] == 0xEF {
        let blocks = u32::from_le_bytes(b[1028..1032].try_into().unwrap()) as u64;
        let log = u32::from_le_bytes(b[1048..1052].try_into().unwrap());
        if log <= 6 {
            return Some((FsKind::Ext, Some(blocks << (10 + log))));
        }
    }
    if let Some(r) = detect_fat(b) {
        return Some(r);
    }
    None
}

fn detect_fat(b: &[u8]) -> Option<(FsKind, Option<u64>)> {
    if b[510] != 0x55 || b[511] != 0xAA {
        return None;
    }
    let bps = u16::from_le_bytes([b[11], b[12]]) as u64;
    let spc = b[13] as u64;
    let reserved = u16::from_le_bytes([b[14], b[15]]) as u64;
    let nfats = b[16] as u64;
    let root_entries = u16::from_le_bytes([b[17], b[18]]) as u64;
    let ts16 = u16::from_le_bytes([b[19], b[20]]) as u64;
    let fat16 = u16::from_le_bytes([b[22], b[23]]) as u64;
    let ts32 = u32::from_le_bytes(b[32..36].try_into().unwrap()) as u64;
    let fat32 = u32::from_le_bytes(b[36..40].try_into().unwrap()) as u64;
    if !bps.is_power_of_two()
        || !(512..=4096).contains(&bps)
        || spc == 0
        || !spc.is_power_of_two()
        || reserved == 0
        || !(1..=2).contains(&nfats)
    {
        return None;
    }
    let total = if ts16 != 0 { ts16 } else { ts32 };
    let fatsz = if fat16 != 0 { fat16 } else { fat32 };
    if total == 0 || fatsz == 0 {
        return None;
    }
    let root_secs = (root_entries * 32).div_ceil(bps);
    let data = total.saturating_sub(reserved + nfats * fatsz + root_secs);
    let clusters = data / spc;
    // O layout do BPB manda: raiz fixa zerada + FAT de 32 bits = FAT32, mesmo com poucos
    // clusters (mkfs.vfat -F 32 em mídias pequenas gera isso e o Linux monta).
    let is32 = root_entries == 0 && fat16 == 0 && fat32 != 0;
    let kind = if is32 {
        FsKind::Fat32
    } else if clusters < 4085 {
        FsKind::Fat12
    } else {
        FsKind::Fat16
    };
    Some((kind, Some(total * bps)))
}

/// Lê o rótulo (label) do volume quando disponível de forma simples.
fn fat_label(b: &[u8], kind: FsKind) -> Option<String> {
    let off = if kind == FsKind::Fat32 { 71 } else { 43 };
    let s = String::from_utf8_lossy(b.get(off..off + 11)?)
        .trim()
        .to_string();
    (!s.is_empty() && s != "NO NAME").then_some(s)
}

// ----------------------------------------------------------------- tabelas

/// Analisa o disco. `search_lost` varre lacunas atrás de volumes perdidos (lento em discos grandes).
pub fn analyze(src: &dyn Source, search_lost: bool) -> DiskLayout {
    let size = src.size();
    let mut layout = DiskLayout {
        size,
        table: "none".into(),
        ..Default::default()
    };
    let sector0 = read_vec(src, 0, 512).unwrap_or_default();
    let has_mbr = sector0.len() == 512 && sector0[510] == 0x55 && sector0[511] == 0xAA;
    let gpt = read_gpt(src, size);
    let mut parts: Vec<PartInfo> = Vec::new();

    if let Some(g) = gpt {
        layout.table = "gpt".into();
        parts = g;
    } else if has_mbr {
        // Pode ser MBR de verdade ou um setor de boot de volume sem tabela (superfloppy).
        let looks_boot = detect_fs_in(&read_vec(src, 0, 4096).unwrap_or_default()).is_some();
        if !looks_boot {
            let mp = read_mbr(src, &sector0, size);
            if !mp.is_empty() {
                layout.table = "mbr".into();
                parts = mp;
            }
        }
    }

    for p in &mut parts {
        if let Some((fs, _)) = detect_fs(src, p.start) {
            p.fs = fs;
            p.encrypted = fs.encrypted().map(String::from);
            if matches!(fs, FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32) {
                if let Ok(b) = read_vec(src, p.start, 512) {
                    p.label = fat_label(&b, fs);
                }
            }
        }
    }

    // Volume sem tabela de partições (pendrive "superfloppy" ou partição crua).
    if parts.is_empty() {
        if let Some((fs, len)) = detect_fs(src, 0) {
            parts.push(PartInfo {
                index: 0,
                start: 0,
                len: len.unwrap_or(size).min(size),
                fs,
                label: None,
                lost: false,
                encrypted: fs.encrypted().map(String::from),
                table_type: Some("volume sem tabela de partições".into()),
            });
            layout.table = "none".into();
        }
    }

    parts.sort_by_key(|p| p.start);
    layout.gaps = compute_gaps(&parts, size);

    // Partições perdidas: procurar assinaturas de volumes nas lacunas.
    let mut lost = Vec::new();
    if search_lost {
        lost = find_lost(src, &layout.gaps, &parts);
        if parts.is_empty() && layout.table == "none" {
            // Disco sem tabela nem volume no início: procurar no disco todo.
            lost = find_lost(
                src,
                &[Gap {
                    start: 0,
                    len: size,
                }],
                &[],
            );
        }
    }
    for mut l in lost {
        l.index = parts.len();
        parts.push(l);
    }
    parts.sort_by_key(|p| p.start);
    for (i, p) in parts.iter_mut().enumerate() {
        p.index = i;
    }
    layout.gaps = compute_gaps(
        &parts
            .iter()
            .filter(|p| !p.lost)
            .cloned()
            .collect::<Vec<_>>(),
        size,
    );
    layout.partitions = parts;

    for p in &layout.partitions {
        if let Some(e) = &p.encrypted {
            layout.warnings.push(format!(
                "Partição {} está cifrada com {}: informe a senha, a chave de recuperação ou o arquivo de chave para abrir e varrer o conteúdo.",
                p.index + 1,
                e
            ));
        }
        if p.fs == FsKind::Apfs {
            layout.warnings.push(format!(
                "Partição {} é APFS: leitura de metadados ainda não suportada; usando apenas varredura por assinatura. Se estiver cifrada (FileVault), nada será encontrado.",
                p.index + 1
            ));
        }
    }
    layout
}

fn compute_gaps(parts: &[PartInfo], size: u64) -> Vec<Gap> {
    let mut gaps = vec![];
    let mut cur = 0u64;
    let mut sorted: Vec<&PartInfo> = parts.iter().collect();
    sorted.sort_by_key(|p| p.start);
    for p in sorted {
        if p.start > cur && p.start - cur > MIN_GAP {
            gaps.push(Gap {
                start: cur,
                len: p.start - cur,
            });
        }
        cur = cur.max(p.start + p.len);
    }
    if size > cur && size - cur > MIN_GAP {
        gaps.push(Gap {
            start: cur,
            len: size - cur,
        });
    }
    gaps
}

fn read_mbr(src: &dyn Source, s0: &[u8], size: u64) -> Vec<PartInfo> {
    let mut out = vec![];
    let mut ext_base = None;
    for i in 0..4 {
        let e = &s0[446 + i * 16..462 + i * 16];
        let ty = e[4];
        let lba = u32::from_le_bytes(e[8..12].try_into().unwrap()) as u64;
        let cnt = u32::from_le_bytes(e[12..16].try_into().unwrap()) as u64;
        if ty == 0 || cnt == 0 {
            continue;
        }
        if ty == 0xEE {
            continue; // MBR protetor de GPT
        }
        if matches!(ty, 0x05 | 0x0F | 0x85) {
            ext_base = Some(lba * SECTOR);
            continue;
        }
        if lba * SECTOR >= size {
            continue;
        }
        out.push(part(
            out.len(),
            lba * SECTOR,
            cnt * SECTOR,
            Some(format!("MBR 0x{ty:02X}")),
        ));
    }
    if let Some(base) = ext_base {
        let mut cur = base;
        for _ in 0..128 {
            let Ok(ebr) = read_vec(src, cur, 512) else {
                break;
            };
            if ebr.len() < 512 || ebr[510] != 0x55 || ebr[511] != 0xAA {
                break;
            }
            let e1 = &ebr[446..462];
            let lba = u32::from_le_bytes(e1[8..12].try_into().unwrap()) as u64;
            let cnt = u32::from_le_bytes(e1[12..16].try_into().unwrap()) as u64;
            if e1[4] != 0 && cnt != 0 {
                out.push(part(
                    out.len(),
                    cur + lba * SECTOR,
                    cnt * SECTOR,
                    Some(format!("MBR lógica 0x{:02X}", e1[4])),
                ));
            }
            let e2 = &ebr[462..478];
            let next = u32::from_le_bytes(e2[8..12].try_into().unwrap()) as u64;
            if next == 0 || !matches!(e2[4], 0x05 | 0x0F | 0x85) {
                break;
            }
            cur = base + next * SECTOR;
        }
    }
    out
}

fn part(index: usize, start: u64, len: u64, table_type: Option<String>) -> PartInfo {
    PartInfo {
        index,
        start,
        len,
        fs: FsKind::Unknown,
        label: None,
        lost: false,
        encrypted: None,
        table_type,
    }
}

fn read_gpt(src: &dyn Source, size: u64) -> Option<Vec<PartInfo>> {
    // Tenta o cabeçalho primário (LBA 1) e depois o de backup (último setor).
    let hdr = read_vec(src, SECTOR, 512)
        .ok()
        .filter(|h| h.len() == 512 && &h[0..8] == b"EFI PART")
        .or_else(|| {
            let last = size.checked_sub(SECTOR)?;
            read_vec(src, last, 512)
                .ok()
                .filter(|h| h.len() == 512 && &h[0..8] == b"EFI PART")
        })?;
    let table_lba = u64::from_le_bytes(hdr[72..80].try_into().unwrap());
    let n = u32::from_le_bytes(hdr[80..84].try_into().unwrap()) as usize;
    let esz = u32::from_le_bytes(hdr[84..88].try_into().unwrap()) as usize;
    if n == 0 || n > 1024 || !(128..=4096).contains(&esz) {
        return None;
    }
    let mut tbl = read_vec(src, table_lba * SECTOR, n * esz).ok()?;
    if tbl.len() < n * esz {
        // Cabeçalho de backup aponta para a tabela de backup; se falhar, desiste.
        tbl.resize(n * esz, 0);
    }
    let mut out = vec![];
    for i in 0..n {
        let e = &tbl[i * esz..(i + 1) * esz];
        if e[0..16].iter().all(|&b| b == 0) {
            continue;
        }
        let first = u64::from_le_bytes(e[32..40].try_into().unwrap());
        let last = u64::from_le_bytes(e[40..48].try_into().unwrap());
        if last < first || last * SECTOR >= size + SECTOR {
            continue;
        }
        let name: Vec<u16> = e[56..128]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        let mut p = part(
            out.len(),
            first * SECTOR,
            (last - first + 1) * SECTOR,
            Some(gpt_type_name(&e[0..16]).into()),
        );
        let label = String::from_utf16_lossy(&name);
        if !label.is_empty() {
            p.label = Some(label);
        }
        out.push(p);
    }
    Some(out)
}

fn gpt_type_name(guid: &[u8]) -> &'static str {
    // GUIDs em formato de disco (mixed-endian).
    match guid {
        [0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B] => {
            "EFI System"
        }
        [0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7] => {
            "Microsoft basic data"
        }
        [0x16, 0xE3, 0xC9, 0xE3, 0x5C, 0x0B, 0xB8, 0x4D, 0x81, 0x7D, 0xF9, 0x2D, 0xF0, 0x02, 0x15, 0xAE] => {
            "Microsoft reserved"
        }
        [0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4] => {
            "Linux filesystem"
        }
        [0x6D, 0xFD, 0x57, 0x06, 0xAB, 0xA4, 0xC4, 0x43, 0x84, 0xE5, 0x09, 0x33, 0xC8, 0x4B, 0x4F, 0x4F] => {
            "Linux swap"
        }
        [0x00, 0x53, 0x46, 0x48, 0x00, 0x00, 0xAA, 0xAA, 0x11, 0xAA, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC] => {
            "Apple HFS+"
        }
        [0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0xAA, 0x11, 0xAA, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC] => {
            "Apple APFS"
        }
        _ => "GPT",
    }
}

/// Procura volumes (por assinatura) dentro de lacunas, em limites de setor.
fn find_lost(src: &dyn Source, gaps: &[Gap], known: &[PartInfo]) -> Vec<PartInfo> {
    const CHUNK: u64 = 4 * 1024 * 1024;
    let mut found: Vec<PartInfo> = vec![];
    for g in gaps {
        let mut pos = g.start;
        let end = g.start + g.len;
        let mut skip_until = 0u64;
        while pos < end {
            let want = CHUNK.min(end - pos) + 4096;
            let Ok(buf) = read_vec(src, pos, want as usize) else {
                break;
            };
            if buf.len() < 1100 {
                break;
            }
            let limit = CHUNK.min(end - pos).min(buf.len() as u64 - 1100);
            let mut off = 0u64;
            while off <= limit {
                let abs = pos + off;
                if abs >= skip_until {
                    let s = &buf[off as usize..];
                    if s.len() >= 1100 {
                        // Filtro rápido: só chama a detecção completa se algum magic casa.
                        let quick = &s[3..11] == b"NTFS    "
                            || &s[3..11] == b"EXFAT   "
                            || &s[3..11] == b"-FVE-FS-"
                            || s.starts_with(b"LUKS\xBA\xBE")
                            || (s[510] == 0x55 && s[511] == 0xAA && s[0] != 0)
                            || (s[1080] == 0x53 && s[1081] == 0xEF);
                        if quick {
                            if let Some((fs, len)) = detect_fs_in(s) {
                                // ext: o superbloco está a 1024 bytes do início do volume,
                                // por isso o candidato ext é o setor atual (início do volume).
                                let len = len.unwrap_or(0);
                                let ok = len >= 64 * 1024
                                    && abs + len <= src.size() + 1024 * 1024
                                    && !overlaps(known, abs, len);
                                // Evita falso positivo de FAT em setor de boot MBR genérico.
                                if ok && (fs != FsKind::Fat12 || len >= 512 * 1024) {
                                    let mut p =
                                        part(0, abs, len, Some("encontrada por varredura".into()));
                                    p.fs = fs;
                                    p.lost = true;
                                    p.encrypted = fs.encrypted().map(String::from);
                                    skip_until = abs + len;
                                    found.push(p);
                                }
                            }
                        }
                    }
                }
                off += SECTOR;
            }
            pos += CHUNK;
        }
    }
    found
}

fn overlaps(known: &[PartInfo], start: u64, len: u64) -> bool {
    known
        .iter()
        .any(|p| start < p.start + p.len && p.start < start + len)
}
