//! Abre um arquivo achado no aplicativo padrão do sistema (player de vídeo, leitor
//! de PDF, etc.). Usado quando o WebView do app não consegue reproduzir a mídia:
//! o WebKitGTK empacotado no AppImage não traz os plugins de mídia do GStreamer e
//! o processo da página morre ao criar um `<video>`.

use crate::error::{Error, Result};
use crate::fs::sanitize_name;
use crate::types::Found;
use std::path::{Path, PathBuf};

/// Pasta temporária própria do usuário (nunca compartilhada).
fn temp_dir() -> PathBuf {
    let uid = crate::owner::invoking_user()
        .map(|u| u.uid.to_string())
        .unwrap_or_else(|| {
            #[cfg(unix)]
            {
                // SAFETY: getuid não tem pré-condições.
                unsafe { libc::getuid() }.to_string()
            }
            #[cfg(not(unix))]
            {
                "user".to_string()
            }
        });
    std::env::temp_dir().join(format!("guara-abrir-{uid}"))
}

/// Remove cópias com mais de um dia.
fn cleanup(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let limit = std::time::Duration::from_secs(24 * 3600);
    for e in rd.flatten() {
        if let Ok(age) = e
            .metadata()
            .and_then(|m| m.modified())
            .and_then(|t| t.elapsed().map_err(std::io::Error::other))
        {
            if age > limit {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Copia o arquivo achado para uma pasta temporária do usuário e o abre no aplicativo padrão.
pub fn open_found(src: &dyn crate::source::Source, f: &Found) -> Result<PathBuf> {
    const MAX: u64 = 8 * 1024 * 1024 * 1024;
    if f.size > MAX {
        return Err(Error::msg("Arquivo grande demais para abrir direto. Recupere-o e abra pelo seu gerenciador de arquivos."));
    }
    let dir = temp_dir();
    crate::owner::create_dir_all_owned(&dir)?;
    cleanup(&dir);
    let path = dir.join(format!("{}-{}", f.id, sanitize_name(&f.name)));
    let _ = std::fs::remove_file(&path);
    crate::recover::write_file(src, f, &path)?;
    open_path(&path)
        .map_err(|e| Error::msg(format!("Não foi possível abrir {}: {e}", path.display())))?;
    Ok(path)
}

/// Abre o caminho com o aplicativo padrão, na sessão do usuário real mesmo se formos root.
pub fn open_path(path: &Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    #[cfg(target_os = "linux")]
    let mut cmd = if let Some(u) = crate::owner::invoking_user() {
        let mut c = Command::new("runuser");
        c.args(["-u", &u.name, "--", "env"]);
        for k in ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY"] {
            if let Ok(v) = std::env::var(k) {
                c.arg(format!("{k}={v}"));
            }
        }
        c.arg(format!("XDG_RUNTIME_DIR=/run/user/{}", u.uid));
        c.arg(format!(
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{}/bus",
            u.uid
        ));
        c.arg("xdg-open").arg(path);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]).arg(path);
        c
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
