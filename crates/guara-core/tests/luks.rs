//! LUKS1/LUKS2: imagens criadas pelo cryptsetup; o conteúdo é cifrado por uma
//! implementação independente (python cryptography) com a chave real do volume.

use guara_core::crypt::{luks, Secret, UnlockError};
use guara_core::source::{read_vec, FileSource, Source};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

fn have(tool: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn py_ok() -> bool {
    Command::new("python3")
        .args(["-c", "import cryptography"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_stdin(cmd: &mut Command, input: &str) -> bool {
    use std::io::Write;
    let mut ch = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    ch.stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    ch.wait().unwrap().success()
}

fn plain_data(len: usize) -> Vec<u8> {
    let mut x = 0x9E3779B97F4A7C15u64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 32) as u8
        })
        .collect()
}

struct Case<'a> {
    name: &'a str,
    format_args: Vec<&'a str>,
    cipher: &'a str,
    sector: u32,
    ver2: bool,
}

fn payload_offset(img: &Path, ver2: bool) -> u64 {
    let o = Command::new("cryptsetup")
        .arg("luksDump")
        .arg(img)
        .output()
        .unwrap();
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    if ver2 {
        let l = t
            .lines()
            .skip_while(|l| !l.starts_with("Data segments"))
            .find(|l| l.contains("offset:"))
            .unwrap();
        l.split(':')
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    } else {
        let l = t.lines().find(|l| l.starts_with("Payload offset")).unwrap();
        l.split(':').nth(1).unwrap().trim().parse::<u64>().unwrap() * 512
    }
}

fn run_case(c: &Case) {
    if !have("cryptsetup") || !py_ok() {
        eprintln!("SKIP: cryptsetup/python3-cryptography ausentes");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("l.img");
    let plain = dir.path().join("plain.bin");
    let data = plain_data(3 * 1024 * 1024 + 4096);
    std::fs::write(&plain, &data).unwrap();
    std::fs::File::create(&img)
        .unwrap()
        .set_len(24 << 20)
        .unwrap();
    let pw = "senha com acentuação ç";
    let mut fmt = Command::new("cryptsetup");
    fmt.args(["luksFormat", "--batch-mode", "--key-file", "-"]);
    fmt.args(&c.format_args);
    fmt.arg(&img);
    assert!(run_stdin(&mut fmt, pw), "luksFormat falhou ({})", c.name);
    let vkf = dir.path().join("vk.bin");
    let mut dump = Command::new("cryptsetup");
    dump.args([
        "luksDump",
        "--dump-volume-key",
        "--batch-mode",
        "--key-file",
        "-",
        "--volume-key-file",
    ]);
    dump.arg(&vkf).arg(&img);
    assert!(
        run_stdin(&mut dump, pw),
        "dump da chave falhou ({})",
        c.name
    );
    let off = payload_offset(&img, c.ver2);
    let st = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/luks_encrypt.py"))
        .arg(&img)
        .arg(&plain)
        .arg(&vkf)
        .arg(off.to_string())
        .arg(c.cipher)
        .arg(c.sector.to_string())
        .status()
        .unwrap();
    assert!(st.success());

    let src: Arc<dyn Source> = Arc::new(FileSource::open(&img).unwrap());
    // Senha errada primeiro.
    match luks::unlock(src.clone(), &[Secret::Password("errada".into())]) {
        Err(UnlockError::WrongSecret) => {}
        Err(e) => panic!("{}: erro inesperado {e}", c.name),
        Ok(_) => panic!("{}: abriu com senha errada", c.name),
    }
    let u = luks::unlock(
        src.clone(),
        &[Secret::Password("x".into()), Secret::Password(pw.into())],
    )
    .unwrap_or_else(|e| panic!("{}: {e}", c.name));
    let got = read_vec(u.source.as_ref(), 0, data.len()).unwrap();
    assert!(got == data, "{}: conteúdo difere", c.name);
    // Também abre com a chave do volume.
    let vk = std::fs::read(&vkf).unwrap();
    let u2 = luks::unlock(src, &[Secret::VolumeKey(vk)]).unwrap();
    assert_eq!(
        read_vec(u2.source.as_ref(), 1000, 5000).unwrap(),
        &data[1000..6000]
    );
}

#[test]
fn luks1_xts() {
    run_case(&Case {
        name: "luks1-xts",
        format_args: vec![
            "--type",
            "luks1",
            "--pbkdf-force-iterations",
            "1000",
            "-c",
            "aes-xts-plain64",
            "-s",
            "512",
        ],
        cipher: "aes-xts-plain64",
        sector: 512,
        ver2: false,
    });
}

#[test]
fn luks1_cbc_essiv_sha1() {
    run_case(&Case {
        name: "luks1-cbc-essiv",
        format_args: vec![
            "--type",
            "luks1",
            "--pbkdf-force-iterations",
            "1000",
            "-c",
            "aes-cbc-essiv:sha256",
            "-s",
            "256",
            "-h",
            "sha1",
        ],
        cipher: "aes-cbc-essiv:sha256",
        sector: 512,
        ver2: false,
    });
}

#[test]
fn luks2_argon2id_xts() {
    run_case(&Case {
        name: "luks2-argon2id",
        format_args: vec![
            "--type",
            "luks2",
            "--pbkdf",
            "argon2id",
            "--pbkdf-force-iterations",
            "4",
            "--pbkdf-memory",
            "64",
            "-c",
            "aes-xts-plain64",
            "-s",
            "512",
        ],
        cipher: "aes-xts-plain64",
        sector: 4096,
        ver2: true,
    });
}

#[test]
fn luks2_pbkdf2_sector512_aes256() {
    run_case(&Case {
        name: "luks2-pbkdf2",
        format_args: vec![
            "--type",
            "luks2",
            "--pbkdf",
            "pbkdf2",
            "--pbkdf-force-iterations",
            "1000",
            "-c",
            "aes-xts-plain64",
            "-s",
            "256",
            "--sector-size",
            "512",
        ],
        cipher: "aes-xts-plain64",
        sector: 512,
        ver2: true,
    });
}
