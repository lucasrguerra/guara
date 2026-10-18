//! Enumeração de dispositivos de armazenamento (melhor esforço por plataforma).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    /// "HD", "SSD", "NVMe", "USB", "Disco" ou "Imagem".
    pub kind: String,
    pub removable: bool,
    pub model: Option<String>,
    pub mounted_at: Vec<String>,
    /// O processo atual consegue abrir o dispositivo para leitura bruta.
    pub readable: bool,
    /// SSD com descarte (TRIM) ativo: arquivos apagados tendem a sumir de verdade.
    pub trim: Option<bool>,
}

/// (dispositivo, ponto de montagem)
pub fn mount_table() -> Vec<(String, String)> {
    #[cfg(target_os = "linux")]
    {
        let Ok(s) = std::fs::read_to_string("/proc/mounts") else {
            return vec![];
        };
        s.lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let d = it.next()?;
                let m = it.next()?;
                d.starts_with("/dev/")
                    .then(|| (d.to_string(), m.replace("\\040", " ")))
            })
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        let Ok(o) = std::process::Command::new("mount").output() else {
            return vec![];
        };
        let mut v = vec![];
        for l in String::from_utf8_lossy(&o.stdout).lines() {
            if let Some((d, rest)) = l.split_once(" on ") {
                if d.starts_with("/dev/") {
                    let m = rest.split(" (").next().unwrap_or("").to_string();
                    v.push((d.to_string(), m.clone()));
                    v.push((d.replace("/dev/disk", "/dev/rdisk"), m));
                }
            }
        }
        v
    }
    #[cfg(windows)]
    {
        vec![]
    }
}

pub fn list_devices() -> Vec<DeviceInfo> {
    #[cfg(target_os = "linux")]
    return linux();
    #[cfg(target_os = "macos")]
    return macos();
    #[cfg(windows)]
    return windows();
}

#[cfg(target_os = "linux")]
fn linux() -> Vec<DeviceInfo> {
    use std::fs;
    let mounts = mount_table();
    let mut out = vec![];
    let Ok(rd) = fs::read_dir("/sys/block") else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if ["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let base = e.path();
        let rd_str = |p: &str| {
            fs::read_to_string(base.join(p))
                .ok()
                .map(|s| s.trim().to_string())
        };
        let sectors: u64 = rd_str("size").and_then(|s| s.parse().ok()).unwrap_or(0);
        if sectors == 0 {
            continue;
        }
        let removable = rd_str("removable").as_deref() == Some("1");
        let rotational = rd_str("queue/rotational").as_deref() == Some("1");
        let trim = rd_str("queue/discard_max_bytes")
            .and_then(|s| s.parse::<u64>().ok())
            .map(|v| v > 0);
        let path = format!("/dev/{name}");
        let kind = if removable {
            "USB"
        } else if name.starts_with("nvme") {
            "NVMe"
        } else if rotational {
            "HD"
        } else {
            "SSD"
        };
        let model = rd_str("device/model").or_else(|| rd_str("device/name"));
        let mounted_at = mounts
            .iter()
            .filter(|(d, _)| {
                d.starts_with(&path)
                    && d[path.len()..]
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_digit() || c == 'p')
                        .unwrap_or(true)
            })
            .map(|(_, m)| m.clone())
            .collect();
        let readable = fs::File::open(&path).is_ok();
        out.push(DeviceInfo {
            path,
            name: model.clone().unwrap_or_else(|| name.clone()),
            size: sectors * 512,
            kind: kind.into(),
            removable,
            model,
            mounted_at,
            readable,
            trim: if rotational { None } else { trim },
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(target_os = "macos")]
fn macos() -> Vec<DeviceInfo> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir("/dev") else {
        return out;
    };
    let mounts = mount_table();
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        let Some(num) = n.strip_prefix("disk") else {
            continue;
        };
        if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let info = std::process::Command::new("diskutil")
            .args(["info", &n])
            .output()
            .ok();
        let text = info
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        let field = |k: &str| {
            text.lines().find_map(|l| {
                l.trim()
                    .strip_prefix(k)
                    .map(|v| v.trim().trim_start_matches(':').trim().to_string())
            })
        };
        let size = field("Disk Size:")
            .and_then(|v| {
                v.split('(')
                    .nth(1)
                    .and_then(|s| s.split_whitespace().next().map(String::from))
            })
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let ssd = field("Solid State:").map(|v| v == "Yes").unwrap_or(false);
        let removable = field("Removable Media:")
            .map(|v| v != "Fixed" && v != "No")
            .unwrap_or(false)
            || field("Protocol:").map(|v| v == "USB").unwrap_or(false);
        let model = field("Device / Media Name:");
        let path = format!("/dev/r{n}");
        let readable = std::fs::File::open(&path).is_ok();
        let mounted_at = mounts
            .iter()
            .filter(|(d, _)| d.starts_with(&format!("/dev/{n}")))
            .map(|(_, m)| m.clone())
            .collect();
        out.push(DeviceInfo {
            path,
            name: model.clone().unwrap_or(n.clone()),
            size,
            kind: if removable {
                "USB"
            } else if ssd {
                "SSD"
            } else {
                "HD"
            }
            .into(),
            removable,
            model,
            mounted_at,
            readable,
            trim: ssd.then_some(true),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(windows)]
fn windows() -> Vec<DeviceInfo> {
    let mut out = vec![];
    for i in 0..32 {
        let path = format!(r"\\.\PhysicalDrive{i}");
        match crate::source::open_windows_device(std::path::Path::new(&path)) {
            Ok(s) => {
                use crate::source::Source;
                out.push(DeviceInfo {
                    path,
                    name: format!("Disco {i}"),
                    size: s.size(),
                    kind: "Disco".into(),
                    removable: false,
                    model: None,
                    mounted_at: vec![],
                    readable: true,
                    trim: None,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => out.push(DeviceInfo {
                path,
                name: format!("Disco {i}"),
                size: 0,
                kind: "Disco".into(),
                removable: false,
                model: None,
                mounted_at: vec![],
                readable: false,
                trim: None,
            }),
            Err(_) => {}
        }
    }
    out
}
