//! Saúde do disco antes de varrer: lê o SMART (via `smartctl`, quando instalado) e
//! os contadores de erro do kernel. Um disco com defeito deve ser copiado para uma
//! imagem primeiro: cada leitura extra pode piorar o estado dele.

use serde::Serialize;
use serde_json::Value;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Serialize, Default)]
pub struct Health {
    /// Foi possível avaliar? (imagens de disco e dispositivos sem SMART ficam sem avaliação)
    pub available: bool,
    /// "ok", "warning", "failing" ou "unknown".
    pub status: String,
    pub summary: String,
    pub details: Vec<(String, String)>,
    pub advice: Vec<String>,
}

impl Health {
    fn unknown(msg: &str) -> Self {
        Health {
            available: false,
            status: "unknown".into(),
            summary: msg.into(),
            ..Default::default()
        }
    }
}

const IMAGE_ADVICE: &str = "Crie a imagem do disco primeiro (botão \"Criar imagem\" ou `guara image`) e varra a cópia. Cada leitura extra pode piorar um disco com defeito.";

/// Avalia o dispositivo `path`. Para arquivos comuns (imagens) devolve "sem avaliação".
pub fn check(path: &str) -> Health {
    #[cfg(unix)]
    let is_dev = {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(path)
            .map(|m| m.file_type().is_block_device() || m.file_type().is_char_device())
            .unwrap_or(false)
    };
    #[cfg(not(unix))]
    let is_dev = path.starts_with(r"\\.\");
    if !is_dev {
        return Health::unknown("Imagem de disco: não há SMART para avaliar.");
    }
    let mut h = match run_smartctl(path) {
        Some(v) => parse_smartctl(&v),
        None => Health::unknown("O SMART não pôde ser lido. Instale o pacote smartmontools (comando smartctl) e rode como administrador para ver a saúde do disco."),
    };
    kernel_errors(path, &mut h);
    h
}

fn smartctl_name(path: &str) -> String {
    // Windows: \\.\PhysicalDriveN -> /dev/sdX (convenção do smartctl)
    if let Some(n) = path
        .strip_prefix(r"\\.\PhysicalDrive")
        .and_then(|n| n.parse::<u8>().ok())
    {
        return format!("/dev/sd{}", (b'a' + n.min(25)) as char);
    }
    // macOS: /dev/rdiskN -> /dev/diskN
    if let Some(rest) = path.strip_prefix("/dev/rdisk") {
        return format!("/dev/disk{rest}");
    }
    path.to_string()
}

fn run_smartctl(path: &str) -> Option<Value> {
    let dev = smartctl_name(path);
    let out = Command::new("smartctl")
        .args(["--json=o", "-H", "-A", "-i", "-l", "error", &dev])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    // smartctl usa bits do código de saída para avisos; o JSON vale mesmo assim
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    // sem permissão ou dispositivo não suportado
    if v.pointer("/smartctl/messages")
        .and_then(|m| m.as_array())
        .is_some_and(|m| m.iter().any(|x| x["severity"] == "error"))
        && v.get("smart_status").is_none()
        && v.get("ata_smart_attributes").is_none()
        && v.get("nvme_smart_health_information_log").is_none()
    {
        return None;
    }
    Some(v)
}

fn attr(v: &Value, id: u64) -> Option<i64> {
    v.pointer("/ata_smart_attributes/table")?
        .as_array()?
        .iter()
        .find(|a| a["id"] == id)?
        .pointer("/raw/value")?
        .as_i64()
}

/// Interpreta a saída JSON do `smartctl` (testável sem disco).
pub fn parse_smartctl(v: &Value) -> Health {
    let mut h = Health {
        available: true,
        status: "ok".into(),
        ..Default::default()
    };
    let mut bad = 0u32; // 2 = falhando, 1 = atenção
    let mut note = |h: &mut Health, sev: u32, k: &str, val: String| {
        bad = bad.max(sev);
        h.details.push((k.to_string(), val));
    };
    if let Some(m) = v["model_name"].as_str() {
        h.details.push(("Modelo".into(), m.to_string()));
    }
    if let Some(s) = v["serial_number"].as_str() {
        h.details.push(("Nº de série".into(), s.to_string()));
    }
    if let Some(hrs) = v.pointer("/power_on_time/hours").and_then(|x| x.as_i64()) {
        h.details.push(("Horas ligado".into(), hrs.to_string()));
    }
    if let Some(t) = v.pointer("/temperature/current").and_then(|x| x.as_i64()) {
        if t >= 60 {
            note(&mut h, 1, "Temperatura", format!("{t} °C (alta)"));
        } else {
            h.details.push(("Temperatura".into(), format!("{t} °C")));
        }
    }
    if v.pointer("/smart_status/passed") == Some(&Value::Bool(false)) {
        note(
            &mut h,
            2,
            "Autoavaliação SMART",
            "REPROVADO: o próprio disco prevê falha".into(),
        );
    } else if v.pointer("/smart_status/passed") == Some(&Value::Bool(true)) {
        h.details
            .push(("Autoavaliação SMART".into(), "aprovado".into()));
    }
    // ATA (HD e SSD SATA)
    if let Some(n) = attr(v, 5) {
        if n > 0 {
            note(
                &mut h,
                if n >= 50 { 2 } else { 1 },
                "Setores realocados",
                n.to_string(),
            );
        }
    }
    if let Some(n) = attr(v, 197) {
        if n > 0 {
            note(&mut h, 2, "Setores pendentes de realocação", n.to_string());
        }
    }
    if let Some(n) = attr(v, 198) {
        if n > 0 {
            note(&mut h, 2, "Setores incorrigíveis offline", n.to_string());
        }
    }
    if let Some(n) = attr(v, 187) {
        if n > 0 {
            note(&mut h, 1, "Erros incorrigíveis reportados", n.to_string());
        }
    }
    if let Some(n) = attr(v, 199) {
        if n > 0 {
            note(
                &mut h,
                1,
                "Erros de CRC na interface (cabo ou porta)",
                n.to_string(),
            );
        }
    }
    if let Some(n) = v
        .pointer("/ata_smart_error_log/summary/count")
        .and_then(|x| x.as_i64())
    {
        if n > 0 {
            note(
                &mut h,
                1,
                "Erros registrados no log do disco",
                n.to_string(),
            );
        }
    }
    // NVMe
    if let Some(n) = v.get("nvme_smart_health_information_log") {
        if n["critical_warning"].as_i64().unwrap_or(0) != 0 {
            note(
                &mut h,
                2,
                "Aviso crítico NVMe",
                n["critical_warning"].to_string(),
            );
        }
        if let Some(u) = n["percentage_used"].as_i64() {
            if u >= 100 {
                note(&mut h, 2, "Vida útil usada", format!("{u}%"));
            } else if u >= 90 {
                note(&mut h, 1, "Vida útil usada", format!("{u}%"));
            } else {
                h.details.push(("Vida útil usada".into(), format!("{u}%")));
            }
        }
        if let Some(m) = n["media_errors"].as_i64() {
            if m > 0 {
                note(&mut h, 2, "Erros de mídia", m.to_string());
            }
        }
        if let (Some(s), Some(t)) = (
            n["available_spare"].as_i64(),
            n["available_spare_threshold"].as_i64(),
        ) {
            if s < t {
                note(
                    &mut h,
                    2,
                    "Reserva de blocos abaixo do limite",
                    format!("{s}% (limite {t}%)"),
                );
            }
        }
    }
    match bad {
        2 => {
            h.status = "failing".into();
            h.summary = "Este disco mostra sinais sérios de defeito.".into();
            h.advice.push(IMAGE_ADVICE.into());
        }
        1 => {
            h.status = "warning".into();
            h.summary = "Este disco mostra sinais de desgaste ou erros.".into();
            h.advice.push(IMAGE_ADVICE.into());
        }
        _ => h.summary = "Nenhum sinal de defeito encontrado pelo SMART.".into(),
    }
    h
}

/// Contadores de erro de E/S do kernel (Linux): reforçam o aviso do SMART.
fn kernel_errors(path: &str, h: &mut Health) {
    #[cfg(target_os = "linux")]
    {
        let name = path.trim_start_matches("/dev/");
        let base = name.trim_end_matches(|c: char| c.is_ascii_digit());
        for n in [name, base] {
            if let Ok(s) = std::fs::read_to_string(format!("/sys/block/{n}/device/ioerr_cnt")) {
                if let Ok(c) = u64::from_str_radix(s.trim().trim_start_matches("0x"), 16) {
                    if c > 0 {
                        h.details.push((
                            "Erros de leitura/gravação vistos pelo sistema".into(),
                            c.to_string(),
                        ));
                        if h.status == "ok" || h.status == "unknown" {
                            h.available = true;
                            h.status = "warning".into();
                            h.summary = "O sistema já registrou erros de E/S neste disco.".into();
                            h.advice.push(IMAGE_ADVICE.into());
                        }
                    }
                    return;
                }
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (path, h);
}
