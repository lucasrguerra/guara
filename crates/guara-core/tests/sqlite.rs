//! Leitor de SQLite: esquema, linhas (com overflow e árvores de vários níveis),
//! registros apagados e robustez contra bancos danificados.

use guara_core::source::MemSource;
use guara_core::sqlite::{is_sqlite, Cell, Db};

const DB: &[u8] = include_bytes!("data/exemplo.sqlite");

fn text(c: &Cell) -> &str {
    match c {
        Cell::Text(t) => t,
        other => panic!("esperava texto, veio {other:?}"),
    }
}

#[test]
fn esquema_com_tabelas_indices_e_views() {
    let src = MemSource(DB.to_vec());
    assert!(is_sqlite(DB));
    let db = Db::open(&src).unwrap();
    let s = db.schema();
    assert_eq!(s.page_size, 1024);
    assert_eq!(s.encoding, "UTF-8");
    let names: Vec<_> = s.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["usuarios", "notas", "sem_rowid"]);
    let u = &s.tables[0];
    assert_eq!(
        u.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "nome", "email", "idade", "saldo", "foto"]
    );
    assert_eq!(u.columns[0].decl_type, "INTEGER");
    assert!(u.columns[0].pk && u.rowid_alias == Some(0));
    assert!(s.tables[2].without_rowid);
    assert_eq!(s.indexes, ["idx_usuarios_email"]);
    assert_eq!(s.views, ["maiores"]);
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    assert_eq!(db.count_rows(u), 493); // 500 menos 7 apagados
}

#[test]
fn linhas_em_arvore_de_varios_niveis_e_paginacao() {
    let src = MemSource(DB.to_vec());
    let db = Db::open(&src).unwrap();
    let p = db.rows("usuarios", 0, 5).unwrap();
    assert_eq!(p.total, 493);
    assert_eq!(p.rows.len(), 5);
    // o alias do rowid (INTEGER PRIMARY KEY) vem preenchido
    assert!(matches!(p.rows[0].cells[0], Cell::Int(1)));
    assert_eq!(text(&p.rows[0].cells[1]), "Pessoa 0001");
    assert_eq!(text(&p.rows[0].cells[2]), "pessoa1@exemplo.com");
    assert!(matches!(p.rows[0].cells[3], Cell::Int(19)));
    assert!(matches!(p.rows[1].cells[4], Cell::Real(r) if (r - 3.0).abs() < 1e-9));
    assert!(matches!(&p.rows[2].cells[5], Cell::Blob { len: 3, .. }));
    // pula os apagados 101..103: a linha de índice 100 é o id 104
    let q = db.rows("usuarios", 100, 2).unwrap();
    assert!(matches!(q.rows[0].cells[0], Cell::Int(104)));
    // última linha
    let last = db.rows("usuarios", 492, 10).unwrap();
    assert_eq!(last.rows.len(), 1);
    assert_eq!(text(&last.rows[0].cells[1]), "Pessoa 0500");
}

#[test]
fn texto_longo_com_overflow_e_acentos() {
    let src = MemSource(DB.to_vec());
    let db = Db::open(&src).unwrap();
    let n = db.rows("notas", 0, 10).unwrap();
    assert_eq!(n.rows.len(), 3);
    assert_eq!(text(&n.rows[0].cells[1]), "texto pequeno");
    assert_eq!(
        text(&n.rows[1].cells[1]).len(),
        "Lorem ipsum dolor sit amet. ".len() * 400
    );
    assert!(text(&n.rows[1].cells[1]).ends_with("amet. "));
    assert_eq!(text(&n.rows[2].cells[1]), "coração, ação, relatório");
    assert_eq!(text(&n.rows[2].cells[0]), "acentuação");
}

#[test]
fn recupera_registros_apagados_das_areas_livres() {
    let src = MemSource(DB.to_vec());
    let db = Db::open(&src).unwrap();
    let d = db.deleted_rows("usuarios").unwrap();
    let nomes: Vec<String> = d
        .rows
        .iter()
        .filter_map(|r| r.cells.get(1))
        .filter_map(|c| {
            if let Cell::Text(t) = c {
                Some(t.clone())
            } else {
                None
            }
        })
        .collect();
    let apagados = [
        "Pessoa 0101",
        "Pessoa 0102",
        "Pessoa 0103",
        "Pessoa 0250",
        "Pessoa 0251",
        "Pessoa 0252",
        "Pessoa 0253",
    ];
    let achados = apagados
        .iter()
        .filter(|n| nomes.iter().any(|x| x == *n))
        .count();
    assert!(
        achados >= 5,
        "só {achados} de 7 registros apagados recuperados: {nomes:?}"
    );
    assert!(d.rows.iter().all(|r| r.deleted));
    // a tabela de notas não teve nada apagado
    assert!(db.deleted_rows("notas").unwrap().rows.is_empty());
}

#[test]
fn banco_truncado_e_corrompido_nao_derruba_nem_trava() {
    // metade do arquivo
    let half = DB[..DB.len() / 2].to_vec();
    let src = MemSource(half);
    let db = Db::open(&src).unwrap();
    let s = db.schema();
    assert!(
        s.warnings.iter().any(|w| w.contains("truncado")),
        "{:?}",
        s.warnings
    );
    let p = db.rows("usuarios", 0, 1000).unwrap();
    assert!(!p.rows.is_empty() && p.rows.len() < 493, "{}", p.rows.len());
    // sem cabeçalho
    let mut bad = DB.to_vec();
    bad[0] = b'X';
    assert!(Db::open(&MemSource(bad)).is_err());
    // corrupção aleatória (semente fixa): nunca pode dar panic
    let mut x: u64 = 0x1234_5678_9abc_def0;
    for _ in 0..60 {
        let mut d = DB.to_vec();
        for _ in 0..40 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let i = 100 + (x as usize % (d.len() - 100));
            d[i] = (x >> 24) as u8;
        }
        let src = MemSource(d);
        if let Ok(db) = Db::open(&src) {
            let s = db.schema();
            for t in &s.tables {
                let _ = db.rows(&t.name, 0, 50);
                let _ = db.deleted_rows(&t.name);
                let _ = db.count_rows(t);
            }
        }
    }
}
