//! Tipos compartilhados entre núcleo, CLI e interface gráfica.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn label_pt(self) -> &'static str {
        match self {
            Confidence::High => "alta",
            Confidence::Medium => "média",
            Confidence::Low => "baixa",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Image,
    Document,
    Video,
    Audio,
    Archive,
    Database,
    Other,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Image,
        Category::Document,
        Category::Video,
        Category::Audio,
        Category::Archive,
        Category::Database,
        Category::Other,
    ];

    pub fn label_pt(self) -> &'static str {
        match self {
            Category::Image => "Fotos",
            Category::Document => "Documentos",
            Category::Video => "Vídeos",
            Category::Audio => "Áudio",
            Category::Archive => "Compactados",
            Category::Database => "Bancos de dados",
            Category::Other => "Outros",
        }
    }

    pub fn parse(s: &str) -> Option<Category> {
        Some(match s.to_ascii_lowercase().as_str() {
            "image" | "imagem" | "fotos" => Category::Image,
            "document" | "documento" | "documentos" => Category::Document,
            "video" | "vídeo" | "videos" | "vídeos" => Category::Video,
            "audio" | "áudio" => Category::Audio,
            "archive" | "compactados" => Category::Archive,
            "database" | "banco" => Category::Database,
            "other" | "outros" => Category::Other,
            _ => return None,
        })
    }
}

/// Trecho contíguo de bytes na origem (offset absoluto).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extent {
    pub offset: u64,
    pub len: u64,
    /// Trecho esparso: lido como zeros.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sparse: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Extent {
    pub fn new(offset: u64, len: u64) -> Self {
        Self {
            offset,
            len,
            sparse: false,
        }
    }
    pub fn end(&self) -> u64 {
        self.offset + self.len
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DataLoc {
    /// Dados dentro do próprio registro (NTFS residente).
    Resident {
        bytes: Vec<u8>,
    },
    Extents {
        extents: Vec<Extent>,
    },
    /// Arquivo montado: trechos em memória (cabeçalhos refeitos) e trechos do disco.
    Parts {
        parts: Vec<Part>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "lowercase")]
pub enum Part {
    Bytes {
        #[serde(with = "b64")]
        bytes: Vec<u8>,
    },
    /// Trecho contíguo da origem.
    Run { offset: u64, len: u64 },
}

impl Part {
    pub fn len(&self) -> u64 {
        match self {
            Part::Bytes { bytes } => bytes.len() as u64,
            Part::Run { len, .. } => *len,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

mod b64 {
    use serde::{Deserialize, Deserializer, Serializer};
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn serialize<S: Serializer>(b: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        s.serialize_str(&out)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        let mut out = vec![];
        let (mut acc, mut bits) = (0u32, 0);
        for c in s.bytes() {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => continue,
            } as u32;
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Ntfs,
    Fat,
    Exfat,
    Ext,
    Btrfs,
    Xfs,
    Hfs,
    Apfs,
    Refs,
    Carved,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Carved => "assinatura",
            Origin::Ntfs => "NTFS",
            Origin::Fat => "FAT",
            Origin::Exfat => "exFAT",
            Origin::Ext => "ext4",
            Origin::Btrfs => "Btrfs",
            Origin::Xfs => "XFS",
            Origin::Hfs => "HFS+",
            Origin::Apfs => "APFS",
            Origin::Refs => "ReFS",
        }
    }
}

/// Um arquivo encontrado (apagado, perdido ou reconstruído por assinatura).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Found {
    pub id: u64,
    pub name: String,
    /// Pasta de origem, quando o sistema de arquivos a preserva.
    pub dir: Option<String>,
    pub ext: String,
    pub category: Category,
    pub size: u64,
    pub origin: Origin,
    pub confidence: Confidence,
    pub data: DataLoc,
    /// Segundos desde 1970-01-01 UTC.
    pub modified: Option<i64>,
    #[serde(default)]
    pub created: Option<i64>,
    #[serde(default)]
    pub accessed: Option<i64>,
    /// Data em que a foto foi tirada (EXIF; hora da câmera lida como UTC), s desde 1970.
    #[serde(default)]
    pub taken: Option<i64>,
    #[serde(default)]
    pub camera: Option<String>,
    /// Largura × altura em pixels, quando conhecidas.
    #[serde(default)]
    pub px: Option<(u32, u32)>,
    /// Há coordenadas GPS nos metadados.
    #[serde(default)]
    pub gps: bool,
    /// Assinatura rápida do conteúdo (tamanho + início + fim) para achar duplicadas.
    #[serde(default)]
    pub sig: Option<u64>,
    pub note: Option<String>,
}

impl Found {
    /// Data mais representativa: quando foi tirada > modificada > criada.
    pub fn when(&self) -> Option<i64> {
        self.taken.or(self.modified).or(self.created)
    }
}

impl Found {
    pub fn first_offset(&self) -> Option<u64> {
        match &self.data {
            DataLoc::Extents { extents } => extents.iter().find(|e| !e.sparse).map(|e| e.offset),
            DataLoc::Resident { .. } => None,
            DataLoc::Parts { parts } => parts.iter().find_map(|p| match p {
                Part::Run { offset, .. } => Some(*offset),
                _ => None,
            }),
        }
    }

    pub fn full_path(&self) -> String {
        match &self.dir {
            Some(d) if !d.is_empty() => format!("{}/{}", d.trim_end_matches('/'), self.name),
            _ => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsKind {
    Ntfs,
    Fat12,
    Fat16,
    Fat32,
    Exfat,
    Ext,
    Apfs,
    HfsPlus,
    Btrfs,
    Xfs,
    Refs,
    Bitlocker,
    Luks,
    /// FileVault 2 (Core Storage).
    Corestorage,
    /// Membro de volume LVM (physical volume).
    Lvm,
    /// Membro de RAID por software (mdadm).
    Raid,
    Unknown,
}

impl FsKind {
    pub fn name(self) -> &'static str {
        match self {
            FsKind::Ntfs => "NTFS",
            FsKind::Fat12 => "FAT12",
            FsKind::Fat16 => "FAT16",
            FsKind::Fat32 => "FAT32",
            FsKind::Exfat => "exFAT",
            FsKind::Ext => "ext2/3/4",
            FsKind::Apfs => "APFS",
            FsKind::HfsPlus => "HFS+",
            FsKind::Btrfs => "Btrfs",
            FsKind::Xfs => "XFS",
            FsKind::Refs => "ReFS",
            FsKind::Bitlocker => "BitLocker",
            FsKind::Luks => "LUKS",
            FsKind::Corestorage => "FileVault 2",
            FsKind::Lvm => "LVM",
            FsKind::Raid => "RAID (mdadm)",
            FsKind::Unknown => "desconhecido",
        }
    }

    /// Volume cifrado: só detectamos (v1), sem desbloquear.
    pub fn encrypted(self) -> Option<&'static str> {
        match self {
            FsKind::Bitlocker => Some("BitLocker"),
            FsKind::Luks => Some("LUKS"),
            FsKind::Corestorage => Some("FileVault 2"),
            _ => None,
        }
    }

    /// Há leitor de metadados (arquivos apagados com nome) para este sistema.
    pub fn has_metadata_scanner(self) -> bool {
        matches!(
            self,
            FsKind::Ntfs | FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 | FsKind::Exfat | FsKind::Ext | FsKind::HfsPlus
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartInfo {
    pub index: usize,
    pub start: u64,
    pub len: u64,
    pub fs: FsKind,
    pub label: Option<String>,
    /// Encontrada por varredura, não consta da tabela de partições.
    pub lost: bool,
    pub encrypted: Option<String>,
    /// Descrição do tipo na tabela (ex.: "Microsoft basic data").
    pub table_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gap {
    pub start: u64,
    pub len: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiskLayout {
    pub size: u64,
    /// "gpt", "mbr" ou "none".
    pub table: String,
    pub partitions: Vec<PartInfo>,
    /// Espaço não particionado relevante (> 1 MiB).
    pub gaps: Vec<Gap>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanMode {
    /// Só metadados dos sistemas de arquivos (segundos).
    Quick,
    /// Metadados + carving por assinatura em todo espaço livre/desconhecido.
    Deep,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanOptions {
    pub mode: ScanMode,
    /// Inclui áreas ainda alocadas no carving (mais lento, acha cópias antigas).
    #[serde(default)]
    pub include_allocated: bool,
    /// Limita o carving a estas categorias (vazio = todas).
    #[serde(default)]
    pub categories: Vec<Category>,
    /// Tamanho máximo aceito para arquivos carved (bytes).
    #[serde(default = "default_max_carve")]
    pub max_carve_size: u64,
    /// Membros extras (RAID/LVM) e segredos para volumes cifrados.
    #[serde(default)]
    pub assembly: crate::assemble::Assembly,
    /// Partições que não estão na tabela do disco (volumes lógicos, desbloqueados).
    #[serde(default)]
    pub extra_parts: Vec<PartInfo>,
    /// Partições que só contêm RAID/LVM remontado: a varredura entra pelos volumes virtuais.
    #[serde(default)]
    pub covered: Vec<u64>,
    /// O disco principal é só membro de RAID sem metadados: varre apenas os volumes virtuais.
    #[serde(default)]
    pub skip_primary: bool,
    /// Vídeos íntegros da mesma câmera/drone: fornecem SPS/PPS para remontar MP4 sem moov.
    #[serde(default)]
    pub references: Vec<String>,
}

fn default_max_carve() -> u64 {
    8 * 1024 * 1024 * 1024
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            mode: ScanMode::Deep,
            include_allocated: false,
            categories: vec![],
            max_carve_size: default_max_carve(),
            assembly: Default::default(),
            extra_parts: vec![],
            covered: vec![],
            skip_primary: false,
            references: vec![],
        }
    }
}
