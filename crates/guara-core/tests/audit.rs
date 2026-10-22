//! Trilha de auditoria: encadeamento por hash, detecção de adulteração e registro das ações do motor.

mod common;
use common::*;
use guara_core::audit::Log;
use guara_core::{service, ScanOptions};
use serde_json::json;

fn log_with(n: usize) -> (tempfile::TempDir, Log) {
    let dir = tempfile::tempdir().unwrap();
    let log = Log::at(dir.path().join("sub/auditoria.jsonl"));
    for i in 0..n {
        log.append("teste", json!({ "i": i, "caminho": "/dev/sdb" }))
            .unwrap();
    }
    (dir, log)
}

#[test]
fn entries_are_numbered_and_chained() {
    let (_d, log) = log_with(4);
    let e = log.entries().unwrap();
    assert_eq!(e.iter().map(|x| x.seq).collect::<Vec<_>>(), [1, 2, 3, 4]);
    assert_eq!(e[0].prev, "0".repeat(64));
    for w in e.windows(2) {
        assert_eq!(w[1].prev, w[0].hash);
    }
    assert!(e[0].time.ends_with('Z') && e[0].time.len() == 20);
    let v = log.verify().unwrap();
    assert!(v.ok && v.entries == 4 && v.head == e[3].hash);
    // continua de onde parou, inclusive numa instância nova
    let again = Log::at(log.path());
    assert_eq!(again.append("mais", json!({})).unwrap().seq, 5);
    assert!(again.verify().unwrap().ok);
}

#[test]
fn empty_or_missing_log_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let log = Log::at(dir.path().join("nada.jsonl"));
    let v = log.verify().unwrap();
    assert!(v.ok && v.entries == 0);
    assert!(log.entries().unwrap().is_empty());
}

fn tamper(log: &Log, f: impl FnOnce(&mut Vec<String>)) {
    let text = std::fs::read_to_string(log.path()).unwrap();
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    f(&mut lines);
    std::fs::write(log.path(), lines.join("\n") + "\n").unwrap();
}

#[test]
fn editing_a_line_breaks_the_chain_at_that_line() {
    let (_d, log) = log_with(5);
    tamper(&log, |l| l[2] = l[2].replace("/dev/sdb", "/dev/sdc"));
    let v = log.verify().unwrap();
    assert!(!v.ok);
    let (n, why) = v.broken_at.unwrap();
    assert_eq!(n, 3, "{why}");
    assert!(why.contains("hash"), "{why}");
}

#[test]
fn removing_or_reordering_lines_is_detected() {
    let (_d, log) = log_with(5);
    tamper(&log, |l| {
        l.remove(1);
    });
    let (n, why) = log.verify().unwrap().broken_at.unwrap();
    assert_eq!(n, 2, "{why}");

    let (_d, log) = log_with(5);
    tamper(&log, |l| l.swap(1, 2));
    assert_eq!(log.verify().unwrap().broken_at.unwrap().0, 2);

    // trocar o hash por outro também não passa: o seguinte deixa de continuar
    let (_d, log) = log_with(3);
    tamper(&log, |l| {
        let h = l[0].rsplit('"').nth(1).unwrap().to_string();
        l[0] = l[0].replace(&h, &"a".repeat(64));
    });
    assert_eq!(log.verify().unwrap().broken_at.unwrap().0, 1);
}

#[test]
fn garbage_line_is_reported_not_skipped() {
    let (_d, log) = log_with(3);
    tamper(&log, |l| l[1] = "isto não é json".into());
    let (n, why) = log.verify().unwrap().broken_at.unwrap();
    assert_eq!((n, why.as_str()), (2, "linha ilegível"));
}

#[test]
fn engine_actions_are_recorded_without_secrets() {
    init();
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("disco.img");
    std::fs::write(&img, vec![0u8; 2 << 20]).unwrap();
    let eng = service::Engine::new();
    let job = eng
        .start_scan(img.to_str().unwrap(), ScanOptions::default())
        .unwrap();
    wait(&eng, &job);
    let hash = eng.start_hash(img.to_str().unwrap()).unwrap();
    wait(&eng, &hash);
    let csv = dir.path().join("achados.csv");
    eng.export_csv(&job, csv.to_str().unwrap(), None).unwrap();
    eng.save_session(&job, dir.path().join("s.guara").to_str().unwrap())
        .unwrap();

    let log = Log::open_default();
    let e = log.entries().unwrap();
    let actions: Vec<&str> = e.iter().map(|x| x.action.as_str()).collect();
    for want in [
        "scan.start",
        "scan.finish",
        "hash.finish",
        "export.csv",
        "session.save",
    ] {
        assert!(actions.contains(&want), "{want} ausente em {actions:?}");
    }
    let start = e.iter().find(|x| x.action == "scan.start").unwrap();
    assert_eq!(start.detail["source"], img.to_str().unwrap());
    assert_eq!(start.detail["size"], 2 << 20);
    let h = e.iter().find(|x| x.action == "hash.finish").unwrap();
    assert_eq!(h.detail["sha256"].as_str().unwrap().len(), 64);
    assert!(log.verify().unwrap().ok);
    let all = std::fs::read_to_string(log.path()).unwrap();
    assert!(!all.contains("password") && !all.contains("senha"));
}

fn wait(eng: &service::Engine, job: &str) {
    for _ in 0..1000 {
        if eng.status(job).unwrap()["status"] != "running" {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("job não terminou");
}
