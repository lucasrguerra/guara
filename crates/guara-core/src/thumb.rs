//! Miniaturas para a grade de arquivos. Decodifica com redução na própria
//! decodificação (JPEG) para ser rápido mesmo com milhares de fotos e, se a
//! imagem estiver danificada, tenta a miniatura embutida no EXIF.

use crate::recover::read_found_at;
use crate::source::Source;
use crate::types::Found;
use image::{imageops, ImageBuffer, Rgb, RgbImage};

const MAX_READ: u64 = 64 * 1024 * 1024;

fn read_all(src: &dyn Source, f: &Found) -> Option<Vec<u8>> {
    if f.size == 0 || f.size > MAX_READ {
        return None;
    }
    let mut v = vec![0u8; f.size as usize];
    let mut got = 0;
    while got < v.len() {
        match read_found_at(src, f, got as u64, &mut v[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    v.truncate(got);
    (!v.is_empty()).then_some(v)
}

/// Miniatura JPEG com o maior lado = `max` (None se não for imagem ou não decodificar).
pub fn thumbnail(src: &dyn Source, f: &Found, max: u32) -> Option<Vec<u8>> {
    let data = read_all(src, f)?;
    let img = match f.ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => decode_jpeg(&data, max).or_else(|| {
            let t = crate::meta::exif_thumbnail(&data)?;
            decode_jpeg(&t, max)
        })?,
        "png" | "gif" | "bmp" | "webp" => decode_other(&data, max)?,
        _ => return None,
    };
    encode(&img)
}

fn decode_jpeg(data: &[u8], max: u32) -> Option<RgbImage> {
    let mut m = crate::meta::Meta::default();
    crate::meta::jpeg_bytes(data, &mut m);
    let orientation = m.orientation;
    let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(data));
    dec.read_info().ok()?;
    let _ = dec.scale(max.min(65535) as u16, max.min(65535) as u16);
    let px = dec.decode().ok()?;
    let info = dec.info()?;
    let (w, h) = (info.width as u32, info.height as u32);
    let rgb: Vec<u8> = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => px,
        jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&g| [g, g, g]).collect(),
        jpeg_decoder::PixelFormat::L16 => px
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[0], c[0]])
            .collect(),
        jpeg_decoder::PixelFormat::CMYK32 => px
            .chunks_exact(4)
            .flat_map(|c| {
                // Adobe CMYK vem invertido
                let k = c[3] as u32;
                [
                    (c[0] as u32 * k / 255) as u8,
                    (c[1] as u32 * k / 255) as u8,
                    (c[2] as u32 * k / 255) as u8,
                ]
            })
            .collect(),
    };
    let img: RgbImage = ImageBuffer::from_raw(w, h, rgb)?;
    Some(orient(fit(img, max), orientation))
}

fn decode_other(data: &[u8], max: u32) -> Option<RgbImage> {
    let dynimg = image::load_from_memory(data).ok()?;
    let rgba = dynimg.to_rgba8();
    // Achata a transparência sobre cinza claro.
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as u32;
        let mix = |c: u8| ((c as u32 * a + 240 * (255 - a)) / 255) as u8;
        rgb.put_pixel(x, y, Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
    }
    Some(fit(rgb, max))
}

fn fit(img: RgbImage, max: u32) -> RgbImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= max {
        return img;
    }
    let (nw, nh) = if w >= h {
        (max, (h as u64 * max as u64 / w as u64).max(1) as u32)
    } else {
        ((w as u64 * max as u64 / h as u64).max(1) as u32, max)
    };
    imageops::resize(&img, nw, nh, imageops::FilterType::Triangle)
}

fn orient(img: RgbImage, o: u16) -> RgbImage {
    match o {
        2 => imageops::flip_horizontal(&img),
        3 => imageops::rotate180(&img),
        4 => imageops::flip_vertical(&img),
        5 => imageops::flip_horizontal(&imageops::rotate90(&img)),
        6 => imageops::rotate90(&img),
        7 => imageops::flip_horizontal(&imageops::rotate270(&img)),
        8 => imageops::rotate270(&img),
        _ => img,
    }
}

fn encode(img: &RgbImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let enc = jpeg_encoder::Encoder::new(&mut out, 80);
    enc.encode(
        img.as_raw(),
        img.width() as u16,
        img.height() as u16,
        jpeg_encoder::ColorType::Rgb,
    )
    .ok()?;
    Some(out)
}
