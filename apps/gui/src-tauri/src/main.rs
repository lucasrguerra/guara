// Evita a janela de console extra no Windows em release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use guara_core::service::Engine;
use std::sync::Arc;

struct State(Arc<Engine>);

/// Ponte única entre a interface e o núcleo (mesmos métodos do modo `guara ui`).
#[tauri::command]
async fn rpc(state: tauri::State<'_, State>, method: String, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let eng = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || eng.rpc(&method, params).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Reabre o app com privilégios de administrador (Linux/macOS). No Windows o
/// manifesto já exige elevação.
#[tauri::command]
fn relaunch_elevated() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    {
        let mut cmd = std::process::Command::new("pkexec");
        cmd.arg("env");
        for k in ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"] {
            if let Ok(v) = std::env::var(k) {
                cmd.arg(format!("{k}={v}"));
            }
        }
        cmd.arg(exe);
        cmd.spawn().map_err(|e| format!("pkexec indisponível: {e}"))?;
        std::process::exit(0);
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!("do shell script \"'{}' &> /dev/null &\" with administrator privileges", exe.display());
        std::process::Command::new("osascript").args(["-e", &script]).spawn().map_err(|e| e.to_string())?;
        std::process::exit(0);
    }
    #[cfg(windows)]
    {
        let _ = exe;
        Err("o Guará já roda como administrador no Windows".into())
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(State(Arc::new(Engine::new())))
        .invoke_handler(tauri::generate_handler![rpc, relaunch_elevated])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o Guará");
}
