//! Trilha de auditoria: cada ação do examinador (abrir uma origem, varrer, recuperar, criar
//! imagem, exportar) vira uma linha JSON com quem, quando e o quê.
//!
//! As linhas são encadeadas por hash: o `hash` de cada uma é o SHA-256 do `hash` da anterior
//! seguido do corpo da própria linha. Alterar ou apagar uma linha do meio quebra a cadeia e
//! `guara audit --verify` aponta onde. Cortar o fim do arquivo não é detectável sozinho; para
//! isso, guarde o hash da última entrada (a "cabeça") fora da máquina, por exemplo no relatório.
//!
//! O arquivo fica na pasta de configuração (`auditoria.jsonl`) ou onde `GUARA_AUDIT_LOG`
//! apontar. Segredos (senhas, chaves) nunca entram no registro.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Serialize)]
struct Body<'a> {
    seq: u64,
    time: &'a str,
    user: &'a str,
    host: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    examiner: Option<&'a str>,
    version: &'a str,
    action: &'a str,
    detail: &'a Value,
    prev: &'a str,
}

/// Uma linha da trilha.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    /// UTC, ISO 8601.
    pub time: String,
    pub user: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examiner: Option<String>,
    pub version: String,
    pub action: String,
    pub detail: Value,
    pub prev: String,
    pub hash: String,
}

/// Resultado de conferir a cadeia.
#[derive(Debug, Clone, Serialize)]
pub struct Verification {
    pub entries: usize,
    pub ok: bool,
    /// Entrada (1, 2, ...) onde a cadeia quebra e o motivo.
    pub broken_at: Option<(u64, String)>,
    /// Hash da última entrada; guarde-o fora da máquina para detectar truncamento.
    pub head: String,
}

pub struct Log {
    path: PathBuf,
}

fn sha_hex(prev: &str, body: &str) -> String {
    let mut h = Sha256::new();
    h.update(prev.as_bytes());
    h.update(b"\n");
    h.update(body.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// UTC em ISO 8601 a partir de segundos Unix (conversão civil de Howard Hinnant).
pub fn utc_iso(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    utc_iso(secs)
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// Usuário do sistema por trás da ação (o que chamou sudo, quando houver).
fn current_user() -> String {
    env("SUDO_USER")
        .or_else(|| env("USER"))
        .or_else(|| env("USERNAME"))
        .or_else(|| env("LOGNAME"))
        .unwrap_or_else(|| "?".into())
}

fn host_name() -> String {
    env("HOSTNAME")
        .or_else(|| env("COMPUTERNAME"))
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "?".into())
}

/// Última linha não vazia do arquivo, lendo só o fim.
fn last_line(path: &Path) -> std::io::Result<Option<String>> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let len = f.metadata()?.len();
    let mut want = 8192u64;
    loop {
        let start = len.saturating_sub(want);
        f.seek(SeekFrom::Start(start))?;
        let mut buf = vec![];
        f.read_to_end(&mut buf)?;
        let text = String::from_utf8_lossy(&buf);
        let trimmed = text.trim_end();
        match trimmed.rfind('\n') {
            Some(i) => return Ok(Some(trimmed[i + 1..].to_string())),
            None if start == 0 => {
                return Ok((!trimmed.is_empty()).then(|| trimmed.to_string()));
            }
            None => want *= 4,
        }
    }
}

impl Log {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `GUARA_AUDIT_LOG` ou `auditoria.jsonl` na pasta de configuração.
    pub fn default_path() -> PathBuf {
        match env("GUARA_AUDIT_LOG") {
            Some(p) => PathBuf::from(p),
            None => crate::settings::config_dir().join("auditoria.jsonl"),
        }
    }

    pub fn open_default() -> Self {
        Self::at(Self::default_path())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Acrescenta uma entrada ao fim da cadeia.
    pub fn append(&self, action: &str, detail: Value) -> Result<Entry> {
        let (seq, prev) = match last_line(&self.path)? {
            None => (1, GENESIS.to_string()),
            Some(l) => {
                let e: Entry = serde_json::from_str(&l)
                    .map_err(|e| Error::msg(format!("última linha da trilha inválida: {e}")))?;
                (e.seq + 1, e.hash)
            }
        };
        let (time, user, host) = (now_iso(), current_user(), host_name());
        let examiner = env("GUARA_EXAMINER");
        let version = env!("CARGO_PKG_VERSION");
        let body = serde_json::to_string(&Body {
            seq,
            time: &time,
            user: &user,
            host: &host,
            examiner: examiner.as_deref(),
            version,
            action,
            detail: &detail,
            prev: &prev,
        })?;
        let hash = sha_hex(&prev, &body);
        let line = format!("{},\"hash\":\"{hash}\"}}\n", &body[..body.len() - 1]);
        if let Some(dir) = self.path.parent() {
            crate::owner::create_dir_all_owned(dir)?;
        }
        let existed = self.path.exists();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.flush()?;
        if !existed {
            crate::owner::give_to_user(&self.path);
        }
        Ok(Entry {
            seq,
            time,
            user,
            host,
            examiner,
            version: version.into(),
            action: action.into(),
            detail,
            prev,
            hash,
        })
    }

    /// Todas as entradas, na ordem. Linha ilegível vira erro (use [`Log::verify`] para saber onde).
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .enumerate()
            .map(|(i, l)| {
                serde_json::from_str(l)
                    .map_err(|e| Error::msg(format!("linha {} da trilha inválida: {e}", i + 1)))
            })
            .collect()
    }

    /// Confere a numeração, o encadeamento e o hash de cada linha.
    pub fn verify(&self) -> Result<Verification> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let mut prev = GENESIS.to_string();
        let mut n = 0usize;
        let broken = |n: usize, why: &str, entries: usize, head: &str| Verification {
            entries,
            ok: false,
            broken_at: Some((n as u64, why.to_string())),
            head: head.to_string(),
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            n += 1;
            let Ok(e) = serde_json::from_str::<Entry>(line) else {
                return Ok(broken(n, "linha ilegível", n - 1, &prev));
            };
            if e.seq != n as u64 {
                return Ok(broken(
                    n,
                    "numeração fora de ordem (entrada removida ou inserida)",
                    n - 1,
                    &prev,
                ));
            }
            if e.prev != prev {
                return Ok(broken(n, "não continua a entrada anterior", n - 1, &prev));
            }
            let Some(i) = line.rfind(",\"hash\":\"") else {
                return Ok(broken(n, "linha sem hash", n - 1, &prev));
            };
            let body = format!("{}}}", &line[..i]);
            if sha_hex(&prev, &body) != e.hash {
                return Ok(broken(
                    n,
                    "conteúdo alterado (o hash não confere)",
                    n - 1,
                    &prev,
                ));
            }
            prev = e.hash;
        }
        Ok(Verification {
            entries: n,
            ok: true,
            broken_at: None,
            head: prev,
        })
    }
}

/// Registra uma ação na trilha padrão. Falha ao gravar não interrompe a operação, mas é
/// avisada no stderr: uma ação sem registro precisa ser notada.
pub fn record(action: &str, detail: Value) {
    if let Err(e) = Log::open_default().append(action, detail) {
        eprintln!("aviso: não foi possível gravar a trilha de auditoria ({action}): {e}");
    }
}

/// Atalho para detalhes de um caminho de origem.
pub fn path_detail(path: &str) -> Value {
    json!({ "path": path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_de_datas_conhecidas() {
        assert_eq!(utc_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_iso(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_iso(1_792_000_000 + 3661), "2026-10-14T18:47:41Z");
    }
}
