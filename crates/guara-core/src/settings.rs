//! Preferências do usuário (tema etc.), compartilhadas entre o app normal e o
//! reiniciado como administrador.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// "auto", "light" ou "dark".
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "auto".into(),
        }
    }
}

/// Pasta de configuração do usuário *real*, mesmo quando rodando como root
/// via sudo/pkexec (para o tema não "mudar" ao elevar privilégios).
pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("GUARA_CONFIG_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    #[cfg(unix)]
    {
        if let Some(home) = invoking_user_home() {
            return default_dir(home);
        }
    }
    default_dir(PathBuf::from(
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default(),
    ))
}

fn default_dir(home: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(a) = std::env::var("APPDATA") {
            return PathBuf::from(a).join("Guara");
        }
        home.join("AppData").join("Roaming").join("Guara")
    }
    #[cfg(target_os = "macos")]
    {
        home.join("Library")
            .join("Application Support")
            .join("Guara")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        match std::env::var("XDG_CONFIG_HOME") {
            Ok(x)
                if !x.is_empty()
                    && std::env::var("SUDO_USER").is_err()
                    && std::env::var("PKEXEC_UID").is_err() =>
            {
                PathBuf::from(x).join("guara")
            }
            _ => home.join(".config").join("guara"),
        }
    }
}

/// Se somos root por sudo/pkexec, a home de quem invocou.
#[cfg(unix)]
fn invoking_user_home() -> Option<PathBuf> {
    // SAFETY: geteuid não tem pré-condições.
    if unsafe { libc::geteuid() } != 0 {
        return None;
    }
    let user = std::env::var("SUDO_USER").ok().filter(|u| u != "root");
    let uid = std::env::var("PKEXEC_UID").ok();
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for l in passwd.lines() {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() >= 6 && (user.as_deref() == Some(f[0]) || uid.as_deref() == Some(f[2])) {
            return Some(PathBuf::from(f[5]));
        }
    }
    None
}

fn file() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    std::fs::read(file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = file();
    std::fs::write(&path, serde_json::to_vec_pretty(s)?)?;
    #[cfg(unix)]
    {
        // Como root, devolve a posse ao dono da pasta para o app normal conseguir regravar.
        use std::os::unix::fs::MetadataExt;
        if unsafe { libc::geteuid() } == 0 {
            if let Ok(m) = std::fs::metadata(&dir) {
                let _ = std::os::unix::fs::chown(&path, Some(m.uid()), Some(m.gid()));
            }
        }
    }
    Ok(())
}
