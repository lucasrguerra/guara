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
/// manifesto já exige elevação. O app novo encerra este quando abre; se a
/// autorização falhar, a janela continua aqui e a interface mostra o motivo.
#[tauri::command]
fn relaunch_elevated(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use tauri::Emitter;
        let exe = elevated_exe()?;
        let cfg = guara_core::settings::config_dir();
        let log = std::env::temp_dir().join(format!("guara-elevate-{}.log", std::process::id()));
        let _ = std::fs::write(&log, b"");
        let out = std::fs::OpenOptions::new().append(true).open(&log).map_err(|e| e.to_string())?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let pid = std::process::id().to_string();

        #[cfg(target_os = "linux")]
        let mut cmd = {
            let mut c = std::process::Command::new("pkexec");
            c.arg("/usr/bin/env");
            for k in ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS", "GDK_BACKEND"] {
                if let Ok(v) = std::env::var(k) {
                    c.arg(format!("{k}={v}"));
                }
            }
            c.arg(format!("GUARA_CONFIG_DIR={}", cfg.display()));
            c.arg(format!("GUARA_REPLACE_PID={pid}"));
            c.arg(&exe);
            c
        };
        #[cfg(target_os = "macos")]
        let mut cmd = {
            let shell = format!(
                "GUARA_CONFIG_DIR='{}' GUARA_REPLACE_PID={} '{}' >/dev/null 2>&1 &",
                cfg.display(),
                pid,
                exe.display()
            );
            let script = format!("do shell script \"{}\" with administrator privileges", shell.replace('\\', "\\\\").replace('"', "\\\""));
            let mut c = std::process::Command::new("osascript");
            c.args(["-e", &script]);
            c
        };
        cmd.stdout(out).stderr(err);
        let mut child = cmd.spawn().map_err(|e| format!("não foi possível pedir privilégios: {e}"))?;
        std::thread::spawn(move || {
            // No Linux o pkexec só termina com sucesso quando o app elevado é fechado (e o app
            // elevado já terá encerrado este processo). Qualquer falha antes disso vira aviso.
            let status = child.wait();
            if matches!(&status, Ok(s) if s.success()) {
                let _ = std::fs::remove_file(&log);
                return;
            }
            let detail = std::fs::read_to_string(&log).unwrap_or_default();
            let msg = match status.ok().and_then(|s| s.code()) {
                Some(126) => "Autorização cancelada.".to_string(),
                Some(127) => "Não foi possível autenticar como administrador.".to_string(),
                _ => format!("Não foi possível reiniciar como administrador. {}", detail.lines().last().unwrap_or("").trim()),
            };
            let _ = app.emit("elevate-failed", msg);
            let _ = std::fs::remove_file(&log);
        });
        Ok(())
    }
    #[cfg(windows)]
    {
        let _ = app;
        Err("o Guará já roda como administrador no Windows".into())
    }
}

/// Caminho que o root consegue executar. No AppImage, `current_exe` aponta para
/// dentro de um ponto de montagem FUSE do usuário, inacessível ao root: usa o
/// próprio arquivo `.AppImage` (variável `APPIMAGE`).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn elevated_exe() -> Result<std::path::PathBuf, String> {
    if let Some(p) = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from).filter(|p| p.is_file()) {
        return Ok(p);
    }
    std::env::current_exe().map_err(|e| e.to_string())
}

/// Serve os arquivos recuperados para pré-visualização (`guara://localhost/file/<job>/<id>`),
/// com suporte a Range para vídeo/áudio/PDF, lendo direto da origem.
fn serve_file(eng: &Engine, req: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::Response;
    const CAP: u64 = 8 * 1024 * 1024;
    let mut it = req.uri().path().trim_start_matches("/file/").split('/');
    let (job, id) = (it.next().unwrap_or(""), it.next().and_then(|s| s.parse::<u64>().ok()));
    let err = |code: u16| Response::builder().status(code).header("Access-Control-Allow-Origin", "*").body(Vec::new()).unwrap();
    let Some(id) = id else { return err(400) };
    let range = req.headers().get("range").and_then(|v| v.to_str().ok());
    let slice = match eng.file_slice(job, id, range) {
        Ok(s) => s,
        Err(code) => return err(code),
    };
    let total = slice.total;
    let start = slice.start;
    let want = slice.len();
    let mut headers = slice.headers();
    let partial = slice.partial || want > CAP;
    let Ok(data) = slice.read_all(CAP) else { return err(500) };
    let mut b = Response::builder().status(if partial { 206 } else { 200 });
    headers.retain(|(k, _)| *k != "Content-Range");
    for (k, v) in &headers {
        b = b.header(*k, v);
    }
    if partial && !data.is_empty() {
        b = b.header("Content-Range", format!("bytes {}-{}/{}", start, start + data.len() as u64 - 1, total));
    }
    b.body(data).unwrap_or_else(|_| err(500))
}

fn main() {
    let engine = Arc::new(Engine::new());
    let for_protocol = engine.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(State(engine))
        .register_asynchronous_uri_scheme_protocol("guara", move |_ctx, request, responder| {
            let eng = for_protocol.clone();
            std::thread::spawn(move || responder.respond(serve_file(&eng, &request)));
        })
        .setup(|_app| {
            // Quando abrimos elevados a pedido do app normal, ele fecha o app antigo.
            #[cfg(unix)]
            if let Some(pid) = std::env::var("GUARA_REPLACE_PID").ok().and_then(|p| p.parse::<i32>().ok()) {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1200));
                    // SAFETY: só envia SIGTERM ao processo anterior do próprio usuário.
                    unsafe {
                        if libc::geteuid() == 0 {
                            libc::kill(pid, libc::SIGTERM);
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc, relaunch_elevated])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o Guará");
}
