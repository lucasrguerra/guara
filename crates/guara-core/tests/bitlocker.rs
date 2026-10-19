//! BitLocker: imagens reais do conjunto de testes do cryptsetup (GitLab), com senhas conhecidas.
//! Baixe com `scripts/fetch-testdata.sh`; sem elas os testes são pulados.

use guara_core::crypt::{bitlocker, Secret, UnlockError};
use guara_core::partition::detect_fs_in;
use guara_core::source::{read_vec, FileSource, Source};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

fn dir() -> Option<PathBuf> {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/testdata/bitlk-images");
    d.join("images.conf").exists().then_some(d)
}

fn conf(d: &std::path::Path) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut m: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut cur = String::new();
    for l in std::fs::read_to_string(d.join("images.conf"))
        .unwrap()
        .lines()
    {
        let l = l.trim();
        if let Some(n) = l.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
            cur = n.to_string();
        } else if let Some((k, v)) = l.split_once('=') {
            m.entry(cur.clone()).or_default().insert(k.into(), v.into());
        }
    }
    m
}

fn check_fs(src: &dyn Source, what: &str) {
    // Imagens "encrypt-on-write" não têm sistema de arquivos reconhecível (UUID vazio no images.conf).
    if what.contains("-eow ") {
        return;
    }
    let head = read_vec(src, 0, 4096).unwrap();
    let fs = detect_fs_in(&head);
    assert!(
        fs.is_some(),
        "{what}: setor 0 aberto não é um sistema de arquivos: {:02x?}",
        &head[..32]
    );
}

#[test]
fn opens_all_reference_images_with_password_and_recovery_key() {
    let Some(d) = dir() else {
        eprintln!("SKIP: execute scripts/fetch-testdata.sh");
        return;
    };
    let mut opened = 0;
    for (name, c) in conf(&d) {
        let path = d.join(format!("{name}.img"));
        if !path.exists() {
            continue;
        }
        let src: Arc<dyn Source> = Arc::new(FileSource::open(&path).unwrap());
        let pw = c.get("PW").cloned().unwrap_or_default();
        let rp = c.get("RP").cloned().unwrap_or_default();
        // Proteção suspensa (chave limpa): abre sem segredo nenhum.
        if let Ok(u) = bitlocker::unlock(src.clone(), &[]) {
            check_fs(u.source.as_ref(), &format!("{name} (chave limpa)"));
            eprintln!("ok {name}: chave limpa");
            opened += 1;
            continue;
        }
        match bitlocker::unlock(src.clone(), &[Secret::Password("errada".into())]) {
            Err(UnlockError::WrongSecret) | Err(UnlockError::Unsupported(_)) => {}
            Err(e) => panic!("{name}: {e}"),
            Ok(_) => panic!("{name}: abriu com senha errada"),
        }
        let mut views = vec![];
        if !pw.is_empty() {
            let u = bitlocker::unlock(src.clone(), &[Secret::Password(pw.clone())])
                .unwrap_or_else(|e| panic!("{name} senha: {e}"));
            check_fs(u.source.as_ref(), &format!("{name} (senha)"));
            views.push(u);
        }
        if !rp.is_empty() {
            let u = bitlocker::unlock(src.clone(), &[Secret::RecoveryKey(rp.clone())])
                .unwrap_or_else(|e| panic!("{name} recuperação: {e}"));
            check_fs(u.source.as_ref(), &format!("{name} (recuperação)"));
            views.push(u);
        }
        if let Some(g) = c.get("SK_VMK_GUID") {
            let f = d.join(format!("{}.BEK", g.to_uppercase()));
            if f.exists() {
                let u =
                    bitlocker::unlock(src.clone(), &[Secret::KeyFile(std::fs::read(f).unwrap())])
                        .unwrap_or_else(|e| panic!("{name} BEK: {e}"));
                check_fs(u.source.as_ref(), &format!("{name} (BEK)"));
                views.push(u);
            }
        }
        assert!(!views.is_empty(), "{name}: nenhum protetor testável");
        let first = read_vec(views[0].source.as_ref(), 0, 1 << 20).unwrap();
        for v in &views[1..] {
            assert!(
                read_vec(v.source.as_ref(), 0, 1 << 20).unwrap() == first,
                "{name}: protetores divergem"
            );
        }
        opened += 1;
        eprintln!(
            "ok {name}: {} ({} protetores)",
            views[0].description,
            views.len()
        );
    }
    assert!(opened > 0);
}
