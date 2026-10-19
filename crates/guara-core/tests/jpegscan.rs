use guara_core::jpegscan::*;

fn photo(w: u16, h: u16) -> Vec<u8> {
    // Imagem "natural": gradientes suaves + textura + bordas.
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    let mut x = 0x9E3779B97F4A7C15u64;
    for yy in 0..h as i32 {
        for xx in 0..w as i32 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let noise = ((x >> 40) & 31) as i32;
            let base = 90 + (xx * 90 / w as i32) + (yy * 60 / h as i32);
            let blob = if ((xx - 200).pow(2) + (yy - 150).pow(2)) < 3600 {
                70
            } else {
                0
            };
            let v = (base + blob + noise).clamp(0, 255) as u8;
            rgb.extend_from_slice(&[v, v.saturating_sub(20), (v as i32 / 2 + noise) as u8]);
        }
    }
    let mut out = vec![];
    let enc = jpeg_encoder::Encoder::new(&mut out, 92);
    enc.encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    out
}

#[test]
fn decodes_whole_image_and_ends_at_eoi() {
    let j = photo(1024, 768);
    let h = parse_header(&j).expect("cabeçalho");
    assert_eq!((h.width, h.height), (1024, 768));
    let d = decode(&h, &j, State::start(&h), u32::MAX, 0);
    assert_eq!(d.stop, Stop::Done);
    assert_eq!(d.dc.len() as u32, h.total_mcus());
    // O que sobra depois do último MCU é o EOI.
    let tail = &j[d.state.pos.min(j.len())..];
    assert!(
        tail.windows(2).any(|w| w == [0xFF, 0xD9]),
        "sem EOI depois do último MCU"
    );
    assert!(first_discontinuity(&h, &d.dc).is_none());
}

#[test]
fn detects_break_and_resumes_with_the_right_continuation() {
    let j = photo(1024, 768);
    let h = parse_header(&j).unwrap();
    let cut = 12 * 4096; // quebra no limite de cluster
    assert!(j.len() > cut + 8192, "imagem pequena demais: {}", j.len());
    // Dados estranhos no lugar do resto.
    let mut x = 12345u64;
    let junk: Vec<u8> = (0..40_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    let mut broken = j[..cut].to_vec();
    broken.extend_from_slice(&junk);
    let d = decode(&h, &broken, State::start(&h), u32::MAX, 512);
    assert_ne!(d.stop, Stop::Done);
    // Decodificação segue o lixo por um tempo; mas a quebra aparece pelo erro ou pela descontinuidade.
    let mcu_break = first_discontinuity(&h, &d.dc);
    // MCUs bons: os decodificados ao fim do cluster `cut`.
    let good = d
        .snaps
        .iter()
        .rfind(|s| s.pos <= cut)
        .expect("snapshot antes da quebra")
        .clone();
    // Retomada com a continuação verdadeira reproduz a decodificação inteira.
    let mut joined = j[..cut].to_vec();
    joined.extend_from_slice(&j[cut..]);
    let r = decode(&h, &joined, good.clone(), u32::MAX, 0);
    assert_eq!(r.stop, Stop::Done);
    assert_eq!(good.mcu as usize + r.dc.len(), h.total_mcus() as usize);
    let _ = mcu_break;
}
