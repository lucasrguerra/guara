//! Btrfs: imagens criadas com mkfs.btrfs --rootdir; exclusão simulada como o COW faz
//! (nova versão da folha em outro bloco, a antiga continua no disco).

mod common;
use common::*;
use guara_core::fs::btrfs::{crc32c, live_paths, lzo1x_decompress};
use guara_core::source::FileSource;
use guara_core::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn have(t: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {t}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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

struct Img {
    _dir: tempfile::TempDir,
    path: PathBuf,
    files: Vec<(&'static str, Vec<u8>)>,
}

fn build() -> Option<Img> {
    if !have("mkfs.btrfs") {
        eprintln!("SKIP: mkfs.btrfs ausente");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("docs/sub")).unwrap();
    std::fs::create_dir_all(root.join("fotos")).unwrap();
    let files = vec![
        ("docs/relatorio.dat", data(1, 150_000)),
        ("docs/sub/contrato.dat", data(2, 9_000)),
        ("fotos/viagem.dat", data(3, 700_000)),
        ("fotos/fica.dat", data(4, 30_000)),
        ("pequeno.txt", b"conteudo pequeno inline".to_vec()),
    ];
    for (p, c) in &files {
        std::fs::write(root.join(p), c).unwrap();
    }
    let path = dir.path().join("b.img");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(128 << 20)
        .unwrap();
    let o = Command::new("mkfs.btrfs")
        .args(["-q", "-f", "--rootdir"])
        .arg(&root)
        .arg(&path)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    Some(Img {
        _dir: dir,
        path,
        files,
    })
}

#[test]
fn lists_the_live_tree_with_paths_and_sizes() {
    let Some(i) = build() else { return };
    let src = FileSource::open(&i.path).unwrap();
    let listed = live_paths(&src, 0);
    for (p, c) in &i.files {
        let f = listed
            .iter()
            .find(|x| x.0 == *p)
            .unwrap_or_else(|| panic!("{p} ausente: {listed:?}"));
        assert_eq!(f.1, c.len() as u64, "{p}");
        assert!(!f.2);
    }
    assert!(listed.iter().any(|x| x.0 == "docs/sub" && x.2));
}

// --- manipulação bruta de folhas (independente do módulo) ---

struct Leaf {
    raw: Vec<u8>,
}

impl Leaf {
    fn nritems(&self) -> usize {
        u32::from_le_bytes(self.raw[96..100].try_into().unwrap()) as usize
    }
    fn item(&self, i: usize) -> (u64, u8, u64, usize, usize) {
        let o = 101 + i * 25;
        let r = &self.raw;
        (
            u64::from_le_bytes(r[o..o + 8].try_into().unwrap()),
            r[o + 8],
            u64::from_le_bytes(r[o + 9..o + 17].try_into().unwrap()),
            u32::from_le_bytes(r[o + 17..o + 21].try_into().unwrap()) as usize,
            u32::from_le_bytes(r[o + 21..o + 25].try_into().unwrap()) as usize,
        )
    }
    fn data(&self, i: usize) -> &[u8] {
        let (_, _, _, off, size) = self.item(i);
        &self.raw[101 + off..101 + off + size]
    }
    fn seal(&mut self) {
        let c = crc32c(&self.raw[32..]);
        self.raw[0..4].copy_from_slice(&c.to_le_bytes());
    }
    /// Nova versão da folha sem os itens que `drop` rejeita, num novo endereço lógico.
    fn without(&self, drop: impl Fn(u64, u8) -> bool, new_bytenr: u64) -> Leaf {
        let mut kept: Vec<(usize, Vec<u8>)> = vec![];
        for i in 0..self.nritems() {
            let (obj, ty, _, _, _) = self.item(i);
            if !drop(obj, ty) {
                kept.push((i, self.data(i).to_vec()));
            }
        }
        let mut raw = vec![0u8; self.raw.len()];
        raw[..101].copy_from_slice(&self.raw[..101]);
        raw[48..56].copy_from_slice(&new_bytenr.to_le_bytes());
        let gen = u64::from_le_bytes(raw[80..88].try_into().unwrap()) + 1;
        raw[80..88].copy_from_slice(&gen.to_le_bytes());
        raw[96..100].copy_from_slice(&(kept.len() as u32).to_le_bytes());
        let mut end = raw.len() - 101;
        for (n, (i, d)) in kept.iter().enumerate() {
            let o = 101 + n * 25;
            raw[o..o + 17].copy_from_slice(&self.raw[101 + i * 25..101 + i * 25 + 17]);
            end -= d.len();
            raw[o + 17..o + 21].copy_from_slice(&(end as u32).to_le_bytes());
            raw[o + 21..o + 25].copy_from_slice(&(d.len() as u32).to_le_bytes());
            raw[101 + end..101 + end + d.len()].copy_from_slice(d);
        }
        let mut l = Leaf { raw };
        l.seal();
        l
    }
}

fn superblock(img: &[u8]) -> &[u8] {
    &img[0x10000..0x11000]
}

/// Mapa lógico -> físico dos chunks de sistema (suficiente para imagens pequenas de um dispositivo).
fn chunk_map(img: &[u8]) -> Vec<(u64, u64, u64)> {
    let sb = superblock(img);
    let n = u32::from_le_bytes(sb[160..164].try_into().unwrap()) as usize;
    let arr = &sb[0x32B..0x32B + n];
    let mut p = 0;
    let mut out = vec![];
    while p + 17 + 80 <= arr.len() {
        let logical = u64::from_le_bytes(arr[p + 9..p + 17].try_into().unwrap());
        let len = u64::from_le_bytes(arr[p + 17..p + 25].try_into().unwrap());
        let phys = u64::from_le_bytes(arr[p + 17 + 48 + 8..p + 17 + 48 + 16].try_into().unwrap());
        out.push((logical, len, phys));
        let ns = u16::from_le_bytes(arr[p + 17 + 44..p + 17 + 46].try_into().unwrap()) as usize;
        p += 17 + 48 + ns * 32;
    }
    out
}

#[test]
fn deleted_file_survives_in_the_previous_version_of_the_leaf() {
    let Some(i) = build() else { return };
    let mut img = std::fs::read(&i.path).unwrap();
    let sb = superblock(&img).to_vec();
    let nodesize = u32::from_le_bytes(sb[148..152].try_into().unwrap()) as usize;
    let src = FileSource::open(&i.path).unwrap();
    let t = guara_core::fs::btrfs::locate_trees(&src, 0).expect("árvores");
    assert_eq!(t.nodesize as usize, nodesize);
    let fs_leaf = (t.fs_tree as usize, t.fs_tree_logical);
    let root_leaf = (t.root_tree as usize, t.root_tree_logical);
    let ext_leaf = (t.extent_tree as usize, t.extent_tree_logical);
    let to_logical = |phys: usize| -> u64 {
        let c = t
            .chunks
            .iter()
            .find(|c| (phys as u64) >= c.2 && (phys as u64) < c.2 + c.1)
            .expect("chunk");
        c.0 + (phys as u64 - c.2)
    };
    let fl = Leaf {
        raw: img[fs_leaf.0..fs_leaf.0 + nodesize].to_vec(),
    };
    // inode do arquivo apagado
    let victim = "relatorio.dat";
    let mut ino = 0;
    for k in 0..fl.nritems() {
        let (obj, ty, _, _, _) = fl.item(k);
        if ty == 12 {
            let d = fl.data(k);
            let nl = u16::from_le_bytes([d[8], d[9]]) as usize;
            if &d[10..10 + nl] == victim.as_bytes() {
                ino = obj;
            }
        }
    }
    assert!(ino > 256);
    // extent de dados do arquivo (para liberar no extent tree)
    let mut data_bytenr = 0;
    for k in 0..fl.nritems() {
        let (obj, ty, _, _, _) = fl.item(k);
        if obj == ino && ty == 108 {
            data_bytenr = u64::from_le_bytes(fl.data(k)[21..29].try_into().unwrap());
        }
    }
    assert!(data_bytenr > 0);
    // Novo bloco livre para a nova versão: primeiro bloco do metadata sem folha válida e todo zero.
    let free_slot = (0..img.len() / nodesize)
        .map(|n| n * nodesize)
        .find(|&o| {
            o > fs_leaf.0
                && img[o..o + nodesize].iter().all(|&x| x == 0)
                && t.chunks
                    .iter()
                    .any(|c| (o as u64) >= c.2 && (o as u64) < c.2 + c.1)
                && o < fs_leaf.0 + (8 << 20)
        })
        .expect("bloco livre");
    // logical = bytenr da folha antiga + (offset físico - offset físico antigo): mesmo chunk
    let new_bytenr = to_logical(free_slot);
    let newleaf = fl.without(|o, _| o == ino, new_bytenr);
    img[free_slot..free_slot + nodesize].copy_from_slice(&newleaf.raw);
    // Aponta o ROOT_ITEM da árvore 5 para a nova versão (e sobe a geração), na folha da árvore raiz.
    let mut rl = Leaf {
        raw: img[root_leaf.0..root_leaf.0 + nodesize].to_vec(),
    };
    for k in 0..rl.nritems() {
        let (obj, ty, _, off, _) = rl.item(k);
        if obj == 5 && ty == 132 {
            let at = 101 + off + 176;
            rl.raw[at..at + 8].copy_from_slice(&new_bytenr.to_le_bytes());
        }
    }
    rl.seal();
    img[root_leaf.0..root_leaf.0 + nodesize].copy_from_slice(&rl.raw);
    // Libera o extent de dados no extent tree (em outro endereço da mesma folha).
    let el = Leaf {
        raw: img[ext_leaf.0..ext_leaf.0 + nodesize].to_vec(),
    };
    let newext = el.without(|o, ty| o == data_bytenr && ty == 168, ext_leaf.1);
    let _ = to_logical;
    img[ext_leaf.0..ext_leaf.0 + nodesize].copy_from_slice(&newext.raw);
    let _ = chunk_map(&img);

    let p2 = i._dir.path().join("deleted.img");
    std::fs::write(&p2, &img).unwrap();
    init();
    let c = scan_path(
        &p2,
        &ScanOptions {
            mode: ScanMode::Deep,
            ..Default::default()
        },
    );
    let f = c
        .found
        .iter()
        .find(|f| f.name == victim && f.origin == Origin::Btrfs)
        .unwrap_or_else(|| {
            panic!(
                "não achou: {:?}",
                c.found
                    .iter()
                    .map(|f| (&f.name, f.origin))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(f.size, 150_000);
    assert_eq!(f.dir.as_deref(), Some("/docs"));
    assert!(
        f.confidence >= Confidence::Medium && f.note.as_deref().unwrap_or("").contains("livres"),
        "{:?}",
        f.note
    );
    let out = i._dir.path().join("out");
    let rep = recover_all(&p2, std::slice::from_ref(f), &out);
    assert!(rep.failed.is_empty(), "{:?}", rep.failed);
    let rp = PathBuf::from(&rep.recovered[0].path);
    let got = std::fs::read(if rp.is_absolute() { rp } else { out.join(rp) }).unwrap();
    assert!(got == i.files[0].1);
    // Os arquivos que continuam existindo não aparecem como apagados.
    assert!(!c
        .found
        .iter()
        .any(|f| f.name == "fica.dat" && f.origin == Origin::Btrfs));
    let _ = Path::new("");
}

#[test]
fn lzo_literals_and_a_match() {
    // 20 literais + fim
    let mut s = vec![17 + 20];
    s.extend((0..20u8).map(|i| b'a' + i));
    s.extend_from_slice(&[0x11, 0, 0]);
    assert_eq!(
        lzo1x_decompress(&s, 4096).unwrap(),
        (0..20u8).map(|i| b'a' + i).collect::<Vec<_>>()
    );
    // "abcd" + M2 (dist 4, len 4) + fim
    let s = vec![17 + 4, b'a', b'b', b'c', b'd', 0x6C, 0x00, 0x11, 0, 0];
    assert_eq!(lzo1x_decompress(&s, 4096).unwrap(), b"abcdabcd");
}

#[test]
fn packed_extents_decode_zlib_and_zstd() {
    use guara_core::source::MemSource;
    use guara_core::{Packed, PackedCodec};
    use std::io::Write;
    let plain: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    // zlib
    let mut e = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
    e.write_all(&plain).unwrap();
    let z = e.finish().unwrap();
    let mut disk = vec![0u8; 4096];
    disk.extend_from_slice(&z);
    let p = Packed {
        offset: 4096,
        disk_len: z.len() as u64,
        ram_len: plain.len() as u64,
        skip: 1000,
        len: 5000,
        codec: PackedCodec::Zlib,
    };
    assert_eq!(p.read(&MemSource(disk)).unwrap(), &plain[1000..6000]);
    // zstd, se o utilitário existir
    if have("zstd") {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("p.bin");
        std::fs::write(&f, &plain).unwrap();
        assert!(Command::new("zstd")
            .args(["-q", "-f"])
            .arg(&f)
            .status()
            .unwrap()
            .success());
        let z = std::fs::read(dir.path().join("p.bin.zst")).unwrap();
        let p = Packed {
            offset: 0,
            disk_len: z.len() as u64,
            ram_len: plain.len() as u64,
            skip: 0,
            len: plain.len() as u64,
            codec: PackedCodec::Zstd,
        };
        assert!(p.read(&MemSource(z)).unwrap() == plain);
    }
}
