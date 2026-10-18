//! Rotas HTTP de mídia (`/file` com Range e `/thumb`) compartilhadas pelo modo `guara ui`.

use crate::service::Engine;
use tiny_http::{Header, Method, Request, Response, StatusCode};

pub fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

/// O cabeçalho Host bate com um dos endereços esperados (defesa contra DNS rebinding)?
pub fn host_ok(req: &Request, hosts: &[String]) -> bool {
    req.headers()
        .iter()
        .any(|h| h.field.equiv("Host") && hosts.iter().any(|x| x == h.value.as_str()))
}

/// Responde `/file/<job>/<id>` (com Range) e `/thumb/<job>/<id>?s=NNN`.
/// Devolve a requisição quando ela não é uma rota de mídia.
pub fn handle_media(eng: &Engine, token: &str, req: Request) -> Option<Request> {
    let full = req.url().to_string();
    let (path, query) = full.split_once('?').unwrap_or((full.as_str(), ""));
    let is_file = path.starts_with("/file/");
    let is_thumb = path.starts_with("/thumb/");
    if !(is_file || is_thumb) {
        return Some(req);
    }
    if req.method() == &Method::Options {
        let r = Response::empty(204)
            .with_header(header("Access-Control-Allow-Origin", "*"))
            .with_header(header("Access-Control-Allow-Headers", "Range"))
            .with_header(header("Access-Control-Allow-Methods", "GET, OPTIONS"));
        let _ = req.respond(r);
        return None;
    }
    if req.method() != &Method::Get {
        let _ = req.respond(Response::from_string("método não permitido").with_status_code(405));
        return None;
    }
    if !query
        .split('&')
        .any(|kv| kv.strip_prefix("t=") == Some(token))
    {
        let _ = req.respond(Response::from_string("token inválido").with_status_code(401));
        return None;
    }
    let mut it = path
        .trim_start_matches("/file/")
        .trim_start_matches("/thumb/")
        .split('/');
    let (job, id) = (
        it.next().unwrap_or(""),
        it.next().and_then(|s| s.parse::<u64>().ok()),
    );
    let Some(id) = id else {
        let _ = req.respond(Response::from_string("id inválido").with_status_code(400));
        return None;
    };
    if is_thumb {
        let size = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("s="))
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(256);
        match eng.thumb(job, id, size) {
            Ok(bytes) => {
                let r = Response::from_data(bytes.as_ref().clone())
                    .with_header(header("Content-Type", "image/jpeg"))
                    .with_header(header("Cache-Control", "private, max-age=3600"))
                    .with_header(header("Access-Control-Allow-Origin", "*"));
                let _ = req.respond(r);
            }
            Err(code) => {
                let _ = req.respond(
                    Response::from_string("sem miniatura")
                        .with_status_code(code)
                        .with_header(header("Access-Control-Allow-Origin", "*")),
                );
            }
        }
        return None;
    }
    let range = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str().to_string());
    match eng.file_slice(job, id, range.as_deref()) {
        Ok(slice) => {
            let (status, len) = (slice.status(), slice.len() as usize);
            let headers: Vec<Header> = slice.headers().iter().map(|(k, v)| header(k, v)).collect();
            let resp = Response::new(StatusCode(status), headers, slice.reader(), Some(len), None);
            let _ = req.respond(resp);
        }
        Err(code) => {
            let _ = req.respond(
                Response::from_string("indisponível")
                    .with_status_code(code)
                    .with_header(header("Access-Control-Allow-Origin", "*")),
            );
        }
    }
    None
}
