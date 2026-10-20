//! Notificações do sistema (varredura concluída, disco desconectado...), usando os
//! comandos nativos de cada sistema, sem dependências extras.

use std::process::{Command, Stdio};

/// Avisos que o núcleo envia ao sistema operacional. O texto é montado no
/// idioma efetivo da interface (`settings::ui_lang`), porque notificações não
/// passam pelo dicionário do DOM.
pub enum Notice {
    ScanDone { count: usize, target: String },
    RecoverDone { count: usize, destination: String },
    DiskLost,
}

impl Notice {
    fn text(&self, lang: &str) -> (String, String) {
        let en = lang == "en";
        match self {
            Notice::ScanDone { count, target } => (
                "Guará: scan complete".into(),
                if en {
                    format!("{count} file(s) found in {target}.")
                } else {
                    format!("{count} arquivo(s) encontrados em {target}.")
                },
            ),
            Notice::RecoverDone { count, destination } => (
                if en {
                    "Guará: recovery complete".into()
                } else {
                    "Guará: recuperação concluída".into()
                },
                if en {
                    format!("{count} file(s) saved to {destination}")
                } else {
                    format!("{count} arquivo(s) salvos em {destination}")
                },
            ),
            Notice::DiskLost => (
                if en {
                    "Guará: disk disconnected?".into()
                } else {
                    "Guará: disco desconectado?".into()
                },
                if en {
                    "The scan was paused without losing anything. Reconnect the disk and continue."
                        .into()
                } else {
                    "A varredura foi pausada sem perder nada. Reconecte o disco e continue.".into()
                },
            ),
        }
    }
}

/// Envia uma notificação sem bloquear. Falhas são ignoradas (é só um aviso).
pub fn send(notice: Notice) {
    let (title, body) = notice.text(&crate::settings::ui_lang());
    std::thread::spawn(move || {
        if let Some(mut c) = command(&title, &body) {
            if let Ok(mut child) = c
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                let _ = child.wait();
            }
        }
    });
}

#[cfg(target_os = "linux")]
fn command(title: &str, body: &str) -> Option<Command> {
    // Como root, notifica na sessão do usuário real.
    let mut c = if let Some(u) = crate::owner::invoking_user() {
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
        c.arg("notify-send");
        c
    } else {
        Command::new("notify-send")
    };
    c.args(["-a", "Guará", "-i", "drive-harddisk", title, body]);
    Some(c)
}

#[cfg(target_os = "macos")]
fn command(title: &str, body: &str) -> Option<Command> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut c = Command::new("osascript");
    c.args([
        "-e",
        &format!(
            "display notification \"{}\" with title \"{}\"",
            esc(body),
            esc(title)
        ),
    ]);
    Some(c)
}

#[cfg(windows)]
fn command(title: &str, body: &str) -> Option<Command> {
    let esc = |s: &str| s.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; $n = New-Object System.Windows.Forms.NotifyIcon; $n.Icon = [System.Drawing.SystemIcons]::Information; $n.Visible = $true; $n.ShowBalloonTip(6000, '{}', '{}', [System.Windows.Forms.ToolTipIcon]::None); Start-Sleep -Seconds 7; $n.Dispose()",
        esc(title),
        esc(body)
    );
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]);
    Some(c)
}
