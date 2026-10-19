//! FileVault 2 sobre Core Storage (volumes HFS+ cifrados do macOS 10.7 a 10.12 e Fusion Drives).
//! Senha ou chave do volume; chave de recuperação institucional não é suportada.

use super::*;
use crate::source::{read_vec, SubSource};
use sha2::{Digest, Sha256};

const MD_BLOCK: usize = 8192;

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

pub fn detect(head: &[u8]) -> bool {
    head.len() >= 512 && head[88] == 0x43 && head[89] == 0x53 && head[10] == 0x10
}

/// Desembrulha uma chave AES-128 de 24 bytes (RFC 3394).
fn unwrap_key(kek: &[u8], wrapped: &[u8]) -> Option<[u8; 16]> {
    let aes = AesAny::new(kek)?;
    if wrapped.len() != 24 {
        return None;
    }
    let mut a: [u8; 8] = wrapped[..8].try_into().unwrap();
    let mut r2: [u8; 8] = wrapped[8..16].try_into().unwrap();
    let mut r3: [u8; 8] = wrapped[16..24].try_into().unwrap();
    for t in (1..=12u64).rev() {
        let prev_r2 = r2;
        let tb = t.to_be_bytes();
        let mut blk = [0u8; 16];
        for i in 0..8 {
            blk[i] = a[i] ^ tb[i];
        }
        blk[8..].copy_from_slice(&r3);
        aes.decrypt(&mut blk);
        a.copy_from_slice(&blk[..8]);
        r2.copy_from_slice(&blk[8..]);
        r3 = prev_r2;
    }
    if a != [0xA6; 8] {
        return None;
    }
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&r2);
    out[8..].copy_from_slice(&r3);
    Some(out)
}

fn b64(s: &str) -> Option<Vec<u8>> {
    let mut out = vec![];
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Valor de `<key>NOME</key><tipo ...>VALOR</tipo>` num plist simples (primeira ocorrência com valor).
fn plist_value(xml: &str, key: &str) -> Option<String> {
    let k = format!("<key>{key}</key>");
    let mut from = 0;
    while let Some(p) = xml[from..].find(&k) {
        let rest = &xml[from + p + k.len()..];
        from += p + k.len();
        let Some(open) = rest.find('<').filter(|&o| o == 0) else {
            continue;
        };
        let Some(gt) = rest[open..].find('>') else {
            continue;
        };
        if rest[open..open + gt].ends_with('/') {
            continue; // <reference .../>
        }
        let Some(te) = rest[open + 1..].find([' ', '>']) else {
            continue;
        };
        let tag = &rest[open + 1..open + 1 + te];
        let vstart = open + gt + 1;
        if let Some(e) = rest[vstart..].find(&format!("</{tag}>")) {
            let v = rest[vstart..vstart + e].trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn xml_of(blk: &[u8], off_pos: usize) -> Option<String> {
    let off = le32(blk, off_pos) as usize;
    let size = le32(blk, off_pos + 4) as usize;
    if off >= MD_BLOCK || size > MD_BLOCK - off {
        return None;
    }
    Some(String::from_utf8_lossy(&blk[off..off + size]).to_string())
}

pub fn unlock(src: Arc<dyn Source>, secrets: &[Secret]) -> UnlockResult {
    let corrupt = |s: &str| UnlockError::Corrupt(s.to_string());
    let h = read_vec(src.as_ref(), 0, 512).map_err(|e| corrupt(&e.to_string()))?;
    if !detect(&h) {
        return Err(corrupt("não é um volume Core Storage"));
    }
    let block_size = le32(&h, 96) as u64;
    let disklbl = le64(&h, 104);
    if block_size == 0 || block_size > 1 << 20 || le32(&h, 168) != 16 {
        return Err(UnlockError::Unsupported(
            "Core Storage com chave de metadados que não é AES-128".into(),
        ));
    }
    let mut md_key = [0u8; 32];
    md_key[..16].copy_from_slice(&h[176..192]);
    md_key[16..].copy_from_slice(&h[304..320]);
    let (k1, k2) = (
        AesAny::new(&md_key[..16]).unwrap(),
        AesAny::new(&md_key[16..]).unwrap(),
    );
    let md_cipher = SectorCipher::Xts {
        k1,
        k2,
        iv: Iv::Plain64,
    };

    let label = read_vec(src.as_ref(), disklbl * block_size, MD_BLOCK)
        .map_err(|e| corrupt(&e.to_string()))?;
    if label.len() < MD_BLOCK {
        return Err(corrupt("rótulo do disco truncado"));
    }
    let vgd_off = le32(&label, 220) as u64;
    let vgd = read_vec(src.as_ref(), disklbl * block_size + vgd_off, 40)
        .map_err(|e| corrupt(&e.to_string()))?;
    if vgd.len() < 40 {
        return Err(corrupt("descritor de grupos truncado"));
    }
    let md_blocks = le64(&vgd, 8);
    let md_start = le64(&vgd, 32) * block_size;

    let (mut iters, mut salt, mut wkek, mut wvk) = (0u32, vec![], vec![], vec![]);
    let (mut lv_size, mut lv_off) = (0u64, 0u64);
    let mut family = [0u8; 16];
    let mut got = 0u8;
    for i in 0..md_blocks.min(64) {
        let mut b = read_vec(src.as_ref(), md_start + i * MD_BLOCK as u64, MD_BLOCK)
            .map_err(|e| corrupt(&e.to_string()))?;
        if b.len() < MD_BLOCK || b.iter().all(|&x| x == 0) {
            break;
        }
        // XTS sobre o bloco inteiro, com o número do bloco como ajuste inicial.
        md_cipher.decrypt_sector(&mut b, i, 0);
        match u16::from_le_bytes([b[10], b[11]]) {
            0x0019 => {
                if let Some(xml) = xml_of(&b, 112) {
                    let pwk = plist_value(&xml, "PassphraseWrappedKEKStruct").and_then(|s| b64(&s));
                    let kw = plist_value(&xml, "KEKWrappedVolumeKeyStruct").and_then(|s| b64(&s));
                    if let (Some(p), Some(k)) = (pwk, kw) {
                        if p.len() >= 172 && k.len() >= 32 {
                            salt = p[8..24].to_vec();
                            wkek = p[32..56].to_vec();
                            iters = le32(&p, 168);
                            wvk = k[8..32].to_vec();
                            got |= 1;
                        }
                    }
                }
            }
            0x001a => {
                if let Some(xml) = xml_of(&b, 128) {
                    if let Some(v) = plist_value(&xml, "com.apple.corestorage.lv.size") {
                        lv_size = u64::from_str_radix(v.trim_start_matches("0x"), 16).unwrap_or(0);
                        got |= 8;
                        if let Some(u) = plist_value(&xml, "com.apple.corestorage.lv.familyUUID") {
                            let hex: String = u.chars().filter(|c| c.is_ascii_hexdigit()).collect();
                            if let Some(b) = super::from_hex(&hex).filter(|b| b.len() == 16) {
                                family.copy_from_slice(&b);
                                got |= 2;
                            }
                        }
                    }
                }
            }
            0x0305 => {
                lv_off = le32(&b, 104) as u64 * block_size;
                got |= 4;
            }
            _ => {}
        }
    }
    if got != 7 | 8 {
        return Err(corrupt("metadados Core Storage incompletos"));
    }

    let finish = |vk16: [u8; 16]| -> UnlockResult {
        let mut full = [0u8; 32];
        full[..16].copy_from_slice(&vk16);
        full[16..].copy_from_slice(
            &Sha256::new()
                .chain_update(vk16)
                .chain_update(family)
                .finalize()[..16],
        );
        let cipher = SectorCipher::Xts {
            k1: AesAny::new(&full[..16]).unwrap(),
            k2: AesAny::new(&full[16..]).unwrap(),
            iv: Iv::Plain64,
        };
        let cs = CryptSource::new(src.clone(), cipher, lv_off, lv_size, 512);
        Ok(Unlocked {
            source: Arc::new(cs),
            description: "FileVault 2 (Core Storage, AES-128-XTS)".into(),
            volume_key: Some(vk16.to_vec()),
        })
    };
    for s in secrets {
        if let Secret::VolumeKey(k) = s {
            if k.len() >= 16 {
                let mut a = [0u8; 16];
                a.copy_from_slice(&k[..16]);
                return finish(a);
            }
        }
    }
    for s in secrets {
        if let Secret::Password(p) = s {
            let mut pk = [0u8; 16];
            pbkdf2::pbkdf2_hmac::<Sha256>(p.as_bytes(), &salt, iters.max(1), &mut pk);
            if let Some(kek) = unwrap_key(&pk, &wkek) {
                if let Some(vk) = unwrap_key(&kek, &wvk) {
                    return finish(vk);
                }
            }
        }
    }
    Err(UnlockError::WrongSecret)
}

pub fn unlock_window(
    base: Arc<dyn Source>,
    start: u64,
    len: u64,
    secrets: &[Secret],
) -> UnlockResult {
    unlock(Arc::new(SubSource::new(base, start, len)), secrets)
}
