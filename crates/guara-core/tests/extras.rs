//! Saúde do disco, notificações, configurações e retomada da recuperação.

mod common;
use common::*;
use guara_core::*;
use serde_json::json;

// ------------------------------------------------------------------ saúde (SMART)

#[test]
fn smart_reprovado_e_setores_ruins_viram_alerta_serio() {
    let v = json!({
        "model_name": "WDC WD5000", "serial_number": "ABC123",
        "power_on_time": {"hours": 43000}, "temperature": {"current": 38},
        "smart_status": {"passed": false},
        "ata_smart_attributes": {"table": [
            {"id": 5, "raw": {"value": 120}}, {"id": 197, "raw": {"value": 3}}, {"id": 199, "raw": {"value": 0}}
        ]}
    });
    let h = health::parse_smartctl(&v);
    assert!(h.available);
    assert_eq!(h.status, "failing");
    assert!(h
        .details
        .iter()
        .any(|(k, v)| k == "Setores realocados" && v == "120"));
    assert!(h
        .details
        .iter()
        .any(|(k, _)| k == "Setores pendentes de realocação"));
    assert!(
        h.advice.iter().any(|a| a.contains("imagem")),
        "deve recomendar a imagem primeiro"
    );
}

#[test]
fn smart_saudavel_nvme_desgastado_e_nvme_critico() {
    let ok = json!({"smart_status": {"passed": true}, "ata_smart_attributes": {"table": [{"id": 5, "raw": {"value": 0}}, {"id": 197, "raw": {"value": 0}}]}});
    let h = health::parse_smartctl(&ok);
    assert_eq!(h.status, "ok");
    assert!(h.advice.is_empty());

    let worn = json!({"smart_status": {"passed": true}, "nvme_smart_health_information_log": {"critical_warning": 0, "percentage_used": 93, "media_errors": 0, "available_spare": 80, "available_spare_threshold": 10}});
    assert_eq!(health::parse_smartctl(&worn).status, "warning");

    let crit = json!({"smart_status": {"passed": true}, "nvme_smart_health_information_log": {"critical_warning": 4, "percentage_used": 10, "media_errors": 0}});
    assert_eq!(health::parse_smartctl(&crit).status, "failing");

    let media = json!({"nvme_smart_health_information_log": {"critical_warning": 0, "percentage_used": 10, "media_errors": 7}});
    assert_eq!(health::parse_smartctl(&media).status, "failing");
}

#[test]
fn smart_ssd_sata_traz_ficha_metricas_e_tabela() {
    let v = json!({
        "device": {"protocol": "ATA"},
        "model_name": "Samsung SSD 860 EVO 500GB", "firmware_version": "RVT04B6Q",
        "serial_number": "S3Z1NB0K", "sata_version": {"string": "SATA 3.2"},
        "interface_speed": {"max": {"string": "6.0 Gb/s"}, "current": {"string": "6.0 Gb/s"}},
        "user_capacity": {"bytes": 500107862016u64}, "logical_block_size": 512, "rotation_rate": 0,
        "smart_support": {"enabled": true}, "trim": {"supported": true},
        "smart_status": {"passed": true}, "temperature": {"current": 31},
        "power_on_time": {"hours": 12034}, "power_cycle_count": 811,
        "ata_smart_attributes": {"table": [
            {"id": 5, "name": "Reallocated_Sector_Ct", "value": 100, "worst": 100, "thresh": 10, "raw": {"value": 0, "string": "0"}},
            {"id": 177, "name": "Wear_Leveling_Count", "value": 92, "worst": 92, "thresh": 0, "raw": {"value": 41, "string": "41"}},
            {"id": 241, "name": "Total_LBAs_Written", "value": 99, "worst": 99, "thresh": 0, "raw": {"value": 2000000000u64, "string": "2000000000"}}
        ]}
    });
    let h = health::parse_smartctl(&v);
    assert_eq!(h.status, "ok");
    assert_eq!(h.health_pct, Some(92));
    assert_eq!(h.temperature_c, Some(31));
    assert_eq!(h.power_cycles, Some(811));
    assert_eq!(h.host_writes, Some(2_000_000_000 * 512));
    assert!(h
        .info
        .iter()
        .any(|(k, v)| k == "Capacidade" && v == "500,1 GB"));
    assert!(h
        .info
        .iter()
        .any(|(k, v)| k == "Recursos" && v.contains("TRIM")));
    assert_eq!(h.attributes.len(), 3);
    assert_eq!(h.attributes[0].id, "05");
    assert_eq!(h.attributes[0].name, "Reallocated Sector Ct");
    assert_eq!(h.attributes[1].id, "B1");
}

#[test]
fn smart_atributo_abaixo_do_limite_reprova_e_nvme_vira_tabela() {
    let v = json!({"smart_status": {"passed": true}, "ata_smart_attributes": {"table": [
        {"id": 1, "name": "Raw_Read_Error_Rate", "value": 40, "worst": 40, "thresh": 51, "raw": {"value": 9}}
    ]}});
    let h = health::parse_smartctl(&v);
    assert_eq!(h.attributes[0].status, "failing");
    assert_eq!(h.status, "failing");

    let nv = json!({"smart_status": {"passed": true}, "nvme_smart_health_information_log": {
        "critical_warning": 0, "temperature": 40, "available_spare": 100, "available_spare_threshold": 10,
        "percentage_used": 3, "data_units_read": 1000, "data_units_written": 2000,
        "power_cycles": 50, "power_on_hours": 900, "unsafe_shutdowns": 4, "media_errors": 0}});
    let h = health::parse_smartctl(&nv);
    assert_eq!(h.health_pct, Some(97));
    assert_eq!(h.host_writes, Some(2000 * 512_000));
    assert_eq!(h.power_cycles, Some(50));
    assert!(h
        .attributes
        .iter()
        .any(|a| a.id == "0D" && a.name == "Unsafe Shutdowns" && a.raw == "4"));
}

#[test]
fn imagem_de_disco_nao_tem_smart() {
    let f = tempfile::NamedTempFile::new().unwrap();
    let h = health::check(f.path().to_str().unwrap());
    assert!(!h.available);
    assert_eq!(h.status, "unknown");
}

// ------------------------------------------------------------------ configurações

#[test]
fn configuracoes_antigas_ganham_os_valores_padrao_novos() {
    init();
    // arquivo gravado pela v0.3: só tinha o tema
    std::fs::create_dir_all(settings::config_dir()).unwrap();
    std::fs::write(
        settings::config_dir().join("settings.json"),
        br#"{"theme":"dark"}"#,
    )
    .unwrap();
    let s = settings::load();
    assert_eq!(
        (s.theme.as_str(), s.lang.as_str(), s.notify, s.maps),
        ("dark", "auto", true, false)
    );
    settings::save(&settings::Settings {
        theme: "light".into(),
        lang: "en".into(),
        ui_lang: Some("en".into()),
        notify: false,
        maps: true,
        updates: false,
    })
    .unwrap();
    let s = settings::load();
    assert_eq!(
        (
            s.theme.as_str(),
            s.lang.as_str(),
            s.notify,
            s.maps,
            s.updates
        ),
        ("light", "en", false, true, false)
    );
}

// ------------------------------------------------------------------ notificações

#[cfg(target_os = "linux")]
#[test]
fn notificacao_chama_o_notify_send() {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    let marker = bin.path().join("recebido");
    let script = bin.path().join("notify-send");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s|' \"$@\" > '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{old}", bin.path().display()));
    notify::send(notify::Notice::ScanDone {
        count: 7,
        target: "disco".into(),
    });
    for _ in 0..100 {
        if marker.exists() && !std::fs::read(&marker).unwrap().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    std::env::set_var("PATH", old);
    let got = String::from_utf8(std::fs::read(&marker).unwrap()).unwrap();
    assert!(got.contains("Guará: "), "{got}");
    assert!(
        got.contains("7 arquivo(s) encontrados em disco.")
            || got.contains("7 file(s) found in disco."),
        "{got}"
    );
}

// ------------------------------------------------------------------ retomar recuperação

#[test]
fn recuperacao_interrompida_retoma_pulando_o_que_ja_foi_conferido() {
    let Some(dir) = fixtures() else { return };
    let img = dir.join("fat32.img");
    let res = scan_path(
        &img,
        &ScanOptions {
            mode: ScanMode::Quick,
            ..Default::default()
        },
    );
    assert_eq!(res.found.len(), 7);
    let src = source::open_path(&img).unwrap();
    let files: Vec<&Found> = res.found.iter().collect();
    let out = tempfile::tempdir().unwrap();
    let key = "disco-de-teste";

    // 1ª execução: "cai a energia" depois de 3 arquivos
    let n = std::sync::atomic::AtomicUsize::new(0);
    let cancel = || n.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 3;
    let r1 = recover::recover_resumable(
        src.as_ref(),
        &files,
        out.path(),
        recover::RecoverLayout::Folders,
        key,
        true,
        &cancel,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert!(r1.cancelled);
    assert_eq!(r1.recovered.len(), 3);
    let journal = std::fs::read_to_string(out.path().join(recover::JOURNAL)).unwrap();
    assert_eq!(journal.lines().count(), 3);

    // 2ª execução: retoma, pula os 3 e faz os outros 4
    let r2 = recover::recover_resumable(
        src.as_ref(),
        &files,
        out.path(),
        recover::RecoverLayout::Folders,
        key,
        true,
        &|| false,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert!(!r2.cancelled);
    assert_eq!((r2.skipped, r2.recovered.len()), (3, 7));
    let got = hashes_in(out.path());
    for (h, p) in expected(&dir, "fat32") {
        assert!(got.contains_key(&h), "faltou {p}");
    }
    assert_eq!(
        std::fs::read_to_string(out.path().join(recover::JOURNAL))
            .unwrap()
            .lines()
            .count(),
        7
    );

    // um arquivo é danificado entre as execuções: é refeito, os outros 6 são pulados
    let victim = out.path().join(&r2.recovered[0].path);
    std::fs::write(&victim, b"lixo").unwrap();
    let r3 = recover::recover_resumable(
        src.as_ref(),
        &files,
        out.path(),
        recover::RecoverLayout::Folders,
        key,
        true,
        &|| false,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!((r3.skipped, r3.recovered.len(), r3.failed.len()), (6, 7, 0));
    assert!(
        hashes_in(out.path()).contains_key(&r2.recovered[0].sha256),
        "o arquivo danificado deveria ter sido refeito"
    );

    // outro disco (outra chave) não reaproveita o diário
    let r4 = recover::recover_resumable(
        src.as_ref(),
        &files,
        out.path(),
        recover::RecoverLayout::Folders,
        "outro-disco",
        true,
        &|| false,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(r4.skipped, 0);
    let _ = json!({});
}
