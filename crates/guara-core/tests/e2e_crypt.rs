//! Ponta a ponta: volume LUKS2 com ext4 dentro, arquivo apagado, senha informada ao Guará.

mod common;
use common::*;
use guara_core::assemble::{assemble_source, Assembly, SecretInput};
use guara_core::scan::{self, Outcome};
use guara_core::source::{FileSource, Source};
use guara_core::*;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

fn have(t: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {t}")]).output().map(|o| o.status.success()).unwrap_or(false)
}

fn run_stdin(c: &mut Command, input: &str) -> bool {
    use std::io::Write;
    let mut ch = c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    ch.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    ch.wait().unwrap().success()
}

fn data(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

#[test]
fn deleted_file_inside_luks2_is_recovered_with_password() {
    if !(have("cryptsetup") && have("mke2fs") && have("debugfs") && have("python3")) {
        eprintln!("SKIP: ferramentas ausentes");
        return;
    }
    init();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("docs")).unwrap();
    let secret_doc = data(11, 120_000);
    std::fs::write(root.join("docs/segredo.dat"), &secret_doc).unwrap();
    std::fs::write(root.join("docs/fica.dat"), data(12, 5000)).unwrap();
    let plain = dir.path().join("plain.img");
    std::fs::File::create(&plain).unwrap().set_len(8 << 20).unwrap();
    assert!(Command::new("mke2fs").args(["-q", "-F", "-t", "ext4", "-d"]).arg(&root).arg(&plain).status().unwrap().success());
    let dbg = |c: &str| {
        Command::new("debugfs").arg("-w").arg("-R").arg(c).arg(&plain).output().unwrap()
    };
    dbg("rm /docs/segredo.dat");

    let img = dir.path().join("luks.img");
    std::fs::File::create(&img).unwrap().set_len(32 << 20).unwrap();
    let pw = "correct horse";
    let mut fmt = Command::new("cryptsetup");
    fmt.args(["luksFormat", "--batch-mode", "--type", "luks2", "--pbkdf", "pbkdf2", "--pbkdf-force-iterations", "1000", "--sector-size", "512", "--key-file", "-"]).arg(&img);
    assert!(run_stdin(&mut fmt, pw));
    let vkf = dir.path().join("vk");
    let mut d = Command::new("cryptsetup");
    d.args(["luksDump", "--dump-volume-key", "--batch-mode", "--key-file", "-", "--volume-key-file"]).arg(&vkf).arg(&img);
    assert!(run_stdin(&mut d, pw));
    let out = Command::new("cryptsetup").arg("luksDump").arg(&img).output().unwrap();
    let t = String::from_utf8_lossy(&out.stdout).to_string();
    let off: u64 = t.lines().skip_while(|l| !l.starts_with("Data segments")).find(|l| l.contains("offset:")).unwrap().split(':').nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
    assert!(Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/luks_encrypt.py"))
        .arg(&img).arg(&plain).arg(&vkf).arg(off.to_string()).arg("aes-xts-plain64").arg("512")
        .status().unwrap().success());

    let base: Arc<dyn Source> = Arc::new(FileSource::open(&img).unwrap());
    // Sem senha: o volume segue fechado e a varredura não inventa arquivos.
    let locked = assemble_source(base.clone(), &Assembly::default()).unwrap();
    assert_eq!(locked.unlocked.len(), 0);
    assert_eq!(locked.locked.len(), 1);

    let asm = Assembly { secrets: vec![SecretInput::Password { value: pw.into() }], ..Default::default() };
    let open = assemble_source(base, &asm).unwrap();
    assert_eq!(open.unlocked.len(), 1);
    let mut sink = Collect::default();
    let outc = scan::run(open.source.as_ref(), &ScanOptions { mode: ScanMode::Deep, ..Default::default() }, None, &[], &|| false, &mut sink);
    assert!(matches!(outc, Outcome::Completed));
    let f = sink.found.iter().find(|f| f.name == "segredo.dat").unwrap_or_else(|| panic!("não achou: {:?}", sink.found.iter().map(|f| &f.name).collect::<Vec<_>>()));
    assert_eq!(f.size, 120_000);
    let dest = tempfile::tempdir().unwrap();
    let refs = vec![f];
    let rep = recover::recover(open.source.as_ref(), &refs, dest.path(), &|| false, &mut |_, _, _| {}).unwrap();
    assert!(rep.failed.is_empty());
    let p = &rep.recovered[0].path;
    let p = if Path::new(p).is_absolute() { Path::new(p).to_path_buf() } else { dest.path().join(p) };
    assert!(std::fs::read(p).unwrap() == secret_doc);
}
