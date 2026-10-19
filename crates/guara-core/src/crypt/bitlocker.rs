//! BitLocker (Windows 7 em diante, inclusive BitLocker To Go): AES-CBC com ou sem difusor
//! Elephant, e AES-XTS. Protetores: senha, chave de recuperação, chave de inicialização (.BEK),
//! chave limpa (proteção suspensa) e chave do volume já conhecida. TPM não é suportado.

use super::*;
use crate::source::{read_vec, SubSource};
use sha2::{Digest, Sha256};

fn le16(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

const GUID_NORMAL: [u8; 16] = [
    0x3b, 0xd6, 0x67, 0x49, 0x29, 0x2e, 0xd8, 0x4a, 0x83, 0x99, 0xf6, 0xa3, 0x39, 0xe3, 0xd0, 0x01,
];
const GUID_EOW: [u8; 16] = [
    0x3b, 0x4d, 0xa8, 0x92, 0x80, 0xdd, 0x0e, 0x4d, 0x9e, 0x4e, 0xb1, 0xe3, 0x28, 0x4e, 0xae, 0xd8,
];

/// Posição da tabela de deslocamentos FVE, se for BitLocker (normal ou To Go).
fn offsets_pos(vbr: &[u8]) -> Option<usize> {
    if vbr.len() >= 11 && &vbr[3..11] == b"-FVE-FS-" {
        return Some(176);
    }
    // BitLocker To Go: parece FAT ("MSWIN4.1"), mas guarda o GUID do BitLocker no setor de boot.
    if vbr.len() >= 464 && (vbr[424..440] == GUID_NORMAL || vbr[424..440] == GUID_EOW) {
        return Some(440);
    }
    None
}

pub fn detect(vbr: &[u8]) -> bool {
    offsets_pos(vbr).is_some()
}

// ------------------------------------------------------------- Elephant

fn rotl(x: u32, n: u32) -> u32 {
    x.rotate_left(n)
}

const RA: [u32; 4] = [9, 0, 13, 0];
const RB: [u32; 4] = [0, 10, 0, 25];

fn words(buf: &[u8]) -> Vec<u32> {
    buf.chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

// Decifragem (ordem do dm-crypt do Linux): somas em ordem crescente.
fn diffuser_a_decrypt(d: &mut [u32]) {
    let n = d.len();
    for _ in 0..5 {
        for i in 0..n {
            let a = d[(i + n - 2) % n];
            let b = rotl(d[(i + n - 5) % n], RA[i % 4]);
            d[i] = d[i].wrapping_add(a ^ b);
        }
    }
}

fn diffuser_b_decrypt(d: &mut [u32]) {
    let n = d.len();
    for _ in 0..3 {
        for i in 0..n {
            let a = d[(i + 2) % n];
            let b = rotl(d[(i + 5) % n], RB[i % 4]);
            d[i] = d[i].wrapping_add(a ^ b);
        }
    }
}

/// Decifra um setor AES-CBC + difusor Elephant (BitLocker Vista/7 sem XTS).
pub fn elephant_decrypt(key: &AesAny, tweak: &AesAny, buf: &mut [u8], byte_off: u64) {
    let mut iv = [0u8; 16];
    iv[..8].copy_from_slice(&byte_off.to_le_bytes());
    key.encrypt(&mut iv);
    cbc_decrypt(key, &iv, buf);
    let mut d = words(buf);
    diffuser_b_decrypt(&mut d);
    diffuser_a_decrypt(&mut d);
    for (i, w) in d.iter().enumerate() {
        buf[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    // Chave do setor: duas cifragens do deslocamento com a chave de ajuste.
    let mut k0 = [0u8; 16];
    k0[..8].copy_from_slice(&byte_off.to_le_bytes());
    let mut k1 = k0;
    k1[15] = 0x80;
    tweak.encrypt(&mut k0);
    tweak.encrypt(&mut k1);
    let mut sk = [0u8; 32];
    sk[..16].copy_from_slice(&k0);
    sk[16..].copy_from_slice(&k1);
    for (i, b) in buf.iter_mut().enumerate() {
        *b ^= sk[i % 32];
    }
}

// ------------------------------------------------------------------ CCM

/// AES-CCM (nonce de 12 bytes, tag de 16): `blob` = tag cifrada (16) + dados cifrados.
fn ccm_decrypt(key: &AesAny, nonce: &[u8], blob: &[u8]) -> Option<Vec<u8>> {
    if nonce.len() != 12 || blob.len() < 16 {
        return None;
    }
    let ctr = |i: u32| -> [u8; 16] {
        let mut a = [0u8; 16];
        a[0] = 2;
        a[1..13].copy_from_slice(nonce);
        a[13..16].copy_from_slice(&i.to_be_bytes()[1..]);
        key.encrypt(&mut a);
        a
    };
    let s0 = ctr(0);
    let mut tag = [0u8; 16];
    for i in 0..16 {
        tag[i] = blob[i] ^ s0[i];
    }
    let ct = &blob[16..];
    let mut pt = Vec::with_capacity(ct.len());
    for (bi, ch) in ct.chunks(16).enumerate() {
        let s = ctr(bi as u32 + 1);
        pt.extend(ch.iter().zip(s.iter()).map(|(a, b)| a ^ b));
    }
    // CBC-MAC sobre B0 + dados.
    let mut b0 = [0u8; 16];
    b0[0] = 0x3A;
    b0[1..13].copy_from_slice(nonce);
    b0[13..16].copy_from_slice(&(pt.len() as u32).to_be_bytes()[1..]);
    key.encrypt(&mut b0);
    let mut x = b0;
    for ch in pt.chunks(16) {
        for i in 0..ch.len() {
            x[i] ^= ch[i];
        }
        key.encrypt(&mut x);
    }
    (x == tag).then_some(pt)
}

// --------------------------------------------------------------- chaves

fn stretch(initial: &[u8; 32], salt: &[u8]) -> [u8; 32] {
    let mut last = [0u8; 32];
    let mut buf = [0u8; 88];
    buf[32..64].copy_from_slice(initial);
    buf[64..80].copy_from_slice(salt);
    for i in 0u64..0x10_0000 {
        buf[..32].copy_from_slice(&last);
        buf[80..88].copy_from_slice(&i.to_le_bytes());
        last.copy_from_slice(&Sha256::digest(buf));
    }
    last
}

fn password_key(pw: &str, salt: &[u8]) -> [u8; 32] {
    let utf16: Vec<u8> = pw.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let h1 = Sha256::digest(&utf16);
    let mut init = [0u8; 32];
    init.copy_from_slice(&Sha256::digest(h1));
    stretch(&init, salt)
}

/// Chave de recuperação: 8 blocos de 6 dígitos, cada um múltiplo de 11.
fn recovery_key(rp: &str, salt: &[u8]) -> Option<[u8; 32]> {
    let digits: String = rp.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 48 {
        return None;
    }
    let mut raw = [0u8; 16];
    for i in 0..8 {
        let n: u32 = digits[i * 6..i * 6 + 6].parse().ok()?;
        if n % 11 != 0 || n / 11 > 0xFFFF {
            return None;
        }
        raw[i * 2..i * 2 + 2].copy_from_slice(&((n / 11) as u16).to_le_bytes());
    }
    let mut init = [0u8; 32];
    init.copy_from_slice(&Sha256::digest(raw));
    Some(stretch(&init, salt))
}

// ----------------------------------------------------------- metadados

#[derive(Debug, Clone)]
struct Entry {
    etype: usize,
    vtype: usize,
    data: Vec<u8>,
}

fn parse_entries(b: &[u8]) -> Vec<Entry> {
    let mut v = vec![];
    let mut o = 0;
    while o + 8 <= b.len() {
        let size = le16(b, o);
        if size < 8 || o + size > b.len() {
            break;
        }
        v.push(Entry {
            etype: le16(b, o + 2),
            vtype: le16(b, o + 4),
            data: b[o + 8..o + size].to_vec(),
        });
        o += size;
    }
    v
}

struct Vmk {
    protection: usize,
    nested: Vec<Entry>,
}

impl Vmk {
    fn ccm(&self) -> Option<&Entry> {
        self.nested.iter().find(|e| e.vtype == 5)
    }
    fn stretch_salt(&self) -> Option<&[u8]> {
        self.nested
            .iter()
            .find(|e| e.vtype == 3)
            .filter(|e| e.data.len() >= 20)
            .map(|e| &e.data[4..20])
    }
}

struct Fve {
    method: usize,
    volume_header_offset: u64,
    volume_header_size: u64,
    encrypted_size: u64,
    meta_size: u64,
    vmks: Vec<Vmk>,
    fvek: Option<Entry>,
}

fn read_fve(src: &dyn Source, offset: u64) -> Option<Fve> {
    let h = read_vec(src, offset, 64 + 48).ok()?;
    if h.len() < 112 || &h[..8] != b"-FVE-FS-" {
        return None;
    }
    let version = le16(&h, 10);
    if version != 2 {
        return None; // Vista (versão 1) não suportado
    }
    let encrypted_size = le64(&h, 16);
    let hdr_sectors = le32(&h, 28) as u64;
    let volume_header_offset = le64(&h, 56);
    let meta_size = le32(&h, 64) as usize;
    if !(48..=1 << 20).contains(&meta_size) {
        return None;
    }
    let method = le16(&h, 64 + 36);
    let all = read_vec(src, offset + 64, meta_size).ok()?;
    if all.len() < meta_size {
        return None;
    }
    let entries = parse_entries(&all[48..]);
    let mut vmks = vec![];
    let mut fvek = None;
    let (mut vh_off, mut vh_size) = (volume_header_offset, hdr_sectors * 512);
    for e in entries {
        match (e.etype, e.vtype) {
            (15, 15) if e.data.len() >= 16 => {
                vh_off = le64(&e.data, 0);
                vh_size = le64(&e.data, 8);
            }
            (2, 8) if e.data.len() > 28 => vmks.push(Vmk {
                protection: le16(&e.data, 26),
                nested: parse_entries(&e.data[28..]),
            }),
            (3, 5) => fvek = Some(e),
            _ => {}
        }
    }
    Some(Fve {
        method,
        volume_header_offset: vh_off,
        volume_header_size: vh_size,
        encrypted_size,
        meta_size: 0x10000,
        vmks,
        fvek,
    })
}

/// Chave de um arquivo .BEK (chave de inicialização).
fn bek_key(file: &[u8]) -> Option<Vec<u8>> {
    if file.len() < 48 {
        return None;
    }
    let es = parse_entries(&file[48..]);
    for e in &es {
        if e.vtype == 9 && e.data.len() > 24 {
            for n in parse_entries(&e.data[24..]) {
                if n.vtype == 1 && n.data.len() >= 36 {
                    return Some(n.data[4..36].to_vec());
                }
            }
        }
    }
    None
}

fn candidate_keys(v: &Vmk, secrets: &[Secret]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = vec![];
    // Chave limpa: o próprio protetor guarda a chave em claro.
    if v.protection == 0 {
        for n in &v.nested {
            if n.vtype == 1 && n.data.len() >= 36 {
                out.push(n.data[4..36].to_vec());
            }
        }
    }
    for s in secrets {
        match s {
            Secret::Password(p) if v.protection == 0x2000 => {
                if let Some(salt) = v.stretch_salt() {
                    out.push(password_key(p, salt).to_vec());
                }
            }
            Secret::RecoveryKey(r) if v.protection == 0x0800 => {
                if let Some(k) = v.stretch_salt().and_then(|salt| recovery_key(r, salt)) {
                    out.push(k.to_vec());
                }
            }
            Secret::KeyFile(f) if v.protection == 0x0200 => {
                if let Some(k) = bek_key(f) {
                    out.push(k);
                }
            }
            _ => {}
        }
    }
    out
}

fn decrypt_key_entry(key: &[u8], e: &Entry) -> Option<Vec<u8>> {
    let aes = AesAny::new(key)?;
    if e.data.len() < 12 + 16 + 8 {
        return None;
    }
    let pt = ccm_decrypt(&aes, &e.data[..12], &e.data[12..])?;
    // payload = cabeçalho de entrada (8) + método (4) + chave
    (pt.len() > 12).then(|| pt[12..].to_vec())
}

pub fn unlock(src: Arc<dyn Source>, secrets: &[Secret]) -> UnlockResult {
    let vbr = read_vec(src.as_ref(), 0, 512).map_err(|e| UnlockError::Corrupt(e.to_string()))?;
    if !detect(&vbr) || vbr.len() < 512 {
        return Err(UnlockError::Corrupt("não é BitLocker".into()));
    }
    let bps = le16(&vbr, 11).max(512) as u64;
    let pos = offsets_pos(&vbr).unwrap_or(176);
    // Três cópias dos metadados; usa a primeira legível.
    let mut fve = None;
    let mut meta_offsets = vec![];
    for i in 0..3 {
        let off = le64(&vbr, pos + i * 8);
        if off != 0 {
            meta_offsets.push(off);
        }
        if off != 0 && off < src.size() {
            if let Some(f) = read_fve(src.as_ref(), off) {
                fve = Some(f);
                break;
            }
        }
    }
    let fve = fve.ok_or_else(|| {
        UnlockError::Unsupported("metadados FVE ilegíveis ou do Windows Vista".into())
    })?;
    let fvek_entry = fve
        .fvek
        .as_ref()
        .ok_or_else(|| UnlockError::Corrupt("sem FVEK".into()))?;

    let mut volume_key: Option<Vec<u8>> = None;
    for s in secrets {
        if let Secret::VolumeKey(k) = s {
            volume_key = Some(k.clone());
        }
    }
    let mut vmk_found: Option<Vec<u8>> = None;
    if volume_key.is_none() {
        'outer: for v in &fve.vmks {
            let Some(ccm) = v.ccm() else { continue };
            for k in candidate_keys(v, secrets) {
                if let Some(vmk) = decrypt_key_entry(&k, ccm) {
                    if vmk.len() >= 32 {
                        vmk_found = Some(vmk[..32].to_vec());
                        break 'outer;
                    }
                }
            }
        }
    }
    let fvek_bytes = if let Some(k) = volume_key {
        k
    } else {
        let vmk = vmk_found.ok_or(UnlockError::WrongSecret)?;
        decrypt_key_entry(&vmk, fvek_entry)
            .ok_or_else(|| UnlockError::Corrupt("FVEK não decifra com a VMK".into()))?
    };
    let k = &fvek_bytes;
    let bad = || UnlockError::Corrupt("tamanho de chave incompatível com o método".into());
    let (cipher, name) = match fve.method {
        0x8000 | 0x8001 => {
            let n = if fve.method == 0x8000 { 16 } else { 32 };
            if k.len() < 32 + n {
                return Err(bad());
            }
            (
                SectorCipher::Elephant {
                    key: AesAny::new(&k[..n]).ok_or_else(bad)?,
                    tweak: AesAny::new(&k[32..32 + n]).ok_or_else(bad)?,
                },
                if n == 16 {
                    "AES-128-CBC + difusor"
                } else {
                    "AES-256-CBC + difusor"
                },
            )
        }
        0x8002 | 0x8003 => {
            let n = if fve.method == 0x8002 { 16 } else { 32 };
            if k.len() < n {
                return Err(bad());
            }
            let key = AesAny::new(&k[..n]).ok_or_else(bad)?;
            (
                SectorCipher::Cbc {
                    key: key.clone(),
                    iv: Iv::Eboiv(key),
                },
                if n == 16 {
                    "AES-128-CBC"
                } else {
                    "AES-256-CBC"
                },
            )
        }
        0x8004 | 0x8005 => {
            let n = if fve.method == 0x8004 { 16 } else { 32 };
            if k.len() < 2 * n {
                return Err(bad());
            }
            let iv = if bps > 512 {
                Iv::Unit(bps)
            } else {
                Iv::Plain64
            };
            (
                SectorCipher::Xts {
                    k1: AesAny::new(&k[..n]).ok_or_else(bad)?,
                    k2: AesAny::new(&k[n..2 * n]).ok_or_else(bad)?,
                    iv,
                },
                if n == 16 {
                    "AES-128-XTS"
                } else {
                    "AES-256-XTS"
                },
            )
        }
        m => {
            return Err(UnlockError::Unsupported(format!(
                "método de cifragem BitLocker {m:#x}"
            )))
        }
    };
    let len = src.size();
    let mut cs = CryptSource::new(src.clone(), cipher, 0, len, bps);
    let mut patches = vec![];
    // Áreas de metadados FVE: o Windows não as expõe como dados.
    for off in meta_offsets {
        patches.push(Patch {
            at: off,
            len: fve.meta_size,
            from: 0,
            decrypt_from: None,
            zero: true,
        });
    }
    if fve.volume_header_size > 0 && fve.volume_header_offset > 0 {
        // O início do volume original foi movido para outro lugar e cifrado lá.
        patches.push(Patch {
            at: fve.volume_header_offset,
            len: fve.volume_header_size,
            from: 0,
            decrypt_from: None,
            zero: true,
        });
        patches.push(Patch {
            at: 0,
            len: fve.volume_header_size,
            from: fve.volume_header_offset,
            decrypt_from: Some(fve.volume_header_offset),
            zero: false,
        });
    }
    cs = cs.with_patches(patches);
    if fve.encrypted_size > 0 && fve.encrypted_size < len {
        cs = cs.with_plain_from(fve.encrypted_size);
    }
    Ok(Unlocked {
        source: Arc::new(cs),
        description: format!("BitLocker ({name})"),
        volume_key: Some(fvek_bytes),
    })
}

pub fn unlock_window(
    base: Arc<dyn Source>,
    start: u64,
    len: u64,
    secrets: &[Secret],
) -> UnlockResult {
    unlock(Arc::new(SubSource::new(base, start, len)), secrets)
}
