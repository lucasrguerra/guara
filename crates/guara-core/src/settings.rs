//! Preferências do usuário (tema etc.), compartilhadas entre o app normal e o
//! reiniciado como administrador.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// "auto", "light" ou "dark".
    #[serde(default = "auto")]
    pub theme: String,
    /// Idioma da interface: "auto", "pt" ou "en".
    #[serde(default = "auto")]
    pub lang: String,
    /// Idioma que a interface resolveu de fato ("pt" ou "en"), para as
    /// notificações do sistema e a saída do terminal acompanharem a tela.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_lang: Option<String>,
    /// Notificação do sistema quando uma tarefa longa termina.
    #[serde(default = "yes")]
    pub notify: bool,
    /// Mostrar mapas das fotos com GPS (baixa blocos do OpenStreetMap pela internet).
    #[serde(default)]
    pub maps: bool,
}

fn auto() -> String {
    "auto".into()
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: auto(),
            lang: auto(),
            ui_lang: None,
            notify: true,
            maps: false,
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
        if let Some(u) = crate::owner::invoking_user() {
            return default_dir(u.home);
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

fn file() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    std::fs::read(file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Idioma efetivo para textos gerados fora da interface (notificações e CLI).
/// Usa o que a interface informou; sem isso, a preferência e o ambiente.
pub fn ui_lang() -> String {
    let s = load();
    if let Some(l) = s.ui_lang.as_deref() {
        if l == "pt" || l == "en" {
            return l.to_string();
        }
    }
    if s.lang == "pt" || s.lang == "en" {
        return s.lang;
    }
    let env = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if env.starts_with("pt") {
        "pt".into()
    } else {
        "en".into()
    }
}

pub fn save(s: &Settings) -> Result<()> {
    crate::owner::write_owned(&file(), serde_json::to_vec_pretty(s)?)?;
    Ok(())
}
