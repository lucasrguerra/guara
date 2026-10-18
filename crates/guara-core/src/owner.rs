//! Quando o Guará roda como administrador (sudo/pkexec), tudo que ele grava para
//! o usuário (sessões, relatórios, arquivos recuperados) pertence ao usuário que
//! o abriu, e não ao root, e as pastas sugeridas ficam na pasta desse usuário.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

/// O usuário real por trás de um processo root iniciado por sudo/pkexec.
#[cfg(unix)]
pub fn invoking_user() -> Option<User> {
    // SAFETY: geteuid não tem pré-condições.
    if unsafe { libc::geteuid() } != 0 {
        return None;
    }
    let uid = std::env::var("SUDO_UID")
        .ok()
        .or_else(|| std::env::var("PKEXEC_UID").ok())
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&u| u != 0);
    let name = std::env::var("SUDO_USER").ok().filter(|u| u != "root");
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    lookup_user(&passwd, uid, name.as_deref())
}

/// Procura o usuário (por uid ou, na falta dele, por nome) num conteúdo no formato do /etc/passwd.
pub fn lookup_user(passwd: &str, uid: Option<u32>, name: Option<&str>) -> Option<User> {
    for l in passwd.lines() {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() < 6 {
            continue;
        }
        let (Ok(fuid), Ok(fgid)) = (f[2].parse::<u32>(), f[3].parse::<u32>()) else {
            continue;
        };
        if uid == Some(fuid) || (uid.is_none() && name == Some(f[0])) {
            return Some(User {
                name: f[0].to_string(),
                uid: fuid,
                gid: fgid,
                home: PathBuf::from(f[5]),
            });
        }
    }
    None
}

#[cfg(not(unix))]
pub fn invoking_user() -> Option<User> {
    None
}

/// Pasta pessoal para sugerir ao usuário (a dele, mesmo se estivermos como root).
pub fn user_home() -> PathBuf {
    if let Some(u) = invoking_user() {
        return u.home;
    }
    PathBuf::from(
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default(),
    )
}

/// Entrega o arquivo/pasta ao usuário que abriu o Guará (não faz nada fora do modo root).
pub fn give_to_user(path: &Path) {
    #[cfg(unix)]
    if let Some(u) = invoking_user() {
        let _ = std::os::unix::fs::lchown(path, Some(u.uid), Some(u.gid));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Como `create_dir_all`, mas as pastas criadas ficam com o dono certo.
pub fn create_dir_all_owned(path: &Path) -> io::Result<()> {
    let mut missing = vec![];
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p.as_os_str().is_empty() || p.exists() {
            break;
        }
        missing.push(p.to_path_buf());
        cur = p.parent();
    }
    std::fs::create_dir_all(path)?;
    for p in missing.iter().rev() {
        give_to_user(p);
    }
    Ok(())
}

/// Como `fs::write`, mas o arquivo fica com o dono certo.
pub fn write_owned(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_dir_all_owned(parent)?;
    }
    std::fs::write(path, bytes)?;
    give_to_user(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\nlucas:x:1000:1000:Lucas:/home/lucas:/bin/bash\nmaria:x:1001:1002::/home/maria:/bin/zsh\nlinha-quebrada\n";

    #[test]
    fn acha_o_usuario_por_uid_ou_nome() {
        let u = lookup_user(PASSWD, Some(1000), None).unwrap();
        assert_eq!(
            (u.uid, u.gid, u.home.to_str().unwrap()),
            (1000, 1000, "/home/lucas")
        );
        let m = lookup_user(PASSWD, None, Some("maria")).unwrap();
        assert_eq!(
            (m.uid, m.gid, m.home.to_str().unwrap()),
            (1001, 1002, "/home/maria")
        );
        assert!(lookup_user(PASSWD, Some(4242), None).is_none());
        // o uid manda sobre o nome
        assert_eq!(
            lookup_user(PASSWD, Some(1001), Some("lucas")).unwrap().uid,
            1001
        );
    }

    #[test]
    fn fora_do_modo_root_nao_muda_dono_e_grava_normalmente() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a/b/sessao.json");
        write_owned(&f, b"{}").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"{}");
        assert!(create_dir_all_owned(&d.path().join("x/y")).is_ok());
    }
}
