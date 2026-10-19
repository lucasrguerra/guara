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
    /// Ficha do dispositivo, no estilo do CrystalDiskInfo (modelo, firmware, interface...).
    pub info: Vec<(String, String)>,
    /// Saúde estimada em % (SSD: vida útil restante). `None` em HD, que não informa isso.
    pub health_pct: Option<u8>,
    pub temperature_c: Option<i64>,
    pub power_on_hours: Option<i64>,
    pub power_cycles: Option<i64>,
    /// Total lido e gravado pelo computador, em bytes, quando o disco informa.
    pub host_reads: Option<u64>,
    pub host_writes: Option<u64>,
    /// Tabela completa de atributos SMART.
    pub attributes: Vec<Attr>,
    /// Caminho avaliado de fato (o disco inteiro, quando `path` é uma partição).
    pub disk: String,
    /// Partição pedida, quando a avaliação foi feita no disco que a contém.
    pub partition: Option<String>,
}

/// Uma linha da tabela de atributos SMART.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Attr {
    /// Identificador em hexadecimal, como o CrystalDiskInfo mostra ("05", "C5").
    pub id: String,
    pub name: String,
    /// Valores normalizados (ATA): atual, pior já visto e limite de falha.
    pub value: Option<i64>,
    pub worst: Option<i64>,
    pub thresh: Option<i64>,
    pub raw: String,
    /// "ok", "warning" ou "failing".
    pub status: String,
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
    let disk = parent_disk(path);
    let mut h = match run_smartctl(&disk) {
        Some(v) => parse_smartctl(&v),
        None => Health::unknown("O SMART não pôde ser lido. Instale o pacote smartmontools (comando smartctl) e rode como administrador para ver a saúde do disco."),
    };
    kernel_errors(path, &mut h);
    if disk != path {
        h.partition = Some(path.to_string());
    }
    h.disk = disk;
    h
}

/// O SMART é do disco, não da partição: `/dev/sda1` vira `/dev/sda`,
/// `/dev/nvme0n1p2` vira `/dev/nvme0n1` e `/dev/disk2s1` vira `/dev/disk2`.
pub fn parent_disk(path: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        let name = path.trim_start_matches("/dev/");
        let sys = format!("/sys/class/block/{name}");
        if std::path::Path::new(&format!("{sys}/partition")).exists() {
            if let Ok(real) = std::fs::canonicalize(&sys) {
                if let Some(p) = real.parent().and_then(|p| p.file_name()) {
                    return format!("/dev/{}", p.to_string_lossy());
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        for pre in ["/dev/rdisk", "/dev/disk"] {
            if let Some(rest) = path.strip_prefix(pre) {
                if let Some(i) = rest.find('s') {
                    return format!("{pre}{}", &rest[..i]);
                }
            }
        }
    }
    path.to_string()
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
        .args(["--json=o", "-a", &dev])
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
    fill_info(v, &mut h);
    fill_attributes(v, &mut h);
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
    if h.attributes.iter().any(|a| a.status == "failing") {
        bad = 2;
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

fn s(v: &Value, p: &str) -> Option<String> {
    v.pointer(p)
        .and_then(|x| x.as_str())
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
}
fn n(v: &Value, p: &str) -> Option<i64> {
    v.pointer(p).and_then(|x| x.as_i64())
}

/// Ficha do dispositivo e métricas gerais (temperatura, horas, total lido/gravado, saúde %).
fn fill_info(v: &Value, h: &mut Health) {
    let mut put = |k: &str, val: Option<String>| {
        if let Some(val) = val {
            h.info.push((k.to_string(), val));
        }
    };
    put(
        "Modelo",
        s(v, "/model_name").or_else(|| {
            let a = s(v, "/scsi_vendor")?;
            Some(format!("{a} {}", s(v, "/scsi_product").unwrap_or_default()))
        }),
    );
    put("Firmware", s(v, "/firmware_version"));
    put("Nº de série", s(v, "/serial_number"));
    put("Família", s(v, "/model_family"));
    let proto = s(v, "/device/protocol");
    let iface = s(v, "/sata_version/string").or_else(|| {
        let sp = v
            .pointer("/nvme_pci_vendor")
            .map(|_| "NVM Express".to_string());
        sp.or_else(|| proto.clone())
    });
    put("Interface", iface);
    let speed = |p: &str| s(v, &format!("/interface_speed/{p}/string"));
    put(
        "Modo de transferência",
        match (speed("current"), speed("max")) {
            (Some(c), Some(m)) if c != m => Some(format!("{c} (máx. {m})")),
            (Some(c), _) => Some(c),
            (None, Some(m)) => Some(m),
            _ => None,
        },
    );
    put(
        "Capacidade",
        n(v, "/user_capacity/bytes")
            .or_else(|| n(v, "/nvme_total_capacity"))
            .map(|b| decimal_size(b as u64)),
    );
    put(
        "Rotação",
        n(v, "/rotation_rate").map(|r| {
            if r == 0 {
                "SSD (sem rotação)".into()
            } else {
                format!("{r} RPM")
            }
        }),
    );
    put("Formato", s(v, "/form_factor/name"));
    let mut feats = Vec::new();
    if v.pointer("/smart_support/enabled") == Some(&Value::Bool(true))
        || v.get("nvme_smart_health_information_log").is_some()
    {
        feats.push("S.M.A.R.T.");
    }
    if v.pointer("/trim/supported") == Some(&Value::Bool(true)) {
        feats.push("TRIM");
    }
    if v.pointer("/write_cache/enabled") == Some(&Value::Bool(true)) {
        feats.push("Cache de gravação");
    }
    if v.get("ata_security").and_then(|x| x.get("enabled")) == Some(&Value::Bool(true)) {
        feats.push("Segurança ATA");
    }
    if !feats.is_empty() {
        put("Recursos", Some(feats.join(", ")));
    }

    h.temperature_c = n(v, "/temperature/current");
    h.power_on_hours = n(v, "/power_on_time/hours");
    h.power_cycles = n(v, "/power_cycle_count");
    if let Some(l) = v.get("nvme_smart_health_information_log") {
        // a unidade NVMe vale 1000 setores de 512 bytes
        h.host_reads = l["data_units_read"].as_u64().map(|u| u * 512_000);
        h.host_writes = l["data_units_written"].as_u64().map(|u| u * 512_000);
        h.health_pct = l["percentage_used"]
            .as_i64()
            .map(|u| (100 - u.clamp(0, 100)) as u8);
        h.power_cycles = h.power_cycles.or(l["power_cycles"].as_i64());
    } else {
        let lba = n(v, "/logical_block_size").unwrap_or(512) as u64;
        h.host_reads = attr(v, 242).map(|x| x as u64 * lba);
        h.host_writes = attr(v, 241).map(|x| x as u64 * lba);
        // SSD SATA: vida útil restante no valor normalizado de um destes atributos
        // (231 SSD Life Left, 233 Media Wearout, 177 Wear Leveling, 202 Percent Lifetime Remain)
        h.health_pct = [231u64, 233, 177, 202, 169]
            .iter()
            .find_map(|&id| attr_field(v, id, "value"))
            .filter(|_| n(v, "/rotation_rate") == Some(0))
            .map(|x| x.clamp(0, 100) as u8);
    }
}

/// Capacidade como o fabricante anuncia (base 1000): "500,1 GB".
fn decimal_size(b: u64) -> String {
    let units = ["B", "kB", "MB", "GB", "TB", "PB"];
    let mut x = b as f64;
    let mut i = 0;
    while x >= 1000.0 && i < units.len() - 1 {
        x /= 1000.0;
        i += 1;
    }
    format!("{x:.1} {}", units[i]).replace('.', ",")
}

fn attr_field(v: &Value, id: u64, f: &str) -> Option<i64> {
    v.pointer("/ata_smart_attributes/table")?
        .as_array()?
        .iter()
        .find(|a| a["id"] == id)?
        .get(f)?
        .as_i64()
}

/// Atributos cujo valor bruto acima de zero já é motivo de atenção.
const RAW_WARN: [u64; 5] = [5, 196, 197, 198, 187];

fn fill_attributes(v: &Value, h: &mut Health) {
    if let Some(t) = v
        .pointer("/ata_smart_attributes/table")
        .and_then(|t| t.as_array())
    {
        for a in t {
            let id = a["id"].as_u64().unwrap_or(0);
            let value = a["value"].as_i64();
            let thresh = a["thresh"].as_i64();
            let raw_n = a
                .pointer("/raw/value")
                .and_then(|x| x.as_i64())
                .unwrap_or(0);
            let failing = a
                .pointer("/when_failed")
                .and_then(|x| x.as_str())
                .is_some_and(|w| w == "now")
                || matches!((value, thresh), (Some(v), Some(t)) if t > 0 && v <= t);
            let status = if failing {
                "failing"
            } else if RAW_WARN.contains(&id) && raw_n > 0 {
                "warning"
            } else {
                "ok"
            };
            h.attributes.push(Attr {
                id: format!("{id:02X}"),
                name: a["name"]
                    .as_str()
                    .unwrap_or("Unknown_Attribute")
                    .replace('_', " "),
                value,
                worst: a["worst"].as_i64(),
                thresh,
                raw: a
                    .pointer("/raw/string")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| raw_n.to_string()),
                status: status.into(),
            });
        }
    }
    if let Some(l) = v.get("nvme_smart_health_information_log") {
        // mesma ordem e nomes que o CrystalDiskInfo usa para NVMe
        let rows: [(u8, &str, &str); 15] = [
            (1, "Critical Warning", "critical_warning"),
            (2, "Composite Temperature", "temperature"),
            (3, "Available Spare", "available_spare"),
            (4, "Available Spare Threshold", "available_spare_threshold"),
            (5, "Percentage Used", "percentage_used"),
            (6, "Data Units Read", "data_units_read"),
            (7, "Data Units Written", "data_units_written"),
            (8, "Host Read Commands", "host_reads"),
            (9, "Host Write Commands", "host_writes"),
            (10, "Controller Busy Time", "controller_busy_time"),
            (11, "Power Cycles", "power_cycles"),
            (12, "Power On Hours", "power_on_hours"),
            (13, "Unsafe Shutdowns", "unsafe_shutdowns"),
            (14, "Media and Data Integrity Errors", "media_errors"),
            (
                15,
                "Number of Error Information Log Entries",
                "num_err_log_entries",
            ),
        ];
        let spare_low = matches!((l["available_spare"].as_i64(), l["available_spare_threshold"].as_i64()), (Some(s), Some(t)) if s < t);
        for (id, name, key) in rows {
            let Some(x) = l.get(key).and_then(|x| x.as_i64()) else {
                continue;
            };
            let status = match key {
                "critical_warning" | "media_errors" if x > 0 => "failing",
                "available_spare" if spare_low => "failing",
                "percentage_used" if x >= 100 => "failing",
                "percentage_used" if x >= 90 => "warning",
                _ => "ok",
            };
            h.attributes.push(Attr {
                id: format!("{id:02X}"),
                name: name.into(),
                raw: x.to_string(),
                status: status.into(),
                ..Default::default()
            });
        }
    }
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
