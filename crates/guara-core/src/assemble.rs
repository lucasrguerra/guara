//! Monta o disco virtual a partir do que o usuário informou: membros extras (RAID/LVM) e
//! senhas ou chaves para volumes cifrados. É a única porta de entrada para varrer e recuperar
//! discos que não são "simples".

use crate::crypt::{self, Secret, UnlockError};
use crate::error::{Error, Result};
use crate::partition;
use crate::source::{open_path, Source, SubSource};
use crate::types::{FsKind, PartInfo};
use crate::volume::{Overlay, Region};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SecretInput {
    Password { value: String },
    Recovery { value: String },
    /// Caminho de um arquivo de chave (.BEK do BitLocker, keyfile do LUKS).
    Keyfile { path: String },
    /// Chave do volume em hexadecimal.
    Key { hex: String },
}

/// Chave de volume guardada na sessão (só quando o usuário pede para lembrar).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedKey {
    pub start: u64,
    pub kind: String,
    pub key_hex: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Assembly {
    /// Outros discos ou imagens do mesmo conjunto (RAID/LVM), além do alvo principal.
    #[serde(default)]
    pub members: Vec<String>,
    /// Senhas e chaves: nunca vão para o disco junto com a sessão.
    #[serde(default, skip_serializing)]
    pub secrets: Vec<SecretInput>,
    /// Guarda as chaves de volume derivadas na sessão, para reabrir sem pedir a senha de novo.
    #[serde(default)]
    pub remember_keys: bool,
    #[serde(default)]
    pub keys: Vec<SavedKey>,
}

impl Assembly {
    pub fn is_plain(&self) -> bool {
        self.members.is_empty() && self.secrets.is_empty() && self.keys.is_empty()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UnlockInfo {
    pub start: u64,
    pub len: u64,
    pub kind: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LockedInfo {
    pub start: u64,
    pub kind: String,
    pub reason: String,
}

pub struct Assembled {
    pub source: Arc<dyn Source>,
    pub unlocked: Vec<UnlockInfo>,
    pub locked: Vec<LockedInfo>,
    /// Chaves derivadas, para guardar na sessão se o usuário permitir.
    pub keys: Vec<SavedKey>,
    /// Volumes lógicos / partições extras que não estão na tabela do disco.
    pub extra_parts: Vec<PartInfo>,
    pub notes: Vec<String>,
}

fn to_secrets(a: &Assembly) -> Result<Vec<Secret>> {
    let mut v = vec![];
    for s in &a.secrets {
        v.push(match s {
            SecretInput::Password { value } => Secret::Password(value.clone()),
            SecretInput::Recovery { value } => Secret::RecoveryKey(value.clone()),
            SecretInput::Keyfile { path } => Secret::KeyFile(
                std::fs::read(path).map_err(|e| Error::msg(format!("não foi possível ler o arquivo de chave {path}: {e}")))?,
            ),
            SecretInput::Key { hex } => Secret::VolumeKey(
                crypt::from_hex(hex).ok_or_else(|| Error::msg("chave do volume em hexadecimal inválida"))?,
            ),
        });
    }
    Ok(v)
}

fn kind_name(fs: FsKind) -> &'static str {
    match fs {
        FsKind::Bitlocker => "bitlocker",
        FsKind::Luks => "luks",
        FsKind::Corestorage => "filevault",
        _ => "?",
    }
}

fn try_unlock(window: Arc<dyn Source>, fs: FsKind, secrets: &[Secret]) -> crypt::UnlockResult {
    match fs {
        FsKind::Bitlocker => crypt::bitlocker::unlock(window, secrets),
        FsKind::Luks => crypt::luks::unlock(window, secrets),
        FsKind::Corestorage => crypt::filevault::unlock(window, secrets),
        _ => Err(UnlockError::Unsupported("tipo".into())),
    }
}

/// Abre `path` e aplica a montagem pedida.
pub fn assemble(path: &str, asm: &Assembly) -> Result<Assembled> {
    let base = open_path(Path::new(path)).map_err(|e| Error::msg(format!("{path}: {e}")))?;
    assemble_source(base, asm)
}

pub fn assemble_source(base: Arc<dyn Source>, asm: &Assembly) -> Result<Assembled> {
    let secrets = to_secrets(asm)?;
    let mut regions: Vec<Region> = vec![];
    let mut out = Assembled {
        source: base.clone(),
        unlocked: vec![],
        locked: vec![],
        keys: vec![],
        extra_parts: vec![],
        notes: vec![],
    };
    // Até três rodadas: o que foi aberto pode conter outro volume cifrado.
    for _ in 0..3 {
        let current: Arc<dyn Source> = Arc::new(Overlay::new(base.clone(), regions.clone()));
        let layout = partition::analyze(current.as_ref(), false);
        let mut progress = false;
        out.locked.clear();
        for p in &layout.partitions {
            if p.encrypted.is_none() || regions.iter().any(|r| r.start == p.start) {
                continue;
            }
            let len = if p.len == 0 { current.size().saturating_sub(p.start) } else { p.len };
            let window: Arc<dyn Source> = Arc::new(SubSource::new(current.clone(), p.start, len));
            let mut all = secrets.clone();
            for k in asm.keys.iter().filter(|k| k.start == p.start) {
                if let Some(b) = crypt::from_hex(&k.key_hex) {
                    all.push(Secret::VolumeKey(b));
                }
            }
            match try_unlock(window, p.fs, &all) {
                Ok(u) => {
                    let ulen = u.source.size();
                    out.unlocked.push(UnlockInfo {
                        start: p.start,
                        len: ulen,
                        kind: kind_name(p.fs).into(),
                        description: u.description.clone(),
                    });
                    if let Some(k) = &u.volume_key {
                        out.keys.push(SavedKey { start: p.start, kind: kind_name(p.fs).into(), key_hex: crypt::to_hex(k) });
                    }
                    regions.push(Region { start: p.start, len: ulen, src: u.source, label: u.description });
                    progress = true;
                }
                Err(e) => out.locked.push(LockedInfo {
                    start: p.start,
                    kind: kind_name(p.fs).into(),
                    reason: match e {
                        UnlockError::WrongSecret if secrets.is_empty() && asm.keys.is_empty() => "volume cifrado: informe a senha, a chave de recuperação ou o arquivo de chave".into(),
                        other => other.to_string(),
                    },
                }),
            }
        }
        if !progress {
            break;
        }
    }
    if !regions.is_empty() {
        out.source = Arc::new(Overlay::new(base, regions));
    }
    Ok(out)
}
