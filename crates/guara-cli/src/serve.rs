//! Servidor local da interface (127.0.0.1 + token): mesma UI do app Tauri.

use guara_core::service::Engine;
use serde_json::json;
use std::io::Read;
use tiny_http::{Header, Method, Response, Server};

const INDEX: &str = include_str!("../../../apps/gui/ui/index.html");
const CSS: &str = include_str!("../../../apps/gui/ui/app.css");
const JS: &str = include_str!("../../../apps/gui/ui/app.js");
const ICON: &str = include_str!("../../../apps/gui/ui/guara.svg");

pub fn run(port: u16, open: bool) -> Result<(), String> {
    let server = Server::http(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    let real_port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(port);
    let mut tok = [0u8; 16];
    getrandom::getrandom(&mut tok).map_err(|e| e.to_string())?;
    let token: String = tok.iter().map(|b| format!("{b:02x}")).collect();
    let url = format!("http://127.0.0.1:{real_port}/?token={token}");
    eprintln!("Guará UI em {url}\n(Ctrl+C para encerrar. Para ler discos brutos, rode como administrador/root.)");
    if open {
        open_browser(&url);
    }
    let eng = Engine::new();
    let hosts = [
        format!("127.0.0.1:{real_port}"),
        format!("localhost:{real_port}"),
    ];
    for mut req in server.incoming_requests() {
        let host_ok = req
            .headers()
            .iter()
            .any(|h| h.field.equiv("Host") && hosts.iter().any(|x| x == h.value.as_str()));
        if !host_ok {
            let _ = req.respond(Response::from_string("host inválido").with_status_code(403));
            continue;
        }
        let path = req.url().split('?').next().unwrap_or("/").to_string();
        let ct = |v: &str| Header::from_bytes("Content-Type", v).unwrap();
        match (req.method().clone(), path.as_str()) {
            (Method::Get, "/") | (Method::Get, "/index.html") => {
                let _ = req.respond(
                    Response::from_string(INDEX).with_header(ct("text/html; charset=utf-8")),
                );
            }
            (Method::Get, "/app.css") => {
                let _ = req
                    .respond(Response::from_string(CSS).with_header(ct("text/css; charset=utf-8")));
            }
            (Method::Get, "/app.js") => {
                let _ = req.respond(
                    Response::from_string(JS).with_header(ct("text/javascript; charset=utf-8")),
                );
            }
            (Method::Get, "/guara.svg") => {
                let _ = req.respond(Response::from_string(ICON).with_header(ct("image/svg+xml")));
            }
            (Method::Post, p) if p.starts_with("/rpc/") => {
                let ok = req
                    .headers()
                    .iter()
                    .any(|h| h.field.equiv("X-Guara-Token") && h.value.as_str() == token);
                if !ok {
                    let _ =
                        req.respond(Response::from_string("token inválido").with_status_code(401));
                    continue;
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
    Ok(())
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
