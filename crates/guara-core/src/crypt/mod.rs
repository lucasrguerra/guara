//! Cifras de disco (somente leitura) e desbloqueio de volumes cifrados.
//!
//! O volume desbloqueado é exposto como uma `Source` comum: o restante do Guará
//! (varredura, recuperação) enxerga o sistema de arquivos em claro, sem saber
//! que existe cifra por baixo.

pub mod bitlocker;
pub mod filevault;
pub mod luks;

use crate::source::Source;
use aes::cipher::{Block, BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use aes::{Aes128, Aes192, Aes256};
use std::io;
use std::sync::Arc;

/// AES com tamanho de chave escolhido em tempo de execução.
#[derive(Clone)]
pub enum AesAny {
    A128(Aes128),
    A192(Aes192),
    A256(Aes256),
}

impl AesAny {
    pub fn new(key: &[u8]) -> Option<Self> {
        Some(match key.len() {
            16 => AesAny::A128(Aes128::new_from_slice(key).ok()?),
            24 => AesAny::A192(Aes192::new_from_slice(key).ok()?),
            32 => AesAny::A256(Aes256::new_from_slice(key).ok()?),
            _ => return None,
        })
    }
    pub fn encrypt(&self, b: &mut [u8; 16]) {
        let g: &mut Block<Aes128> = (&mut b[..]).try_into().expect("bloco AES de 16 bytes");
        match self {
            AesAny::A128(c) => c.encrypt_block(g),
            AesAny::A192(c) => c.encrypt_block(g),
            AesAny::A256(c) => c.encrypt_block(g),
        }
    }
    pub fn decrypt(&self, b: &mut [u8; 16]) {
        let g: &mut Block<Aes128> = (&mut b[..]).try_into().expect("bloco AES de 16 bytes");
        match self {
            AesAny::A128(c) => c.decrypt_block(g),
            AesAny::A192(c) => c.decrypt_block(g),
            AesAny::A256(c) => c.decrypt_block(g),
        }
    }
}

fn xor16(a: &mut [u8; 16], b: &[u8]) {
    for i in 0..16 {
        a[i] ^= b[i];
    }
}

/// Como o vetor de inicialização de cada setor é calculado.
#[derive(Clone)]
pub enum Iv {
    /// Número do setor (de 512 bytes) em 64 bits little-endian.
    Plain64,
    /// Número do setor em 32 bits little-endian.
    Plain32,
    Plain64Be,
    /// ESSIV: AES(SHA-256(chave), setor).
    Essiv(AesAny),
    /// Número da unidade de dados (setor de `usize` bytes) em 64 bits little-endian.
    Unit(u64),
    /// BitLocker (Windows 10 1511+): AES(chave, offset em bytes).
    Eboiv(AesAny),
    /// BitLocker CBC com difusor Elephant: IV = AES(chave, offset em bytes).
    Bitlocker(AesAny),
}

impl Iv {
    fn make(&self, sector_512: u64, byte_off: u64) -> [u8; 16] {
        let mut iv = [0u8; 16];
        match self {
            Iv::Plain64 => iv[..8].copy_from_slice(&sector_512.to_le_bytes()),
            Iv::Plain32 => iv[..4].copy_from_slice(&(sector_512 as u32).to_le_bytes()),
            Iv::Plain64Be => iv[8..].copy_from_slice(&sector_512.to_be_bytes()),
            Iv::Unit(u) => iv[..8].copy_from_slice(&(byte_off / u).to_le_bytes()),
            Iv::Essiv(k) => {
                iv[..8].copy_from_slice(&sector_512.to_le_bytes());
                k.encrypt(&mut iv);
            }
            Iv::Eboiv(k) | Iv::Bitlocker(k) => {
                iv[..8].copy_from_slice(&byte_off.to_le_bytes());
                k.encrypt(&mut iv);
            }
        }
        iv
    }
}

/// Cifra aplicada setor a setor.
#[derive(Clone)]
pub enum SectorCipher {
    Xts {
        k1: AesAny,
        k2: AesAny,
        iv: Iv,
    },
    Cbc {
        key: AesAny,
        iv: Iv,
    },
    /// BitLocker AES-CBC com difusor Elephant (chave de dados + chave de ajuste).
    Elephant {
        key: AesAny,
        tweak: AesAny,
    },
}

fn gf_double(t: &mut [u8; 16]) {
    let mut carry = 0u8;
    for b in t.iter_mut() {
        let next = *b >> 7;
        *b = (*b << 1) | carry;
        carry = next;
    }
    if carry != 0 {
        t[0] ^= 0x87;
    }
}

impl SectorCipher {
    /// Decifra um setor no lugar. `sector_512` conta setores de 512 bytes; `byte_off` é a posição
    /// em bytes relativa ao início dos dados cifrados.
    pub fn decrypt_sector(&self, buf: &mut [u8], sector_512: u64, byte_off: u64) {
        match self {
            SectorCipher::Xts { k1, k2, iv } => {
                let mut t = iv.make(sector_512, byte_off);
                k2.encrypt(&mut t);
                for ch in buf.chunks_exact_mut(16) {
                    let mut b = [0u8; 16];
                    b.copy_from_slice(ch);
                    xor16(&mut b, &t);
                    k1.decrypt(&mut b);
                    xor16(&mut b, &t);
                    ch.copy_from_slice(&b);
                    gf_double(&mut t);
                }
            }
            SectorCipher::Cbc { key, iv } => {
                cbc_decrypt(key, &iv.make(sector_512, byte_off), buf);
            }
            SectorCipher::Elephant { key, tweak } => {
                bitlocker::elephant_decrypt(key, tweak, buf, byte_off);
            }
        }
    }
}

pub fn cbc_decrypt(key: &AesAny, iv: &[u8; 16], buf: &mut [u8]) {
    let mut prev = *iv;
    for ch in buf.chunks_exact_mut(16) {
        let mut c = [0u8; 16];
        c.copy_from_slice(ch);
        let mut b = c;
        key.decrypt(&mut b);
        xor16(&mut b, &prev);
        ch.copy_from_slice(&b);
        prev = c;
    }
}

pub fn cbc_encrypt(key: &AesAny, iv: &[u8; 16], buf: &mut [u8]) {
    let mut prev = *iv;
    for ch in buf.chunks_exact_mut(16) {
        let mut b = [0u8; 16];
        b.copy_from_slice(ch);
        xor16(&mut b, &prev);
        key.encrypt(&mut b);
        ch.copy_from_slice(&b);
        prev = b;
    }
}

/// Volume cifrado visto em claro: `len` bytes que começam em `data_offset` na origem.
pub struct CryptSource {
    inner: Arc<dyn Source>,
    cipher: SectorCipher,
    data_offset: u64,
    len: u64,
    sector: u64,
    /// Setores antes deste limite estão em claro (BitLocker parcialmente cifrado) ou
    /// são cobertos por uma cópia (os primeiros setores do BitLocker).
    patches: Vec<Patch>,
    /// A partir deste byte (relativo aos dados) o conteúdo ainda não foi cifrado.
    plain_from: u64,
    /// Somado ao número do setor no cálculo do IV (iv_tweak do LUKS2).
    iv_tweak: u64,
}

/// Trecho do volume lido de outro lugar da origem (BitLocker move o início do volume).
#[derive(Clone)]
pub struct Patch {
    /// Início no volume em claro.
    pub at: u64,
    pub len: u64,
    /// Onde estão os dados, na origem, já em claro.
    pub from: u64,
    /// Se os bytes de origem precisam ser decifrados (usando a posição `from_crypt`).
    pub decrypt_from: Option<u64>,
    /// Devolve zeros (áreas de metadados do BitLocker).
    pub zero: bool,
}

impl CryptSource {
    pub fn new(
        inner: Arc<dyn Source>,
        cipher: SectorCipher,
        data_offset: u64,
        len: u64,
        sector: u64,
    ) -> Self {
        let sector = sector.max(512);
        Self {
            inner,
            cipher,
            data_offset,
            len: len / sector * sector,
            sector,
            patches: vec![],
            plain_from: u64::MAX,
            iv_tweak: 0,
        }
    }
    pub fn with_patches(mut self, p: Vec<Patch>) -> Self {
        self.patches = p;
        self
    }
    pub fn with_iv_tweak(mut self, t: u64) -> Self {
        self.iv_tweak = t;
        self
    }
    pub fn with_plain_from(mut self, off: u64) -> Self {
        self.plain_from = off;
        self
    }

    fn read_sector_plain(&self, rel: u64, out: &mut [u8]) -> io::Result<()> {
        // `rel` alinhado ao setor; `out.len() == sector`.
        for p in &self.patches {
            if rel >= p.at && rel < p.at + p.len {
                if p.zero {
                    out.fill(0);
                    return Ok(());
                }
                let src_off = p.from + (rel - p.at);
                crate::source::read_exact_at(self.inner.as_ref(), self.data_offset + src_off, out)?;
                if let Some(pos) = p.decrypt_from {
                    let r = pos + (rel - p.at);
                    self.cipher.decrypt_sector(out, r / 512, r);
                }
                return Ok(());
            }
        }
        crate::source::read_exact_at(self.inner.as_ref(), self.data_offset + rel, out)?;
        if rel < self.plain_from {
            self.cipher
                .decrypt_sector(out, rel / 512 + self.iv_tweak, rel);
        }
        Ok(())
    }
}

impl Source for CryptSource {
    fn size(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((self.len - offset) as usize);
        let s = self.sector;
        let mut done = 0usize;
        let mut tmp = vec![0u8; s as usize];
        while done < want {
            let pos = offset + done as u64;
            let rel = pos / s * s;
            let within = (pos - rel) as usize;
            let n = (s as usize - within).min(want - done);
            self.read_sector_plain(rel, &mut tmp)?;
            buf[done..done + n].copy_from_slice(&tmp[within..within + n]);
            done += n;
        }
        Ok(done)
    }
}

/// Algo que o usuário fornece para abrir um volume.
#[derive(Clone)]
pub enum Secret {
    Password(String),
    /// Chave de recuperação numérica do BitLocker (8 grupos de 6 dígitos).
    RecoveryKey(String),
    /// Conteúdo de um arquivo de chave (.BEK do BitLocker, keyfile do LUKS).
    KeyFile(Vec<u8>),
    /// Chave do volume já conhecida (FVEK/master key).
    VolumeKey(Vec<u8>),
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Secret::Password(_) => "Secret::Password(..)",
            Secret::RecoveryKey(_) => "Secret::RecoveryKey(..)",
            Secret::KeyFile(_) => "Secret::KeyFile(..)",
            Secret::VolumeKey(_) => "Secret::VolumeKey(..)",
        })
    }
}

#[derive(Debug)]
pub enum UnlockError {
    /// Nenhuma das senhas/chaves abriu o volume.
    WrongSecret,
    Unsupported(String),
    Corrupt(String),
}

impl std::fmt::Display for UnlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnlockError::WrongSecret => write!(f, "senha ou chave incorreta"),
            UnlockError::Unsupported(s) => write!(f, "não suportado: {s}"),
            UnlockError::Corrupt(s) => write!(f, "cabeçalho inválido: {s}"),
        }
    }
}

impl From<UnlockError> for crate::Error {
    fn from(e: UnlockError) -> Self {
        crate::Error::msg(e.to_string())
    }
}

/// Volume aberto.
pub struct Unlocked {
    pub source: Arc<dyn Source>,
    pub description: String,
    /// Chave do volume (para reabrir sem a senha, se o usuário permitir guardá-la).
    pub volume_key: Option<Vec<u8>>,
}

pub type UnlockResult = std::result::Result<Unlocked, UnlockError>;

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}
