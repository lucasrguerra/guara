//! Imagens de disco virtual: QCOW2, VMDK, VHD, VHDX e DMG.
//!
//! Os arquivos de `tests/fixtures/vdisk` foram feitos por ferramentas reais (VirtualBox para
//! VMDK/VHD, qemu para QCOW2, hdiutil para DMG) e conferidos contra o conteúdo bruto. VHDX, DMG
//! sintético e discos diferenciais são montados aqui, byte a byte, a partir da especificação.
//! As imagens do dfvfs em target/testdata (scripts/fetch-testdata.sh) são conferidas por hash,
//! calculado com um leitor independente (7-Zip), e pulam quando ausentes.

use guara_core::source::{open_path, read_vec, Source};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

fn fx(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vdisk")
        .join(name)
}

fn raw() -> Vec<u8> {
    std::fs::read(fx("disk.raw")).unwrap()
}

fn dfvfs(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/testdata/dfvfs")
        .join(name);
    p.exists().then_some(p)
}

fn sha(src: &dyn Source) -> String {
    let mut h = Sha256::new();
    let mut off = 0;
    while off < src.size() {
        let n = (1 << 20).min((src.size() - off) as usize);
        h.update(read_vec(src, off, n).unwrap());
        off += n as u64;
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Lê o disco inteiro e também em pedaços que cruzam blocos, comparando com `want`.
fn check_content(path: &Path, want: &[u8]) {
    let s = open_path(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(s.size(), want.len() as u64, "{}", path.display());
    assert!(
        read_vec(s.as_ref(), 0, want.len()).unwrap() == want,
        "{}: conteúdo difere",
        path.display()
    );
    let mut x = 88172645463325252u64;
    for _ in 0..200 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let off = x % want.len() as u64;
        let len = 1 + (x >> 20) as usize % 70_000;
        let end = (off as usize + len).min(want.len());
        let got = read_vec(s.as_ref(), off, end - off as usize).unwrap();
        assert!(
            got == want[off as usize..end],
            "{}: @{off}+{len}",
            path.display()
        );
    }
    // leitura além do fim devolve só o que existe
    let mut buf = [0u8; 64];
    assert_eq!(s.read_at(want.len() as u64 - 10, &mut buf).unwrap(), 10);
    assert_eq!(s.read_at(want.len() as u64, &mut buf).unwrap(), 0);
}

// ---------- imagens feitas por ferramentas reais ----------

#[test]
fn virtualbox_vmdk_sparse_and_stream_optimized() {
    let want = raw();
    check_content(&fx("disk.vmdk"), &want);
    // streamOptimized: grãos comprimidos com zlib e cabeçalho no rodapé
    check_content(&fx("disk-stream.vmdk"), &want);
}

#[test]
fn virtualbox_vhd_dynamic() {
    check_content(&fx("disk.vhd"), &raw());
}

#[test]
fn qemu_qcow2_and_hdiutil_dmg_match_a_reference_reader() {
    let s = open_path(&fx("ext2.qcow2")).unwrap();
    assert_eq!(s.size(), 4 << 20);
    assert_eq!(
        sha(s.as_ref()),
        "a6c2f0e39afe6c6ab432ca5465349fcefe8dc944398e97b2d957d3f89dbb5d80"
    );
    // UDIF com chunks zlib, zerados e ignorados
    let d = open_path(&fx("hfsplus_zlib.dmg")).unwrap();
    assert_eq!(d.size(), 1_954_304);
    assert_eq!(
        sha(d.as_ref()),
        "c86d3616818e175caca2f6022f49e190e5b1cfe39b2e8f9373119e22aa7eaf58"
    );
}

// ---------- montagem de imagens sintéticas ----------

fn be(v: &mut [u8], o: usize, x: u64, n: usize) {
    v[o..o + n].copy_from_slice(&x.to_be_bytes()[8 - n..]);
}
fn le(v: &mut [u8], o: usize, x: u64, n: usize) {
    v[o..o + n].copy_from_slice(&x.to_le_bytes()[..n]);
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(vec![], flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// QCOW2 v3 com clusters de 64 KiB: zerados (sem entrada e com a flag), brutos e comprimidos.
fn build_qcow2(data: &[u8]) -> Vec<u8> {
    const CS: usize = 1 << 16;
    let clusters = data.len() / CS;
    let mut img = vec![0u8; 3 * CS];
    img[..4].copy_from_slice(b"QFI\xfb");
    be(&mut img, 4, 3, 4);
    be(&mut img, 20, 16, 4);
    be(&mut img, 24, data.len() as u64, 8);
    be(&mut img, 36, 1, 4);
    be(&mut img, 40, CS as u64, 8);
    be(&mut img, 100, 112, 4);
    be(&mut img, CS, (2 * CS as u64) | (1 << 63), 8); // L1 -> L2 no cluster 2
    for i in 0..clusters {
        let c = &data[i * CS..(i + 1) * CS];
        let entry: u64 = if c.iter().all(|&b| b == 0) {
            if i % 2 == 0 {
                0
            } else {
                1 // zerado explícito
            }
        } else if i % 3 == 0 {
            let z = deflate(c);
            let host = img.len() as u64;
            let sectors = z.len().div_ceil(512);
            img.extend_from_slice(&z);
            img.resize(img.len().div_ceil(512) * 512, 0);
            1 << 62 | ((sectors as u64 - 1) << (62 - 8)) | host
        } else {
            let host = img.len() as u64;
            img.extend_from_slice(c);
            host | 1 << 63
        };
        be(&mut img, 2 * CS + i * 8, entry, 8);
    }
    img
}

fn vhd_footer(size: u64, kind: u32, data_offset: u64) -> [u8; 512] {
    let mut f = [0u8; 512];
    f[..8].copy_from_slice(b"conectix");
    be(&mut f, 8, 2, 4);
    be(&mut f, 12, 0x0001_0000, 4);
    be(&mut f, 16, data_offset, 8);
    f[28..32].copy_from_slice(b"guar");
    be(&mut f, 40, size, 8);
    be(&mut f, 48, size, 8);
    be(&mut f, 60, kind as u64, 4);
    let sum = f.iter().fold(0u32, |a, &b| a.wrapping_add(b as u32));
    be(&mut f, 64, !sum as u64, 4);
    f
}

fn base64(d: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
        if s.len() % 76 == 0 {
            s.push('\n');
        }
    }
    s
}

/// ADC só com literais e cópias de 3 bytes (deslocamento 8), como o `hdiutil` produz.
fn adc_pattern(len: usize) -> (Vec<u8>, Vec<u8>) {
    let pat = *b"ADC-test";
    let mut enc = vec![0x80 | 7];
    enc.extend_from_slice(&pat);
    let mut made = 8;
    while made < len {
        let rem = len - made;
        let mut n = rem.min(67);
        if rem - n > 0 && rem - n < 3 {
            n = rem - 3;
        }
        if n >= 4 {
            enc.extend_from_slice(&[0x40 | (n as u8 - 4), 0, 7]);
        } else {
            enc.extend_from_slice(&[((n as u8 - 3) << 2), 7]); // 2 bytes, comprimento 3
        }
        made += n;
    }
    let want: Vec<u8> = (0..len).map(|i| pat[i % 8]).collect();
    (enc, want)
}

struct DmgChunk {
    ty: u32,
    sectors: u64,
    payload: Vec<u8>,
}

fn build_dmg(chunks: &[DmgChunk], total_sectors: u64) -> Vec<u8> {
    let mut fork = vec![];
    let mut table = vec![0u8; 204];
    table[..4].copy_from_slice(b"mish");
    be(&mut table, 4, 1, 4);
    be(&mut table, 16, total_sectors, 8);
    be(&mut table, 200, chunks.len() as u64 + 1, 4);
    let mut sector = 0;
    for c in chunks {
        let mut e = [0u8; 40];
        be(&mut e, 0, c.ty as u64, 4);
        be(&mut e, 8, sector, 8);
        be(&mut e, 16, c.sectors, 8);
        be(&mut e, 24, fork.len() as u64, 8);
        be(&mut e, 32, c.payload.len() as u64, 8);
        table.extend_from_slice(&e);
        fork.extend_from_slice(&c.payload);
        sector += c.sectors;
    }
    let mut end = [0u8; 40];
    be(&mut end, 0, 0xFFFF_FFFF, 4);
    be(&mut end, 8, sector, 8);
    table.extend_from_slice(&end);
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>resource-fork</key><dict>\
         <key>blkx</key><array><dict><key>Data</key><data>\n{}\n</data><key>Name</key><string>teste</string></dict></array>\
         <key>plst</key><array><dict><key>Data</key><data>AAAA</data></dict></array></dict></dict></plist>",
        base64(&table)
    );
    let mut img = fork;
    let xml_off = img.len() as u64;
    img.extend_from_slice(xml.as_bytes());
    let mut k = vec![0u8; 512];
    k[..4].copy_from_slice(b"koly");
    be(&mut k, 4, 4, 4);
    be(&mut k, 8, 512, 4);
    be(&mut k, 32, xml_off, 8);
    be(&mut k, 216, xml_off, 8);
    be(&mut k, 224, xml.len() as u64, 8);
    be(&mut k, 492, total_sectors, 8);
    img.extend_from_slice(&k);
    img
}

// ---------- QCOW2 ----------

#[test]
fn qcow2_with_zero_raw_and_compressed_clusters() {
    let want = raw();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.qcow2");
    std::fs::write(&p, build_qcow2(&want)).unwrap();
    check_content(&p, &want);
}

#[test]
fn qcow2_with_backing_file_or_encryption_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut img = build_qcow2(&raw());
    let p = dir.path().join("b.qcow2");
    be(&mut img, 8, 4096, 8);
    be(&mut img, 16, 3, 4);
    std::fs::write(&p, &img).unwrap();
    let e = open_path(&p).err().expect("deveria recusar").to_string();
    assert!(e.contains("arquivo de apoio"), "{e}");

    let mut img = build_qcow2(&raw());
    be(&mut img, 32, 2, 4);
    let p = dir.path().join("e.qcow2");
    std::fs::write(&p, &img).unwrap();
    let e = open_path(&p).err().expect("deveria recusar").to_string();
    assert!(e.contains("LUKS"), "{e}");
}

// ---------- VMDK ----------

#[test]
fn vmdk_descriptor_with_flat_and_zero_extents() {
    let want = raw();
    let dir = tempfile::tempdir().unwrap();
    let half = want.len() / 2;
    std::fs::write(dir.path().join("a-f001.vmdk"), &want[..half]).unwrap();
    std::fs::write(
        dir.path().join("a-f002.vmdk"),
        &want[half..want.len() - 4096],
    )
    .unwrap();
    let desc = format!(
        "# Disk DescriptorFile\nversion=1\nCID=fffffffe\nparentCID=ffffffff\ncreateType=\"twoGbMaxExtentFlat\"\n\n# Extent description\nRW {} FLAT \"a-f001.vmdk\" 0\nRW {} FLAT \"a-f002.vmdk\" 0\nRW 8 ZERO\n",
        half / 512,
        (want.len() - 4096 - half) / 512
    );
    let p = dir.path().join("a.vmdk");
    std::fs::write(&p, desc).unwrap();
    let mut expect = want.clone();
    expect[want.len() - 4096..].fill(0);
    check_content(&p, &expect);
}

#[test]
fn vmdk_with_a_parent_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("c.vmdk");
    std::fs::write(
        &p,
        "# Disk DescriptorFile\nversion=1\nparentCID=12345678\ncreateType=\"monolithicFlat\"\nRW 8 ZERO\n",
    )
    .unwrap();
    let e = open_path(&p).err().expect("deveria recusar").to_string();
    assert!(e.contains("disco pai"), "{e}");
}

// ---------- VHD ----------

#[test]
fn vhd_fixed_hides_the_footer() {
    let want = raw();
    let mut img = want.clone();
    img.extend_from_slice(&vhd_footer(want.len() as u64, 2, u64::MAX));
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f.vhd");
    std::fs::write(&p, img).unwrap();
    check_content(&p, &want);
}

/// VHD diferencial: a filha só traz o setor 100 e o bloco inteiro do fim; o resto vem do pai.
#[test]
fn vhd_differencing_reads_the_parent_for_missing_sectors() {
    let want = raw();
    let dir = tempfile::tempdir().unwrap();
    let mut parent = want.clone();
    parent.extend_from_slice(&vhd_footer(want.len() as u64, 2, u64::MAX));
    std::fs::write(dir.path().join("pai.vhd"), &parent).unwrap();

    const BLOCK: usize = 1 << 20; // um bloco só
    let mut f = vec![0u8; 512 + 1024 + 512];
    let hdr = vhd_footer(want.len() as u64, 4, 512);
    f[..512].copy_from_slice(&hdr);
    let mut dh = [0u8; 1024];
    dh[..8].copy_from_slice(b"cxsparse");
    be(&mut dh, 8, u64::MAX, 8);
    be(&mut dh, 16, 1536, 8); // BAT
    be(&mut dh, 28, 1, 4);
    be(&mut dh, 32, BLOCK as u64, 4);
    let name = "pai.vhd"
        .encode_utf16()
        .flat_map(|u| u.to_be_bytes())
        .collect::<Vec<u8>>();
    dh[64..64 + name.len()].copy_from_slice(&name);
    f[512..1536].copy_from_slice(&dh);
    be(&mut f, 1536, 4, 4); // bloco 0 no setor 4
    f.resize(4 * 512, 0);
    // bitmap (4096 setores = 512 bytes) com o setor 100 presente, depois os dados
    let mut bitmap = vec![0u8; 512];
    bitmap[100 / 8] |= 0x80 >> (100 % 8);
    f.extend_from_slice(&bitmap);
    let mut block = vec![0u8; BLOCK];
    let marker = [0xA5u8; 512];
    block[100 * 512..101 * 512].copy_from_slice(&marker);
    f.extend_from_slice(&block);
    f.extend_from_slice(&hdr);
    let p = dir.path().join("filho.vhd");
    std::fs::write(&p, f).unwrap();

    let mut expect = want.clone();
    expect[100 * 512..101 * 512].copy_from_slice(&marker);
    check_content(&p, &expect);

    // sem o pai a abertura falha com o motivo
    std::fs::remove_file(dir.path().join("pai.vhd")).unwrap();
    let e = open_path(&p).err().expect("deveria recusar").to_string();
    assert!(e.contains("disco pai não encontrado"), "{e}");
}

// ---------- VHDX ----------

fn guid(s: &str) -> [u8; 16] {
    let h: Vec<u8> = s
        .replace('-', "")
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    let mut g = [0u8; 16];
    g[..4].copy_from_slice(&[h[3], h[2], h[1], h[0]]);
    g[4..6].copy_from_slice(&[h[5], h[4]]);
    g[6..8].copy_from_slice(&[h[7], h[6]]);
    g[8..].copy_from_slice(&h[8..]);
    g
}

const MB: usize = 1 << 20;

/// VHDX com blocos de 1 MiB. `blocks[i]`: `None` = não presente, `Some((estado, dados))`.
/// Com `parent`, acrescenta o localizador de pai e o bitmap de setores (`bitmap`).
fn build_vhdx(
    size: u64,
    blocks: &[Option<(u64, Vec<u8>)>],
    parent: Option<&str>,
    bitmap: Option<Vec<u8>>,
) -> Vec<u8> {
    let mut img = vec![0u8; 4 * MB];
    img[..8].copy_from_slice(b"vhdxfile");
    img[64 * 1024..64 * 1024 + 4].copy_from_slice(b"head");
    img[128 * 1024..128 * 1024 + 4].copy_from_slice(b"head");
    // tabela de regiões: BAT em 2 MiB, metadados em 1 MiB
    let rt = 192 * 1024;
    img[rt..rt + 4].copy_from_slice(b"regi");
    le(&mut img, rt + 8, 2, 4);
    img[rt + 16..rt + 32].copy_from_slice(&guid("2DC27766-F623-4200-9D64-115E9BFD4A08"));
    le(&mut img, rt + 32, 2 * MB as u64, 8);
    le(&mut img, rt + 40, MB as u64, 4);
    img[rt + 48..rt + 64].copy_from_slice(&guid("8B7CA206-4790-4B9A-B8FE-575F050F886E"));
    le(&mut img, rt + 64, MB as u64, 8);
    le(&mut img, rt + 72, MB as u64, 4);
    // metadados
    let m = MB;
    img[m..m + 8].copy_from_slice(b"metadata");
    let mut items: Vec<([u8; 16], Vec<u8>)> = vec![];
    let mut params = vec![0u8; 8];
    le(&mut params, 0, MB as u64, 4);
    le(&mut params, 4, if parent.is_some() { 2 } else { 0 }, 4);
    items.push((guid("CAA16737-FA36-4D43-B3B6-33F0AA44E76B"), params));
    items.push((
        guid("2FA54224-CD1B-4876-B211-5DBED83BF4B8"),
        size.to_le_bytes().to_vec(),
    ));
    items.push((
        guid("8141BF1D-A96F-4709-BA47-F233A8FAAB5F"),
        512u32.to_le_bytes().to_vec(),
    ));
    if let Some(p) = parent {
        let key: Vec<u8> = "relative_path"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let val: Vec<u8> = p.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut loc = vec![0u8; 20 + 12];
        loc[..16].copy_from_slice(&guid("B04AEFB7-D19E-4A81-B789-25B8E9445913"));
        le(&mut loc, 18, 1, 2);
        let (ko, vo) = (32, 32 + key.len());
        le(&mut loc, 20, ko as u64, 4);
        le(&mut loc, 24, vo as u64, 4);
        le(&mut loc, 28, key.len() as u64, 2);
        le(&mut loc, 30, val.len() as u64, 2);
        loc.extend_from_slice(&key);
        loc.extend_from_slice(&val);
        items.push((guid("A8D35F2D-B30B-454D-ABF7-D3D84834AB0C"), loc));
    }
    le(&mut img, m + 10, items.len() as u64, 2);
    let mut at = 64 * 1024;
    for (i, (id, data)) in items.iter().enumerate() {
        let e = m + 32 + i * 32;
        img[e..e + 16].copy_from_slice(id);
        le(&mut img, e + 16, at as u64, 4);
        le(&mut img, e + 20, data.len() as u64, 4);
        img[m + at..m + at + data.len()].copy_from_slice(data);
        at += data.len().div_ceil(8) * 8;
    }
    // blocos de dados a partir de 4 MiB; o BAT guarda estado e deslocamento (em MiB)
    let ratio = (1usize << 23) * 512 / MB;
    let bat = 2 * MB;
    for (i, b) in blocks.iter().enumerate() {
        let Some((state, data)) = b else { continue };
        let mut off = 0;
        if !data.is_empty() {
            off = img.len();
            let mut d = data.clone();
            d.resize(MB, 0);
            img.extend_from_slice(&d);
        }
        le(
            &mut img,
            bat + (i + i / ratio) * 8,
            state | (off as u64 >> 20) << 20,
            8,
        );
    }
    if let Some(bm) = bitmap {
        let off = img.len();
        let mut d = bm;
        d.resize(MB, 0);
        img.extend_from_slice(&d);
        le(&mut img, bat + ratio * 8, 6 | (off as u64 >> 20) << 20, 8);
    }
    img
}

#[test]
fn vhdx_dynamic_with_zero_and_missing_blocks() {
    let want = raw(); // 1 MiB = um bloco
    let mut expect = want.clone();
    expect.extend_from_slice(&vec![0u8; 3 * MB]);
    let tail = vec![0x5Au8; 100_000];
    expect[3 * MB..3 * MB + tail.len()].copy_from_slice(&tail);
    let img = build_vhdx(
        4 * MB as u64,
        &[
            Some((6, want.clone())),
            Some((2, vec![])), // zerado
            None,              // não presente
            Some((6, tail)),
        ],
        None,
        None,
    );
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.vhdx");
    std::fs::write(&p, img).unwrap();
    check_content(&p, &expect);
}

#[test]
fn vhdx_differencing_uses_the_sector_bitmap() {
    let want = raw();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pai.raw"), &want).unwrap();
    // filho: bloco 0 parcialmente presente, só com os setores 3 e 2000..2010 no filho
    let mut data = vec![0u8; MB];
    let mut bm = vec![0u8; 512];
    let mut expect = want.clone();
    for s in std::iter::once(3usize).chain(2000..2010) {
        data[s * 512..(s + 1) * 512].fill(0x77);
        expect[s * 512..(s + 1) * 512].fill(0x77);
        bm[s / 8] |= 1 << (s % 8);
    }
    let img = build_vhdx(MB as u64, &[Some((7, data))], Some("pai.raw"), Some(bm));
    let p = dir.path().join("filho.vhdx");
    std::fs::write(&p, img).unwrap();
    check_content(&p, &expect);
}

// ---------- DMG ----------

#[test]
fn dmg_with_every_chunk_type() {
    let want = raw();
    let sec = 512usize;
    // 8 chunks de 128 setores (64 KiB)
    let part = |i: usize| want[i * 128 * sec..(i + 1) * 128 * sec].to_vec();
    let (adc_enc, adc_want) = adc_pattern(128 * sec);
    let mut bz = bzip2::write::BzEncoder::new(vec![], bzip2::Compression::default());
    bz.write_all(&part(3)).unwrap();
    let chunks = vec![
        DmgChunk {
            ty: 1,
            sectors: 128,
            payload: part(0),
        }, // bruto
        DmgChunk {
            ty: 0,
            sectors: 128,
            payload: vec![],
        }, // zerado
        DmgChunk {
            ty: 0x8000_0005,
            sectors: 128,
            payload: zlib(&part(2)),
        },
        DmgChunk {
            ty: 0x8000_0006,
            sectors: 128,
            payload: bz.finish().unwrap(),
        },
        DmgChunk {
            ty: 0x8000_0004,
            sectors: 128,
            payload: adc_enc,
        },
        DmgChunk {
            ty: 2,
            sectors: 128,
            payload: vec![],
        }, // ignorado
        DmgChunk {
            ty: 0x8000_0005,
            sectors: 128,
            payload: zlib(&part(6)),
        },
    ];
    let mut expect = vec![];
    expect.extend(part(0));
    expect.extend(vec![0u8; 128 * sec]);
    expect.extend(part(2));
    expect.extend(part(3));
    expect.extend(adc_want);
    expect.extend(vec![0u8; 128 * sec]);
    expect.extend(part(6));
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.dmg");
    std::fs::write(&p, build_dmg(&chunks, expect.len() as u64 / 512)).unwrap();
    check_content(&p, &expect);
}

#[test]
fn dmg_lzfse_and_encrypted_are_refused_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("l.dmg");
    let chunks = [DmgChunk {
        ty: 0x8000_0007,
        sectors: 8,
        payload: vec![1, 2, 3],
    }];
    std::fs::write(&p, build_dmg(&chunks, 8)).unwrap();
    let s = open_path(&p).unwrap();
    let e = read_vec(s.as_ref(), 0, 512).unwrap_err().to_string();
    assert!(e.contains("LZFSE"), "{e}");

    let p = dir.path().join("e.dmg");
    let mut img = b"encrcdsa".to_vec();
    img.resize(8192, 0);
    std::fs::write(&p, img).unwrap();
    let e = open_path(&p).err().expect("deveria recusar").to_string();
    assert!(e.contains("cifrado"), "{e}");
}

// ---------- imagens reais do dfvfs (target/testdata) ----------

#[test]
fn dfvfs_images_match_a_reference_reader() {
    // hashes do disco virtual inteiro, conferidos com o 7-Zip (árvore de arquivos e partições)
    let table: &[(&str, &str)] = &[
        (
            "ext2.vhdx",
            "a6c2f0e39afe6c6ab432ca5465349fcefe8dc944398e97b2d957d3f89dbb5d80",
        ),
        (
            "ext2.vmdk",
            "a6c2f0e39afe6c6ab432ca5465349fcefe8dc944398e97b2d957d3f89dbb5d80",
        ),
        (
            "ext2.vhd",
            "870be7ae16c1fa8faab05c6eb9205dc9a7ae35c5f552c5cf8a267c0bc6a5cb99",
        ),
        (
            "fvdetest.qcow2",
            "732091e57632b7a2332f218d2d3a9ce1355a836ebacce91615a2c30a178e4ec3",
        ),
        (
            "windows_volume.qcow2",
            "44abe9deca9658f331a2bdba2deb53192d04f35d157a1fb3ba0597b03f0524f2",
        ),
        (
            "apm.dmg",
            "897f487f4971e72c762e00460935e3b25176cda35620b508b490f65a7e0118ff",
        ),
        (
            "ntfs-dynamic.vhd",
            "8b10b52e3d1e76075d05a5101a6536b9fa8376a5b8f7c22f502d0987d165c876",
        ),
        (
            "ntfs-fixed.vhd",
            "cd81fade3878f1b2957f1ece168201876bdf4e90c9b30f60c2f232eba5fea76c",
        ),
        (
            "ntfs-dynamic.vhdx",
            "b92731f8e4bf3532418444d22a9086823a8122569500acd09cd8d2ce20eb98e9",
        ),
        (
            "ntfs-fixed.vhdx",
            "5237dd895f38052344e2ef35a207fe9df08cd18d17b6ea857a1b93cc475e1f2e",
        ),
        (
            "fat-differential.vhd",
            "f4b61a33ee2f7e8c19f737792a4107e46340026a9a829e4169b751901cc3ef02",
        ),
        (
            "ntfs-differential.vhd",
            "b7922a46a6b08fa4f4fc0c51a25d5587d1d49f15cfade45ca740dd07110ae144",
        ),
        (
            "fat-differential.vhdx",
            "a538161f34bc44c12bf9a542ce9218741c62728723ea18d0e4f6ea92e9f32e93",
        ),
        (
            "ntfs-differential.vhdx",
            "1ef1fb5360247aa7d36a38415bc7bf6e1e52f62af5e6bd1283815e82584c7efc",
        ),
    ];
    for (name, hash) in table {
        let Some(p) = dfvfs(name) else { continue };
        let s = open_path(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&sha(s.as_ref()), hash, "{name}");
    }
}

#[test]
fn partition_table_is_found_inside_a_qcow2() {
    let Some(p) = dfvfs("fvdetest.qcow2") else {
        return;
    };
    let s = open_path(&p).unwrap();
    let layout = guara_core::partition::analyze(s.as_ref(), false);
    assert_eq!(layout.table, "gpt");
    assert_eq!(layout.partitions.len(), 1);
}
