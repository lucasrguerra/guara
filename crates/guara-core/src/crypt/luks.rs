//! LUKS1 e LUKS2 (cifras AES; PBKDF2 e Argon2; hashes SHA-1/256/512 e RIPEMD-160).

use super::*;
use crate::source::{read_vec, SubSource};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

pub const MAGIC: &[u8; 6] = b"LUKS\xBA\xBE";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hash {
    Sha1,
    Sha256,
    Sha512,
    Ripemd160,
}

impl Hash {
    pub fn parse(s: &str) -> Option<Hash> {
        Some(match s.to_ascii_lowercase().as_str() {
            "sha1" => Hash::Sha1,
            "sha256" => Hash::Sha256,
            "sha512" => Hash::Sha512,
            "ripemd160" | "rmd160" => Hash::Ripemd160,
            _ => return None,
        })
    }
    pub fn size(self) -> usize {
        match self {
            Hash::Sha1 | Hash::Ripemd160 => 20,
            Hash::Sha256 => 32,
            Hash::Sha512 => 64,
        }
    }
    pub fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut d = D::new();
            for p in parts {
                d.update(p);
            }
            d.finalize().to_vec()
        }
        match self {
            Hash::Sha1 => run::<Sha1>(parts),
            Hash::Sha256 => run::<Sha256>(parts),
            Hash::Sha512 => run::<Sha512>(parts),
            Hash::Ripemd160 => run::<ripemd::Ripemd160>(parts),
        }
    }
    pub fn pbkdf2(self, pass: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let it = iterations.max(1);
        match self {
            Hash::Sha1 => pbkdf2::pbkdf2_hmac::<Sha1>(pass, salt, it, &mut out),
            Hash::Sha256 => pbkdf2::pbkdf2_hmac::<Sha256>(pass, salt, it, &mut out),
            Hash::Sha512 => pbkdf2::pbkdf2_hmac::<Sha512>(pass, salt, it, &mut out),
            Hash::Ripemd160 => pbkdf2::pbkdf2_hmac::<ripemd::Ripemd160>(pass, salt, it, &mut out),
        }
        out
    }
}

/// Difusão do "anti-forensic splitter".
fn diffuse(block: &mut [u8], h: Hash) {
    let ds = h.size();
    let mut i = 0u32;
    let mut off = 0;
    while off < block.len() {
        let n = ds.min(block.len() - off);
        let d = h.digest(&[&i.to_be_bytes(), &block[off..off + n]]);
        block[off..off + n].copy_from_slice(&d[..n]);
        off += n;
        i += 1;
    }
}

/// Recompõe a chave a partir das `stripes` faixas (AFmerge).
pub fn af_merge(src: &[u8], block: usize, stripes: usize, h: Hash) -> Vec<u8> {
    let mut buf = vec![0u8; block];
    for i in 0..stripes.saturating_sub(1) {
        for (b, s) in buf.iter_mut().zip(&src[i * block..(i + 1) * block]) {
            *b ^= *s;
        }
        diffuse(&mut buf, h);
    }
    let last = &src[(stripes - 1) * block..stripes * block];
    buf.iter().zip(last).map(|(a, b)| a ^ b).collect()
}

/// Cifra descrita por texto como "aes-xts-plain64" ou "aes-cbc-essiv:sha256".
pub fn parse_cipher(spec: &str, key: &[u8]) -> std::result::Result<SectorCipher, UnlockError> {
    let (alg, rest) = spec.split_once('-').ok_or_else(|| UnlockError::Unsupported(spec.into()))?;
    if !alg.eq_ignore_ascii_case("aes") {
        return Err(UnlockError::Unsupported(format!("cifra {alg} (apenas AES)")));
    }
    let (mode, ivs) = rest.split_once('-').unwrap_or((rest, "plain"));
    let bad = || UnlockError::Unsupported(format!("chave inválida para {spec}"));
    let mk_iv = |k: &[u8]| -> std::result::Result<Iv, UnlockError> {
        let ivl = ivs.to_ascii_lowercase();
        Ok(match ivl.as_str() {
            "plain64" => Iv::Plain64,
            "plain" | "plain32" => Iv::Plain32,
            "plain64be" => Iv::Plain64Be,
            s if s.starts_with("essiv:") => {
                let h = Hash::parse(&s[6..]).ok_or_else(|| UnlockError::Unsupported(spec.into()))?;
                Iv::Essiv(AesAny::new(&h.digest(&[k])).ok_or_else(bad)?)
            }
            _ => return Err(UnlockError::Unsupported(format!("IV {ivs}"))),
        })
    };
    match mode.to_ascii_lowercase().as_str() {
        "xts" => {
            if key.len() % 2 != 0 {
                return Err(bad());
            }
            let h = key.len() / 2;
            Ok(SectorCipher::Xts {
                k1: AesAny::new(&key[..h]).ok_or_else(bad)?,
                k2: AesAny::new(&key[h..]).ok_or_else(bad)?,
                iv: mk_iv(&key[..h])?,
            })
        }
        "cbc" => Ok(SectorCipher::Cbc {
            key: AesAny::new(key).ok_or_else(bad)?,
            iv: mk_iv(key)?,
        }),
        m => Err(UnlockError::Unsupported(format!("modo {m}"))),
    }
}

fn passwords(secrets: &[Secret]) -> Vec<Vec<u8>> {
    secrets
        .iter()
        .filter_map(|s| match s {
            Secret::Password(p) => Some(p.as_bytes().to_vec()),
            Secret::KeyFile(k) => Some(k.clone()),
            _ => None,
        })
        .collect()
}

fn volume_keys(secrets: &[Secret]) -> Vec<Vec<u8>> {
    secrets
        .iter()
        .filter_map(|s| match s {
            Secret::VolumeKey(k) => Some(k.clone()),
            _ => None,
        })
        .collect()
}

pub fn detect(head: &[u8]) -> bool {
    head.len() >= 8 && &head[..6] == MAGIC
}

pub fn unlock(src: Arc<dyn Source>, secrets: &[Secret]) -> UnlockResult {
    let head = read_vec(src.as_ref(), 0, 8).map_err(|e| UnlockError::Corrupt(e.to_string()))?;
    if !detect(&head) {
        return Err(UnlockError::Corrupt("não é LUKS".into()));
    }
    match u16::from_be_bytes([head[6], head[7]]) {
        1 => unlock_v1(src, secrets),
        2 => unlock_v2(src, secrets),
        v => Err(UnlockError::Unsupported(format!("LUKS versão {v}"))),
    }
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}

fn cstr(b: &[u8]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).to_string()
}

fn unlock_v1(src: Arc<dyn Source>, secrets: &[Secret]) -> UnlockResult {
    let h = read_vec(src.as_ref(), 0, 592).map_err(|e| UnlockError::Corrupt(e.to_string()))?;
    if h.len() < 592 {
        return Err(UnlockError::Corrupt("cabeçalho LUKS1 truncado".into()));
    }
    let cipher_name = cstr(&h[8..40]);
    let cipher_mode = cstr(&h[40..72]);
    let hash = Hash::parse(&cstr(&h[72..104])).ok_or_else(|| UnlockError::Unsupported(format!("hash {}", cstr(&h[72..104]))))?;
    let payload = be32(&h, 104) as u64 * 512;
    let key_bytes = be32(&h, 108) as usize;
    let mk_digest = &h[112..132];
    let mk_salt = &h[132..164];
    let mk_iter = be32(&h, 164);
    let spec = format!("{cipher_name}-{cipher_mode}");
    if key_bytes == 0 || key_bytes > 128 {
        return Err(UnlockError::Corrupt("tamanho de chave inválido".into()));
    }
    let check = |mk: &[u8]| hash.pbkdf2(mk, mk_salt, mk_iter, 20) == mk_digest;
    let finish = |mk: Vec<u8>| -> UnlockResult {
        let cipher = parse_cipher(&spec, &mk)?;
        let len = src.size().saturating_sub(payload);
        let cs = CryptSource::new(src.clone(), cipher, payload, len, 512);
        Ok(Unlocked {
            source: Arc::new(cs),
            description: format!("LUKS1 ({spec})"),
            volume_key: Some(mk),
        })
    };
    for vk in volume_keys(secrets) {
        if vk.len() == key_bytes && check(&vk) {
            return finish(vk);
        }
    }
    let pws = passwords(secrets);
    for slot in 0..8 {
        let o = 208 + slot * 48;
        if be32(&h, o) != 0x00AC_71F3 {
            continue;
        }
        let iter = be32(&h, o + 4);
        let salt = &h[o + 8..o + 40];
        let km_off = be32(&h, o + 40) as u64 * 512;
        let stripes = be32(&h, o + 44) as usize;
        if stripes == 0 || stripes > 100_000 {
            continue;
        }
        let km_len = (key_bytes * stripes).div_ceil(512) * 512;
        let Ok(mut km) = read_vec(src.as_ref(), km_off, km_len) else { continue };
        if km.len() < km_len {
            continue;
        }
        for pw in &pws {
            let dk = hash.pbkdf2(pw, salt, iter, key_bytes);
            let cipher = parse_cipher(&spec, &dk)?;
            let mut m = km.clone();
            for (i, s) in m.chunks_exact_mut(512).enumerate() {
                cipher.decrypt_sector(s, i as u64, i as u64 * 512);
            }
            let mk = af_merge(&m, key_bytes, stripes, hash);
            if check(&mk) {
                return finish(mk);
            }
        }
        km.clear();
    }
    Err(UnlockError::WrongSecret)
}

fn b64(s: &str) -> Option<Vec<u8>> {
    let mut out = vec![];
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' => continue,
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

fn jstr<'a>(v: &'a serde_json::Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str()
}

/// Números no JSON do LUKS2 vêm como texto (inteiros grandes) ou número.
fn jnum(v: &serde_json::Value, k: &str) -> Option<u64> {
    let x = v.get(k)?;
    x.as_u64().or_else(|| x.as_str()?.parse().ok())
}

fn unlock_v2(src: Arc<dyn Source>, secrets: &[Secret]) -> UnlockResult {
    let h = read_vec(src.as_ref(), 0, 4096).map_err(|e| UnlockError::Corrupt(e.to_string()))?;
    if h.len() < 4096 {
        return Err(UnlockError::Corrupt("cabeçalho LUKS2 truncado".into()));
    }
    let hdr_size = u64::from_be_bytes(h[8..16].try_into().unwrap());
    if !(16384..=(4 << 20)).contains(&hdr_size) {
        return Err(UnlockError::Corrupt("tamanho de cabeçalho LUKS2 inválido".into()));
    }
    let area = read_vec(src.as_ref(), 4096, (hdr_size - 4096) as usize).map_err(|e| UnlockError::Corrupt(e.to_string()))?;
    let end = area.iter().position(|&c| c == 0).unwrap_or(area.len());
    let json: serde_json::Value = serde_json::from_slice(&area[..end]).map_err(|e| UnlockError::Corrupt(format!("JSON: {e}")))?;
    let segments = json.get("segments").and_then(|s| s.as_object()).ok_or_else(|| UnlockError::Corrupt("sem segments".into()))?;
    let (_, seg) = segments
        .iter()
        .find(|(_, s)| jstr(s, "type") == Some("crypt"))
        .ok_or_else(|| UnlockError::Unsupported("sem segmento cifrado".into()))?;
    let seg_off = jnum(seg, "offset").ok_or_else(|| UnlockError::Corrupt("offset do segmento".into()))?;
    let seg_len = match seg.get("size").and_then(|s| s.as_str()) {
        Some("dynamic") | None => src.size().saturating_sub(seg_off),
        Some(n) => n.parse().unwrap_or(0),
    };
    let seg_enc = jstr(seg, "encryption").unwrap_or("aes-xts-plain64").to_string();
    let sector = jnum(seg, "sector_size").unwrap_or(512);
    let iv_tweak = jnum(seg, "iv_tweak").unwrap_or(0);
    let digests = json.get("digests").and_then(|d| d.as_object()).ok_or_else(|| UnlockError::Corrupt("sem digests".into()))?;

    let check = |vk: &[u8]| -> bool {
        for (_, d) in digests {
            if jstr(d, "type") != Some("pbkdf2") {
                continue;
            }
            let (Some(hash), Some(salt), Some(want), Some(it)) = (
                jstr(d, "hash").and_then(Hash::parse),
                jstr(d, "salt").and_then(b64),
                jstr(d, "digest").and_then(b64),
                jnum(d, "iterations"),
            ) else {
                continue;
            };
            if hash.pbkdf2(vk, &salt, it as u32, want.len()) == want {
                return true;
            }
        }
        false
    };
    let finish = |vk: Vec<u8>| -> UnlockResult {
        let cipher = parse_cipher(&seg_enc, &vk)?;
        let cs = CryptSource::new(src.clone(), cipher, seg_off, seg_len, sector).with_iv_tweak(iv_tweak);
        Ok(Unlocked {
            source: Arc::new(cs),
            description: format!("LUKS2 ({seg_enc})"),
            volume_key: Some(vk),
        })
    };
    for vk in volume_keys(secrets) {
        if check(&vk) {
            return finish(vk);
        }
    }
    let pws = passwords(secrets);
    let slots = json.get("keyslots").and_then(|d| d.as_object()).ok_or_else(|| UnlockError::Corrupt("sem keyslots".into()))?;
    let mut unsupported: Option<String> = None;
    for (_, slot) in slots {
        if jstr(slot, "type") != Some("luks2") {
            continue;
        }
        let key_size = jnum(slot, "key_size").unwrap_or(0) as usize;
        let af = slot.get("af");
        let stripes = af.and_then(|a| jnum(a, "stripes")).unwrap_or(4000) as usize;
        let af_hash = af.and_then(|a| jstr(a, "hash")).and_then(Hash::parse).unwrap_or(Hash::Sha256);
        let ar = slot.get("area");
        let (Some(a_off), Some(a_size)) = (ar.and_then(|a| jnum(a, "offset")), ar.and_then(|a| jnum(a, "size"))) else { continue };
        let a_enc = ar.and_then(|a| jstr(a, "encryption")).unwrap_or("aes-xts-plain64");
        let a_ksize = ar.and_then(|a| jnum(a, "key_size")).unwrap_or(64) as usize;
        let kdf = slot.get("kdf");
        let Some(kdf) = kdf else { continue };
        let salt = jstr(kdf, "salt").and_then(b64).unwrap_or_default();
        let km_len = (key_size * stripes).div_ceil(512) * 512;
        if key_size == 0 || stripes == 0 || km_len as u64 > a_size {
            continue;
        }
        let Ok(km) = read_vec(src.as_ref(), a_off, km_len) else { continue };
        if km.len() < km_len {
            continue;
        }
        for pw in &pws {
            let dk: Vec<u8> = match jstr(kdf, "type") {
                Some("pbkdf2") => {
                    let hh = jstr(kdf, "hash").and_then(Hash::parse).unwrap_or(Hash::Sha256);
                    hh.pbkdf2(pw, &salt, jnum(kdf, "iterations").unwrap_or(1000) as u32, a_ksize)
                }
                Some(t @ ("argon2i" | "argon2id")) => {
                    let alg = if t == "argon2i" { argon2::Algorithm::Argon2i } else { argon2::Algorithm::Argon2id };
                    let params = match argon2::Params::new(
                        jnum(kdf, "memory").unwrap_or(65536) as u32,
                        jnum(kdf, "time").unwrap_or(4) as u32,
                        jnum(kdf, "cpus").unwrap_or(1) as u32,
                        Some(a_ksize),
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            unsupported = Some(format!("parâmetros Argon2: {e}"));
                            continue;
                        }
                    };
                    let mut out = vec![0u8; a_ksize];
                    if argon2::Argon2::new(alg, argon2::Version::V0x13, params).hash_password_into(pw, &salt, &mut out).is_err() {
                        continue;
                    }
                    out
                }
                Some(t) => {
                    unsupported = Some(format!("KDF {t}"));
                    continue;
                }
                None => continue,
            };
            let cipher = parse_cipher(a_enc, &dk)?;
            let mut m = km.clone();
            for (i, s) in m.chunks_exact_mut(512).enumerate() {
                cipher.decrypt_sector(s, i as u64, i as u64 * 512);
            }
            let vk = af_merge(&m, key_size, stripes, af_hash);
            if check(&vk) {
                return finish(vk);
            }
        }
    }
    match unsupported {
        Some(u) => Err(UnlockError::Unsupported(u)),
        None => Err(UnlockError::WrongSecret),
    }
}

/// Atalho: abre uma janela da origem como LUKS.
pub fn unlock_window(base: Arc<dyn Source>, start: u64, len: u64, secrets: &[Secret]) -> UnlockResult {
    unlock(Arc::new(SubSource::new(base, start, len)), secrets)
}
