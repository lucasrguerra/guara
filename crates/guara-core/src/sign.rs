//! Assinatura digital (Ed25519) de manifestos, relatórios e qualquer outro arquivo, e conferência
//! de uma recuperação inteira.
//!
//! A assinatura fica ao lado do arquivo, em `<arquivo>.assinatura.json`, e cobre o SHA-256 do
//! conteúdo, o tamanho, o nome, a hora e o assinante. Quem confere recalcula o hash do arquivo,
//! refaz a mensagem a partir dos campos do JSON e valida com a chave pública que vem nele. Isso
//! prova que o arquivo não mudou desde que foi assinado **por quem tem a chave**; para saber de
//! quem é a chave, compare a impressão digital (`fingerprint`) com a que o perito publicou ou
//! registrou fora da máquina, passando-a em `trusted_fingerprint`.
//!
//! A hora é a do relógio da máquina de quem assinou: não é um carimbo de tempo de terceiros
//! (RFC 3161) e, sozinha, não prova *quando* a assinatura foi feita.
//!
//! A chave privada fica em `assinatura.key` na pasta de configuração (ou onde
//! `GUARA_SIGNING_KEY` apontar), em arquivo de texto com permissão 0600 nos sistemas Unix. Quem
//! precisa de mais proteção deve guardá-la num pendrive ou volume cifrado e apontar para lá.

use crate::error::{Error, Result};
use ed25519_compact::{KeyPair, Noise, PublicKey, Seed, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

const FORMAT: &str = "guara-assinatura-v1";
const KEY_HEADER: &str = "guara-ed25519-seed-v1";
const PUB_HEADER: &str = "guara-ed25519-public-v1";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Impressão digital da chave pública: os 10 primeiros bytes do SHA-256, em grupos de 4 hex.
pub fn fingerprint(public_key: &[u8]) -> String {
    let h = Sha256::digest(public_key);
    let h = hex(&h[..10]);
    h.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join(":")
}

/// Compara impressões digitais ignorando maiúsculas, `:` e espaços.
fn same_fingerprint(a: &str, b: &str) -> bool {
    let n = |s: &str| {
        s.chars()
            .filter(|c| c.is_ascii_hexdigit())
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>()
    };
    let (a, b) = (n(a), n(b));
    !a.is_empty() && a == b
}

/// Par de chaves de assinatura.
pub struct SigningKey {
    kp: KeyPair,
}

impl SigningKey {
    pub fn generate() -> Self {
        Self {
            kp: KeyPair::from_seed(Seed::generate()),
        }
    }

    pub fn public_bytes(&self) -> [u8; 32] {
        *self.kp.pk
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public_bytes())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            Error::msg(format!(
                "não foi possível ler a chave {}: {e}",
                path.display()
            ))
        })?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        if lines.next() != Some(KEY_HEADER) {
            return Err(Error::msg(format!(
                "{} não é uma chave de assinatura do Guará",
                path.display()
            )));
        }
        let seed = lines
            .next()
            .and_then(unhex)
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .ok_or_else(|| Error::msg("chave de assinatura corrompida"))?;
        Ok(Self {
            kp: KeyPair::from_seed(Seed::new(seed)),
        })
    }

    /// Grava a chave privada (0600 no Unix) e, ao lado, a pública (`.pub`). Recusa sobrescrever.
    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        if let Some(dir) = path.parent() {
            crate::owner::create_dir_all_owned(dir)?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::msg(format!(
                    "já existe uma chave em {}; apague-a de propósito se quiser trocar (as assinaturas antigas continuam válidas com a chave pública antiga)",
                    path.display()
                ))
            } else {
                e.into()
            }
        })?;
        f.write_all(format!("{KEY_HEADER}\n{}\n", hex(self.kp.sk.seed().as_ref())).as_bytes())?;
        drop(f);
        crate::owner::give_to_user(path);
        let pubp = pub_path(path);
        crate::owner::write_owned(
            &pubp,
            format!(
                "{PUB_HEADER}\n{}\n# impressão digital: {}\n",
                hex(&self.public_bytes()),
                self.fingerprint()
            ),
        )?;
        Ok(())
    }
}

fn pub_path(key: &Path) -> PathBuf {
    key.with_extension("pub")
}

/// `GUARA_SIGNING_KEY` ou `assinatura.key` na pasta de configuração.
pub fn default_key_path() -> PathBuf {
    match std::env::var("GUARA_SIGNING_KEY") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => crate::settings::config_dir().join("assinatura.key"),
    }
}

/// Arquivo que acompanha `path` com a assinatura.
pub fn signature_path(path: &Path) -> PathBuf {
    let mut n = path.as_os_str().to_os_string();
    n.push(".assinatura.json");
    PathBuf::from(n)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureFile {
    pub formato: String,
    pub arquivo: String,
    pub tamanho: u64,
    pub sha256: String,
    pub assinado_em: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assinante: Option<String>,
    pub chave_publica: String,
    pub impressao_digital: String,
    pub assinatura: String,
}

fn message(f: &SignatureFile) -> Vec<u8> {
    format!(
        "{}\narquivo={}\ntamanho={}\nsha256={}\nassinado_em={}\nassinante={}\n",
        f.formato,
        f.arquivo,
        f.tamanho,
        f.sha256,
        f.assinado_em,
        f.assinante.as_deref().unwrap_or("")
    )
    .into_bytes()
}

/// SHA-256 e tamanho de um arquivo, lido em blocos.
pub fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex(&h.finalize()), total))
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    crate::audit::utc_iso(secs)
}

/// Assina `path` e grava `<path>.assinatura.json`. Devolve o caminho da assinatura.
pub fn sign_file(key: &SigningKey, path: &Path, signer: Option<&str>) -> Result<PathBuf> {
    let (sha256, size) = hash_file(path)?;
    let mut sig = SignatureFile {
        formato: FORMAT.into(),
        arquivo: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        tamanho: size,
        sha256,
        assinado_em: now_iso(),
        assinante: signer.filter(|s| !s.is_empty()).map(String::from),
        chave_publica: hex(&key.public_bytes()),
        impressao_digital: key.fingerprint(),
        assinatura: String::new(),
    };
    sig.assinatura = hex(key
        .kp
        .sk
        .sign(message(&sig), Some(Noise::generate()))
        .as_ref());
    let out = signature_path(path);
    crate::owner::write_owned(&out, serde_json::to_vec_pretty(&sig)?)?;
    Ok(out)
}

/// Resultado de conferir uma assinatura.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// O arquivo é o mesmo e a assinatura vale para a chave que vem nela.
    pub valid: bool,
    pub file_matches: bool,
    pub signature_valid: bool,
    pub fingerprint: String,
    pub signed_at: String,
    pub signer: Option<String>,
    /// `Some(true/false)` quando uma impressão digital de confiança foi informada.
    pub trusted: Option<bool>,
    pub problems: Vec<String>,
}

pub fn verify_file(
    path: &Path,
    sig_path: Option<&Path>,
    trusted_fingerprint: Option<&str>,
) -> Result<Check> {
    let sp = sig_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| signature_path(path));
    let raw = std::fs::read(&sp).map_err(|e| {
        Error::msg(format!(
            "não foi possível ler a assinatura {}: {e}",
            sp.display()
        ))
    })?;
    let sig: SignatureFile = serde_json::from_slice(&raw)
        .map_err(|e| Error::msg(format!("assinatura ilegível: {e}")))?;
    let mut problems = vec![];
    if sig.formato != FORMAT {
        problems.push(format!(
            "formato de assinatura desconhecido ({})",
            sig.formato
        ));
    }
    let (sha, size) = hash_file(path)?;
    let file_matches = sha == sig.sha256 && size == sig.tamanho;
    if !file_matches {
        problems.push("o arquivo mudou depois de assinado (hash ou tamanho diferem)".into());
    }
    let pk = unhex(&sig.chave_publica).and_then(|b| PublicKey::from_slice(&b).ok());
    let sg = unhex(&sig.assinatura).and_then(|b| Signature::from_slice(&b).ok());
    let signature_valid = match (&pk, &sg) {
        (Some(pk), Some(sg)) => pk.verify(message(&sig), sg).is_ok(),
        _ => false,
    };
    if !signature_valid {
        problems.push("a assinatura não confere (campos alterados ou chave errada)".into());
    }
    let real_fp = pk
        .as_ref()
        .map(|p| fingerprint(p.as_ref()))
        .unwrap_or_default();
    if real_fp != sig.impressao_digital {
        problems.push("a impressão digital declarada não é a da chave pública".into());
    }
    let trusted = trusted_fingerprint.map(|t| same_fingerprint(t, &real_fp));
    if trusted == Some(false) {
        problems.push(format!(
            "a chave que assinou ({real_fp}) não é a esperada ({})",
            trusted_fingerprint.unwrap_or_default()
        ));
    }
    Ok(Check {
        valid: problems.is_empty(),
        file_matches,
        signature_valid,
        fingerprint: real_fp,
        signed_at: sig.assinado_em,
        signer: sig.assinante,
        trusted,
        problems,
    })
}

/// Confere uma pasta de recuperação: a assinatura do manifesto (se houver) e o hash de cada
/// arquivo contra o que o manifesto registrou.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryCheck {
    pub manifest_signature: Option<Check>,
    pub files: usize,
    pub ok: usize,
    pub missing: Vec<String>,
    pub changed: Vec<String>,
    /// `true` quando tudo confere (e, havendo assinatura, ela vale).
    pub valid: bool,
}

pub fn verify_recovery(dir: &Path, trusted_fingerprint: Option<&str>) -> Result<RecoveryCheck> {
    let manifest = dir.join("guara-manifesto.json");
    let raw = std::fs::read(&manifest)
        .map_err(|e| Error::msg(format!("não foi possível ler {}: {e}", manifest.display())))?;
    let v: serde_json::Value = serde_json::from_slice(&raw)?;
    let sigp = signature_path(&manifest);
    let manifest_signature = if sigp.is_file() {
        Some(verify_file(&manifest, None, trusted_fingerprint)?)
    } else {
        None
    };
    let list = v["relatorio"]["recovered"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let (mut ok, mut missing, mut changed) = (0, vec![], vec![]);
    for f in &list {
        let (Some(rel), Some(want)) = (f["path"].as_str(), f["sha256"].as_str()) else {
            continue;
        };
        // o manifesto é dado de entrada: não deixa sair da pasta
        if rel.split('/').any(|c| c == "..") || Path::new(rel).is_absolute() {
            changed.push(format!("{rel} (caminho fora da pasta)"));
            continue;
        }
        let p = dir.join(rel);
        match hash_file(&p) {
            Ok((h, _)) if h == want => ok += 1,
            Ok(_) => changed.push(rel.to_string()),
            Err(_) => missing.push(rel.to_string()),
        }
    }
    let signature_ok = match &manifest_signature {
        Some(c) => c.valid,
        None => true,
    };
    let valid = missing.is_empty() && changed.is_empty() && signature_ok;
    Ok(RecoveryCheck {
        manifest_signature,
        files: list.len(),
        ok,
        missing,
        changed,
        valid,
    })
}

/// Assina `path` se existir uma chave configurada; nunca interrompe quem chamou. Registra na
/// trilha de auditoria.
pub fn sign_if_configured(path: &Path) {
    let kp = default_key_path();
    if !kp.is_file() {
        return;
    }
    let result = SigningKey::load(&kp)
        .and_then(|k| sign_file(&k, path, std::env::var("GUARA_EXAMINER").ok().as_deref()));
    match result {
        Ok(sig) => crate::audit::record(
            "sign",
            serde_json::json!({ "path": path.to_string_lossy(), "signature": sig.to_string_lossy() }),
        ),
        Err(e) => eprintln!("aviso: não foi possível assinar {}: {e}", path.display()),
    }
}
