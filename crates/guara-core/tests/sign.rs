//! Assinatura digital: chaves, arquivos, adulteração, recuperação inteira e interoperabilidade.

use guara_core::recover;
use guara_core::sign::{self, SigningKey};
use guara_core::source::FileSource;
use guara_core::*;
use std::path::Path;

fn resident(id: u64, name: &str, bytes: Vec<u8>) -> Found {
    let ext = name.rsplit('.').next().unwrap_or("").to_string();
    Found {
        id,
        name: name.into(),
        dir: None,
        category: signatures::category_for_ext(&ext),
        ext,
        size: bytes.len() as u64,
        origin: Origin::Carved,
        confidence: Confidence::High,
        data: DataLoc::Resident { bytes },
        modified: None,
        created: None,
        accessed: None,
        taken: None,
        camera: None,
        px: None,
        gps: false,
        sig: None,
        note: None,
    }
}

/// `recover` assina sozinho quando `GUARA_SIGNING_KEY` existe; testes que recuperam não se misturam.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn keyed(dir: &Path) -> SigningKey {
    let k = SigningKey::generate();
    k.save(&dir.join("k.key")).unwrap();
    k
}

#[test]
fn key_roundtrip_and_no_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let k = keyed(dir.path());
    let again = SigningKey::load(&dir.path().join("k.key")).unwrap();
    assert_eq!(k.public_bytes(), again.public_bytes());
    assert_eq!(k.fingerprint(), again.fingerprint());
    assert_eq!(k.fingerprint().len(), 24); // 5 grupos de 4 + 4 separadores
    let pubfile = std::fs::read_to_string(dir.path().join("k.pub")).unwrap();
    assert!(pubfile.contains(&k.fingerprint()));
    let e = SigningKey::generate()
        .save(&dir.path().join("k.key"))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("já existe uma chave"), "{e}");
    // a chave antiga continua intacta
    assert_eq!(
        SigningKey::load(&dir.path().join("k.key"))
            .unwrap()
            .fingerprint(),
        k.fingerprint()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("k.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a chave privada precisa ser só do dono"
        );
    }
    std::fs::write(dir.path().join("lixo.key"), "outra coisa").unwrap();
    assert!(SigningKey::load(&dir.path().join("lixo.key")).is_err());
}

#[test]
fn signed_file_verifies_and_tampering_is_caught() {
    let dir = tempfile::tempdir().unwrap();
    let k = keyed(dir.path());
    let f = dir.path().join("relatorio.pdf");
    std::fs::write(&f, b"conteudo do relatorio").unwrap();
    let sig = sign::sign_file(&k, &f, Some("Perita Ana")).unwrap();
    assert_eq!(sig, sign::signature_path(&f));
    let c = sign::verify_file(&f, None, Some(&k.fingerprint())).unwrap();
    assert!(
        c.valid && c.trusted == Some(true) && c.signer.as_deref() == Some("Perita Ana"),
        "{c:?}"
    );
    // fingerprint aceito em outro formato (maiúsculas, sem dois pontos)
    let loose = k.fingerprint().replace(':', " ").to_uppercase();
    assert_eq!(
        sign::verify_file(&f, None, Some(&loose)).unwrap().trusted,
        Some(true)
    );

    // outra chave não é a esperada
    let other = SigningKey::generate().fingerprint();
    let c = sign::verify_file(&f, None, Some(&other)).unwrap();
    assert!(!c.valid && c.trusted == Some(false));

    // arquivo alterado
    std::fs::write(&f, b"conteudo ALTERADO do relatorio").unwrap();
    let c = sign::verify_file(&f, None, None).unwrap();
    assert!(!c.valid && !c.file_matches && c.signature_valid, "{c:?}");
    std::fs::write(&f, b"conteudo do relatorio").unwrap();
    assert!(sign::verify_file(&f, None, None).unwrap().valid);

    // campos da assinatura alterados (hora, assinante, hash) invalidam
    let raw = std::fs::read_to_string(&sig).unwrap();
    for (from, to) in [
        ("Perita Ana", "Outra Pessoa"),
        ("\"assinado_em\": \"20", "\"assinado_em\": \"19"),
    ] {
        assert!(raw.contains(from), "{from}");
        std::fs::write(&sig, raw.replacen(from, to, 1)).unwrap();
        let c = sign::verify_file(&f, None, None).unwrap();
        assert!(!c.valid && !c.signature_valid, "{from}: {c:?}");
    }
    // trocar a chave pública pela de outra pessoa também não passa
    let mut j: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let evil = SigningKey::generate();
    j["chave_publica"] = evil
        .public_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .into();
    std::fs::write(&sig, serde_json::to_vec(&j).unwrap()).unwrap();
    assert!(!sign::verify_file(&f, None, None).unwrap().valid);
}

#[test]
fn recovery_folder_is_checked_file_by_file() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let k = keyed(dir.path());
    let src = dir.path().join("origem.img");
    std::fs::write(&src, b"x").unwrap();
    let fs = FileSource::open(&src).unwrap();
    let a = resident(1, "a.txt", b"primeiro arquivo".to_vec());
    let b = resident(2, "b.txt", b"segundo arquivo".to_vec());
    let out = dir.path().join("saida");
    let rep = recover::recover(&fs, &[&a, &b], &out, &|| false, &mut |_, _, _| {}).unwrap();
    assert_eq!(rep.recovered.len(), 2);
    let manifest = out.join("guara-manifesto.json");
    sign::sign_file(&k, &manifest, None).unwrap();

    let c = sign::verify_recovery(&out, Some(&k.fingerprint())).unwrap();
    assert!(c.valid && c.ok == 2 && c.files == 2, "{c:?}");

    let first = out.join(&rep.recovered[0].path);
    let second = out.join(&rep.recovered[1].path);
    std::fs::write(&first, b"adulterado").unwrap();
    std::fs::remove_file(&second).unwrap();
    let c = sign::verify_recovery(&out, None).unwrap();
    assert!(
        !c.valid && c.changed.len() == 1 && c.missing.len() == 1,
        "{c:?}"
    );

    // manifesto adulterado (hash trocado para acompanhar o arquivo alterado) quebra a assinatura
    let m = std::fs::read_to_string(&manifest).unwrap();
    let real = &rep.recovered[0].sha256;
    let fake = sign::hash_file(&first).unwrap().0;
    std::fs::write(&manifest, m.replace(real, &fake)).unwrap();
    let c = sign::verify_recovery(&out, None).unwrap();
    let sigcheck = c.manifest_signature.as_ref().unwrap();
    assert!(!sigcheck.valid && !sigcheck.file_matches, "{c:?}");
}

#[test]
fn manifest_paths_cannot_escape_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fora.txt"), b"segredo").unwrap();
    let out = dir.path().join("saida");
    std::fs::create_dir(&out).unwrap();
    let sha = sign::hash_file(&dir.path().join("fora.txt")).unwrap().0;
    std::fs::write(
        out.join("guara-manifesto.json"),
        serde_json::json!({ "relatorio": { "recovered": [ { "path": "../fora.txt", "sha256": sha } ] } }).to_string(),
    )
    .unwrap();
    let c = sign::verify_recovery(&out, None).unwrap();
    assert!(!c.valid && c.changed[0].contains("fora da pasta"), "{c:?}");
}

/// O mesmo que o `openssl` faria: a assinatura é Ed25519 padrão sobre a mensagem documentada.
#[test]
fn signature_verifies_with_openssl() {
    let Ok(v) = std::process::Command::new("openssl")
        .arg("version")
        .output()
    else {
        return;
    };
    if !v.status.success() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let k = keyed(dir.path());
    let f = dir.path().join("doc.txt");
    std::fs::write(&f, b"hello\n").unwrap();
    let sig = sign::sign_file(&k, &f, Some("Ana")).unwrap();
    let j: serde_json::Value = serde_json::from_slice(&std::fs::read(sig).unwrap()).unwrap();
    let unhex = |s: &str| -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    };
    let msg = format!(
        "{}\narquivo={}\ntamanho={}\nsha256={}\nassinado_em={}\nassinante=Ana\n",
        j["formato"].as_str().unwrap(),
        j["arquivo"].as_str().unwrap(),
        j["tamanho"],
        j["sha256"].as_str().unwrap(),
        j["assinado_em"].as_str().unwrap()
    );
    std::fs::write(dir.path().join("msg"), msg).unwrap();
    std::fs::write(
        dir.path().join("sig"),
        unhex(j["assinatura"].as_str().unwrap()),
    )
    .unwrap();
    let mut der = unhex("302a300506032b6570032100");
    der.extend(unhex(j["chave_publica"].as_str().unwrap()));
    std::fs::write(dir.path().join("pub.der"), der).unwrap();
    let r = std::process::Command::new("openssl")
        .current_dir(dir.path())
        .args([
            "pkeyutl", "-verify", "-pubin", "-inkey", "pub.der", "-keyform", "DER", "-rawin",
            "-in", "msg", "-sigfile", "sig",
        ])
        .output()
        .unwrap();
    // OpenSSL 1.1 não tem -rawin para Ed25519; só exige sucesso onde ele existe
    let text = String::from_utf8_lossy(&r.stdout).to_string() + &String::from_utf8_lossy(&r.stderr);
    if text.contains("Unknown option") || text.contains("unknown option") {
        return;
    }
    assert!(r.status.success(), "{text}");
}

/// Com chave configurada, a recuperação e o relatório saem assinados sem ninguém pedir.
#[test]
fn configured_key_signs_manifest_automatically() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let k = keyed(dir.path());
    std::env::set_var("GUARA_SIGNING_KEY", dir.path().join("k.key"));
    std::env::set_var("GUARA_CONFIG_DIR", dir.path().join("cfg"));
    let src = dir.path().join("origem.img");
    std::fs::write(&src, b"x").unwrap();
    let fs = FileSource::open(&src).unwrap();
    let a = resident(1, "a.txt", b"dados".to_vec());
    let out = dir.path().join("saida");
    recover::recover(&fs, &[&a], &out, &|| false, &mut |_, _, _| {}).unwrap();
    std::env::remove_var("GUARA_SIGNING_KEY");
    let c = sign::verify_recovery(&out, Some(&k.fingerprint())).unwrap();
    assert!(c.valid && c.manifest_signature.is_some(), "{c:?}");
}
