//! Servidor local da interface (127.0.0.1 + token): mesma UI do app Tauri.

use guara_core::service::Engine;
use serde_json::json;
use std::io::Read;
use std::sync::Arc;
use tiny_http::{Header, Method, Request, Response, Server};

const INDEX: &str = include_str!("../../../apps/gui/ui/index.html");
const CSS: &str = include_str!("../../../apps/gui/ui/app.css");
const JS: &str = include_str!("../../../apps/gui/ui/app.js");
const PREVIEW_JS: &str = include_str!("../../../apps/gui/ui/preview.js");
const I18N_JS: &str = include_str!("../../../apps/gui/ui/i18n.js");
const I18N_EN_JS: &str = include_str!("../../../apps/gui/ui/i18n-en.js");
const LOGO: &str = include_str!("../../../apps/gui/ui/guara-logo.svg");
const EXPLORER_JS: &str = include_str!("../../../apps/gui/ui/explorer.js");
const ICON: &str = include_str!("../../../apps/gui/ui/guara.svg");
const PDF_JS: &str = include_str!("../../../apps/gui/ui/vendor/pdf.min.mjs");
const PDF_WORKER: &str = include_str!("../../../apps/gui/ui/vendor/pdf.worker.min.mjs");

pub fn run(port: u16, open: bool) -> Result<(), String> {
    let server = Server::http(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    let real_port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(port);
    let mut tok = [0u8; 16];
    getrandom::getrandom(&mut tok).map_err(|e| e.to_string())?;
    let token: Arc<str> = tok
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .into();
    let url = format!("http://127.0.0.1:{real_port}/?token={token}");
    eprintln!("Guará UI em {url}\n(Ctrl+C para encerrar. Para ler discos brutos, rode como administrador/root.)");
    if open {
        open_browser(&url);
    }
    let eng = Arc::new(Engine::new());
    let hosts: Arc<[String; 2]> = Arc::new([
        format!("127.0.0.1:{real_port}"),
        format!("localhost:{real_port}"),
    ]);
    for req in server.incoming_requests() {
        let (eng, token, hosts) = (eng.clone(), token.clone(), hosts.clone());
        // Uma thread por requisição: vídeo/áudio em streaming não pode travar a interface.
        std::thread::spawn(move || handle(req, &eng, &token, &hosts[..]));
    }
    Ok(())
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn handle(req: Request, eng: &Engine, token: &str, hosts: &[String]) {
    let host_ok = req
        .headers()
        .iter()
        .any(|h| h.field.equiv("Host") && hosts.iter().any(|x| x == h.value.as_str()));
    if !host_ok {
        let _ = req.respond(Response::from_string("host inválido").with_status_code(403));
        return;
    }
    let Some(mut req) = guara_core::http::handle_media(eng, token, req) else {
        return;
    };
    let full = req.url().to_string();
    let (path, _query) = full.split_once('?').unwrap_or((full.as_str(), ""));
    let ct = |v: &str| header("Content-Type", v);
    match (req.method().clone(), path) {
        (Method::Get, "/") | (Method::Get, "/index.html") => {
            let _ = req
                .respond(Response::from_string(INDEX).with_header(ct("text/html; charset=utf-8")));
        }
        (Method::Get, "/app.css") => {
            let _ =
                req.respond(Response::from_string(CSS).with_header(ct("text/css; charset=utf-8")));
        }
        (Method::Get, "/app.js") => {
            let _ = req.respond(
                Response::from_string(JS).with_header(ct("text/javascript; charset=utf-8")),
            );
        }
        (Method::Get, "/preview.js") => {
            let _ = req.respond(
                Response::from_string(PREVIEW_JS).with_header(ct("text/javascript; charset=utf-8")),
            );
        }
        (Method::Get, "/i18n.js") => {
            let _ = req.respond(
                Response::from_string(I18N_JS).with_header(ct("text/javascript; charset=utf-8")),
            );
        }
        (Method::Get, "/i18n-en.js") => {
            let _ = req.respond(
                Response::from_string(I18N_EN_JS).with_header(ct("text/javascript; charset=utf-8")),
            );
        }
        (Method::Get, "/guara-logo.svg") => {
            let _ = req.respond(Response::from_string(LOGO).with_header(ct("image/svg+xml")));
        }
        (Method::Get, "/explorer.js") => {
            let _ = req.respond(
                Response::from_string(EXPLORER_JS)
                    .with_header(ct("text/javascript; charset=utf-8")),
            );
        }
        (Method::Get, "/guara.svg") => {
            let _ = req.respond(Response::from_string(ICON).with_header(ct("image/svg+xml")));
        }
        (Method::Get, "/vendor/pdf.min.mjs") => {
            let _ = req.respond(
                Response::from_string(PDF_JS).with_header(ct("text/javascript; charset=utf-8")),
            );
        }
        (Method::Get, "/vendor/pdf.worker.min.mjs") => {
            let _ = req.respond(
                Response::from_string(PDF_WORKER).with_header(ct("text/javascript; charset=utf-8")),
            );
        }
        (Method::Post, p) if p.starts_with("/rpc/") => {
            let ok = req
                .headers()
                .iter()
                .any(|h| h.field.equiv("X-Guara-Token") && h.value.as_str() == token);
            if !ok {
                let _ = req.respond(Response::from_string("token inválido").with_status_code(401));
                return;
            }
            let method = p.trim_start_matches("/rpc/").to_string();
            let mut body = String::new();
            let _ = req.as_reader().take(1 << 20).read_to_string(&mut body);
            let params = serde_json::from_str(&body).unwrap_or(json!({}));
            let (code, out) = match eng.rpc(&method, params) {
                Ok(v) => (200, v),
                Err(e) => (400, json!({ "error": e.to_string() })),
            };
            let _ = req.respond(
                Response::from_string(out.to_string())
                    .with_status_code(code)
                    .with_header(ct("application/json")),
            );
        }
        _ => {
            let _ = req.respond(Response::from_string("não encontrado").with_status_code(404));
        }
    }
}

fn open_browser(url: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
}
