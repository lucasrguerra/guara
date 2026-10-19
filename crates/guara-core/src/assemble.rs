//! Monta o disco virtual a partir do que o usuário informou: membros extras (RAID/LVM) e
//! senhas ou chaves para volumes cifrados. É a única porta de entrada para varrer e recuperar
//! discos que não são "simples".

use crate::crypt::{self, Secret, UnlockError};
use crate::error::{Error, Result};
use crate::partition;
use crate::source::{open_path, Source, SubSource};
use crate::types::{FsKind, PartInfo};
use crate::volume::{Overlay, Region};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SecretInput {
    Password {
        value: String,
    },
    Recovery {
        value: String,
    },
    /// Caminho de um arquivo de chave (.BEK do BitLocker, keyfile do LUKS).
    Keyfile {
        path: String,
    },
    /// Chave do volume em hexadecimal.
    Key {
        hex: String,
    },
}

/// Chave de volume guardada na sessão (só quando o usuário pede para lembrar).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedKey {
    pub start: u64,
    pub kind: String,
    pub key_hex: String,
}

/// RAID sem metadados: o que o usuário sabe; o resto é descoberto.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RaidSpec {
    pub level: Option<crate::raid::Level>,
    /// Tamanho da faixa em KiB.
    pub chunk_kb: Option<u64>,
    pub layout: Option<crate::raid::Layout>,
    /// Ordem dos discos: índice 0 é o alvo principal, depois `members` na ordem dada.
    pub order: Option<Vec<usize>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Assembly {
    /// Trata o alvo e `members` como discos de um RAID sem metadados (parâmetros descobertos).
    #[serde(default)]
    pub raid: Option<RaidSpec>,
    /// Outros discos ou imagens do mesmo conjunto (RAID/LVM), além do alvo principal.
    #[serde(default)]
    pub members: Vec<String>,
    /// Senhas e chaves: o `snapshot` da sessão as descarta antes de gravar.
    #[serde(default)]
    pub secrets: Vec<SecretInput>,
    /// Guarda as chaves de volume derivadas na sessão, para reabrir sem pedir a senha de novo.
    #[serde(default)]
    pub remember_keys: bool,
    #[serde(default)]
    pub keys: Vec<SavedKey>,
}

impl Assembly {
    pub fn is_plain(&self) -> bool {
        self.members.is_empty()
            && self.secrets.is_empty()
            && self.keys.is_empty()
            && self.raid.is_none()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UnlockInfo {
    pub start: u64,
    pub len: u64,
    pub kind: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LockedInfo {
    pub start: u64,
    pub kind: String,
    pub reason: String,
}

pub struct Assembled {
    pub source: Arc<dyn Source>,
    pub unlocked: Vec<UnlockInfo>,
    pub locked: Vec<LockedInfo>,
    /// Chaves derivadas, para guardar na sessão se o usuário permitir.
    pub keys: Vec<SavedKey>,
    /// Volumes lógicos / partições extras que não estão na tabela do disco.
    pub extra_parts: Vec<PartInfo>,
    /// Partições reais que são só contêineres (membros de RAID/LVM já remontados).
    pub covered: Vec<u64>,
    /// O disco principal é só um membro de RAID: não varrer o disco cru.
    pub skip_primary: bool,
    pub notes: Vec<String>,
}

fn to_secrets(a: &Assembly) -> Result<Vec<Secret>> {
    let mut v = vec![];
    for s in &a.secrets {
        v.push(match s {
            SecretInput::Password { value } => Secret::Password(value.clone()),
            SecretInput::Recovery { value } => Secret::RecoveryKey(value.clone()),
            SecretInput::Keyfile { path } => Secret::KeyFile(std::fs::read(path).map_err(|e| {
                Error::msg(format!(
                    "não foi possível ler o arquivo de chave {path}: {e}"
                ))
            })?),
            SecretInput::Key { hex } => Secret::VolumeKey(
                crypt::from_hex(hex)
                    .ok_or_else(|| Error::msg("chave do volume em hexadecimal inválida"))?,
            ),
        });
    }
    Ok(v)
}

fn kind_name(fs: FsKind) -> &'static str {
    match fs {
        FsKind::Bitlocker => "bitlocker",
        FsKind::Luks => "luks",
        FsKind::Corestorage => "filevault",
        _ => "?",
    }
}

fn try_unlock(window: Arc<dyn Source>, fs: FsKind, secrets: &[Secret]) -> crypt::UnlockResult {
    match fs {
        FsKind::Bitlocker => crypt::bitlocker::unlock(window, secrets),
        FsKind::Luks => crypt::luks::unlock(window, secrets),
        FsKind::Corestorage => crypt::filevault::unlock(window, secrets),
        _ => Err(UnlockError::Unsupported("tipo".into())),
    }
}

/// Abre `path` e aplica a montagem pedida.
pub fn assemble(path: &str, asm: &Assembly) -> Result<Assembled> {
    let base = open_path(Path::new(path)).map_err(|e| Error::msg(format!("{path}: {e}")))?;
    assemble_source(base, asm)
}

const MIB: u64 = 1 << 20;

/// Coloca `src` em `start`; se cair dentro de uma região virtual, a substitui por dentro.
fn patch(regions: &mut Vec<Region>, start: u64, src: Arc<dyn Source>, label: String) {
    let len = src.size();
    if let Some(r) = regions
        .iter_mut()
        .find(|r| r.start <= start && start < r.start + r.len)
    {
        let inner = Region {
            start: start - r.start,
            len,
            src,
            label: label.clone(),
        };
        let wrapped = Overlay::new(r.src.clone(), vec![inner]);
        if start == r.start {
            r.len = r.len.max(len);
        }
        r.src = Arc::new(wrapped);
        return;
    }
    regions.push(Region {
        start,
        len,
        src,
        label,
    });
}

/// Candidato a membro de RAID/LVM: uma janela de disco (inteiro ou partição).
struct Window {
    src: Arc<dyn Source>,
    /// Início no espaço do disco virtual, se a janela faz parte dele.
    primary_start: Option<u64>,
}

fn windows_of(src: &Arc<dyn Source>, primary: bool) -> Vec<Window> {
    let mut v = vec![Window {
        src: src.clone(),
        primary_start: primary.then_some(0),
    }];
    let layout = partition::analyze(src.as_ref(), false);
    for p in &layout.partitions {
        let len = if p.len == 0 {
            src.size().saturating_sub(p.start)
        } else {
            p.len
        };
        v.push(Window {
            src: Arc::new(SubSource::new(src.clone(), p.start, len)),
            primary_start: primary.then_some(p.start),
        });
    }
    v
}

pub fn assemble_source(base: Arc<dyn Source>, asm: &Assembly) -> Result<Assembled> {
    let secrets = to_secrets(asm)?;
    let mut regions: Vec<Region> = vec![];
    let mut out = Assembled {
        source: base.clone(),
        unlocked: vec![],
        locked: vec![],
        keys: vec![],
        extra_parts: vec![],
        covered: vec![],
        skip_primary: false,
        notes: vec![],
    };
    let mut members: Vec<Arc<dyn Source>> = vec![];
    for m in &asm.members {
        members.push(open_path(Path::new(m)).map_err(|e| Error::msg(format!("{m}: {e}")))?);
    }
    let mut next_virtual = base.size().div_ceil(MIB) * MIB + MIB;
    if let Some(spec) = &asm.raid {
        let mut disks: Vec<Arc<dyn Source>> = vec![base.clone()];
        disks.extend(members.iter().cloned());
        let params = pick_raid(&disks, spec, &mut out.notes)?;
        let src: Arc<dyn Source> =
            Arc::new(crate::raid::RaidSource::new(params).map_err(Error::msg)?);
        out.notes.push(format!(
            "RAID montado a partir de {} disco(s) sem metadados.",
            disks.len()
        ));
        out.skip_primary = true;
        members.clear();
        add_virtual(
            &mut regions,
            &mut next_virtual,
            &mut out,
            src,
            "RAID".into(),
        );
    }
    let mut done_containers: std::collections::BTreeSet<String> = Default::default();
    let mut tried_pool = false;
    // Rodadas: o que foi aberto/montado pode conter outro volume cifrado, RAID ou LVM.
    for round in 0..4 {
        let current: Arc<dyn Source> = Arc::new(Overlay::new(base.clone(), regions.clone()));
        let layout = partition::analyze(current.as_ref(), false);
        let mut all_parts: Vec<PartInfo> = layout.partitions.clone();
        for p in &out.extra_parts {
            if !all_parts.iter().any(|q| q.start == p.start) {
                all_parts.push(p.clone());
            }
        }
        let mut progress = false;

        // 1) RAID e LVM: reúne janelas do disco principal e dos membros extras.
        if round == 0 || !tried_pool || !out.extra_parts.is_empty() {
            let mut pool: Vec<Window> = vec![];
            pool.push(Window {
                src: current.clone(),
                primary_start: Some(0),
            });
            for p in &all_parts {
                if matches!(p.fs, FsKind::Raid | FsKind::Lvm) {
                    let len = if p.len == 0 {
                        current.size().saturating_sub(p.start)
                    } else {
                        p.len
                    };
                    pool.push(Window {
                        src: Arc::new(SubSource::new(current.clone(), p.start, len)),
                        primary_start: Some(p.start),
                    });
                }
            }
            for m in &members {
                pool.extend(windows_of(m, false));
            }
            tried_pool = true;
            // RAID
            let mut md: Vec<(Arc<dyn Source>, crate::raid::MdSuper)> = vec![];
            let mut md_primary: Vec<u64> = vec![];
            for w in &pool {
                if let Some(sup) = crate::raid::read_md_super(w.src.as_ref()) {
                    if let Some(st) = w.primary_start {
                        md_primary.push(st);
                    }
                    md.push((w.src.clone(), sup));
                }
            }
            let new_md: Vec<_> = md
                .into_iter()
                .filter(|(_, s)| {
                    !done_containers.contains(&format!("md{}", crypt::to_hex(&s.uuid)))
                })
                .collect();
            if !new_md.is_empty() {
                for (_, s) in &new_md {
                    done_containers.insert(format!("md{}", crypt::to_hex(&s.uuid)));
                }
                for res in crate::raid::assemble_md(new_md) {
                    match res {
                        Ok(a) => {
                            let label = format!(
                                "{} {}",
                                a.info.level.name(),
                                if a.name.is_empty() {
                                    a.uuid.clone()
                                } else {
                                    a.name.clone()
                                }
                            );
                            if a.missing > 0 {
                                out.notes.push(format!(
                                    "{label}: montado sem {} membro(s) (modo degradado).",
                                    a.missing
                                ));
                            } else {
                                out.notes.push(format!("{label} montado."));
                            }
                            add_virtual(
                                &mut regions,
                                &mut next_virtual,
                                &mut out,
                                a.source.clone(),
                                label,
                            );
                            progress = true;
                        }
                        Err(e) => out.notes.push(format!("RAID não montado: {e}")),
                    }
                }
                out.covered.extend(md_primary);
            }
            // LVM
            let mut pvs: Vec<(Arc<dyn Source>, crate::lvm::PvInfo)> = vec![];
            let mut pv_primary: Vec<u64> = vec![];
            let md_srcs: Vec<Arc<dyn Source>> = regions.iter().map(|r| r.src.clone()).collect();
            let mut lvm_pool: Vec<(Arc<dyn Source>, Option<u64>)> = pool
                .iter()
                .map(|w| (w.src.clone(), w.primary_start))
                .collect();
            for (r, s) in regions.iter().zip(md_srcs) {
                lvm_pool.push((s, Some(r.start)));
            }
            for (src, st) in lvm_pool {
                if let Some(pv) = crate::lvm::read_pv(src.as_ref()) {
                    if !done_containers.insert(format!("pv{}", pv.uuid)) {
                        continue;
                    }
                    if let Some(st) = st {
                        pv_primary.push(st);
                    }
                    pvs.push((src, pv));
                }
            }
            if !pvs.is_empty() {
                for vg in crate::lvm::assemble_vgs(pvs) {
                    for w in &vg.warnings {
                        out.notes.push(format!("LVM {}: {w}", vg.name));
                    }
                    for (name, lv) in vg.lvs {
                        add_virtual(
                            &mut regions,
                            &mut next_virtual,
                            &mut out,
                            lv,
                            format!("LVM {}/{}", vg.name, name),
                        );
                        progress = true;
                    }
                }
                out.covered.extend(pv_primary);
            }
        }

        // 2) Desbloqueio de volumes cifrados.
        let current: Arc<dyn Source> = Arc::new(Overlay::new(base.clone(), regions.clone()));
        let layout = partition::analyze(current.as_ref(), false);
        let mut all_parts: Vec<PartInfo> = layout.partitions.clone();
        for p in &out.extra_parts {
            if !all_parts.iter().any(|q| q.start == p.start) {
                all_parts.push(p.clone());
            }
        }
        out.locked.clear();
        for p in &all_parts {
            if p.encrypted.is_none() || out.unlocked.iter().any(|u| u.start == p.start) {
                continue;
            }
            let len = if p.len == 0 {
                current.size().saturating_sub(p.start)
            } else {
                p.len
            };
            let window: Arc<dyn Source> = Arc::new(SubSource::new(current.clone(), p.start, len));
            let mut all = secrets.clone();
            for k in asm.keys.iter().filter(|k| k.start == p.start) {
                if let Some(b) = crypt::from_hex(&k.key_hex) {
                    all.push(Secret::VolumeKey(b));
                }
            }
            match try_unlock(window, p.fs, &all) {
                Ok(u) => {
                    let ulen = u.source.size();
                    out.unlocked.push(UnlockInfo {
                        start: p.start,
                        len: ulen,
                        kind: kind_name(p.fs).into(),
                        description: u.description.clone(),
                    });
                    if let Some(k) = &u.volume_key {
                        out.keys.push(SavedKey { start: p.start, kind: kind_name(p.fs).into(), key_hex: crypt::to_hex(k) });
                    }
                    patch(&mut regions, p.start, u.source, u.description);
                    progress = true;
                }
                Err(e) => out.locked.push(LockedInfo {
                    start: p.start,
                    kind: kind_name(p.fs).into(),
                    reason: match e {
                        UnlockError::WrongSecret if secrets.is_empty() && asm.keys.is_empty() => "volume cifrado: informe a senha, a chave de recuperação ou o arquivo de chave".into(),
                        other => other.to_string(),
                    },
                }),
            }
        }
        if !progress {
            break;
        }
    }
    if !regions.is_empty() {
        out.source = Arc::new(Overlay::new(base, regions));
        // Atualiza o que se sabe sobre as partições virtuais (algumas foram desbloqueadas depois).
        for p in &mut out.extra_parts {
            if let Some((fs, len)) = partition::detect_fs(out.source.as_ref(), p.start) {
                p.fs = fs;
                p.encrypted = fs.encrypted().map(String::from);
                if let Some(l) = len {
                    p.len = p.len.min(l).max(1);
                }
            }
        }
    }
    out.covered.sort_unstable();
    out.covered.dedup();
    Ok(out)
}

/// Anexa um volume virtual depois do fim do disco e registra as partições que ele contém.
fn add_virtual(
    regions: &mut Vec<Region>,
    next: &mut u64,
    out: &mut Assembled,
    src: Arc<dyn Source>,
    label: String,
) {
    let start = *next;
    let len = src.size();
    if len == 0 {
        return;
    }
    *next = (start + len).div_ceil(MIB) * MIB + MIB;
    let inner = partition::analyze(src.as_ref(), false);
    if inner.partitions.is_empty() {
        out.extra_parts.push(PartInfo {
            index: 0,
            start,
            len,
            fs: FsKind::Unknown,
            label: Some(label.clone()),
            lost: false,
            encrypted: None,
            table_type: Some(label.clone()),
        });
    } else {
        for p in inner.partitions {
            let mut q = p.clone();
            q.start += start;
            q.label = q.label.or_else(|| Some(label.clone()));
            out.extra_parts.push(q);
        }
    }
    regions.push(Region {
        start,
        len,
        src,
        label,
    });
}

/// Escolhe os parâmetros do RAID: o que o usuário deu manda; o resto vem da detecção.
fn pick_raid(
    disks: &[Arc<dyn Source>],
    spec: &RaidSpec,
    notes: &mut Vec<String>,
) -> Result<crate::raid::RaidParams> {
    use crate::raid::{Member, RaidParams};
    if let (Some(level), Some(chunk), Some(layout), Some(order)) =
        (spec.level, spec.chunk_kb, spec.layout, spec.order.clone())
    {
        if order.len() == disks.len() && order.iter().all(|&i| i < disks.len()) {
            let members = order
                .iter()
                .map(|&i| {
                    Some(Member {
                        src: disks[i].clone(),
                        data_offset: 0,
                    })
                })
                .collect();
            return Ok(RaidParams::simple(level, chunk * 1024, layout, members));
        }
    }
    let cands = crate::raid_detect::detect(disks, &|| false);
    let c = cands
        .into_iter()
        .find(|c| {
            spec.level.map_or(true, |l| c.level == l)
                && spec.chunk_kb.map_or(true, |k| c.chunk == k * 1024)
                && spec.layout.map_or(true, |l| c.layout == l)
                && spec.order.as_ref().map_or(true, |o| *o == c.order)
        })
        .ok_or_else(|| Error::msg("não foi possível descobrir os parâmetros do RAID (nenhuma combinação produziu um sistema de arquivos coerente)"))?;
    notes.push(format!(
        "Parâmetros do RAID descobertos: {}, faixa de {} KiB, layout {:?}, ordem dos discos {:?} (pontuação {:.1}).",
        c.level.name(),
        c.chunk / 1024,
        c.layout,
        c.order,
        c.score
    ));
    Ok(c.params(disks))
}
