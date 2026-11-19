//! Маленькие кодеки для golden-эталонов: PAM 16 бит без сжатия (эталон) и PNG 8 бит со «stored»-deflate (превью для глаз).
//! Свои, потому что тяжёлых зависимостей ради теста не хочется (правило проекта).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "тестовый код")]

/// Кодирует пиксели (RGBA float 0..1, построчно сверху вниз) в PAM 16 бит RGB_ALPHA, big-endian.
pub fn encode_pam(width: u32, height: u32, pixels: &[[f32; 4]]) -> Vec<u8> {
    let mut bytes =
        format!("P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 4\nMAXVAL 65535\nTUPLTYPE RGB_ALPHA\nENDHDR\n").into_bytes();
    for pixel in pixels {
        for channel in pixel {
            let value = (channel.clamp(0.0, 1.0) * 65535.0).round() as u16;
            bytes.extend_from_slice(&value.to_be_bytes());
        }
    }
    bytes
}

/// Разбор PAM, записанного `encode_pam`: (ширина, высота, пиксели). Другой заголовок — `Err` с причиной.
pub fn decode_pam(bytes: &[u8]) -> Result<(u32, u32, Vec<[f32; 4]>), String> {
    const END: &[u8] = b"ENDHDR\n";
    let header_end = bytes
        .windows(END.len())
        .position(|window| window == END)
        .ok_or("нет ENDHDR")?
        + END.len();
    let header = std::str::from_utf8(&bytes[..header_end]).map_err(|e| e.to_string())?;
    let field = |name: &str| -> Result<u32, String> {
        header
            .lines()
            .find_map(|line| line.strip_prefix(name).and_then(|rest| rest.trim().parse().ok()))
            .ok_or(format!("нет поля {name}"))
    };
    let (width, height) = (field("WIDTH")?, field("HEIGHT")?);
    if field("DEPTH")? != 4 || field("MAXVAL")? != 65535 {
        return Err("ожидался DEPTH 4 и MAXVAL 65535".to_owned());
    }
    let body = &bytes[header_end..];
    if body.len() != (width * height * 8) as usize {
        return Err(format!("тело {} байт, нужно {}", body.len(), width * height * 8));
    }
    let pixels = body
        .chunks_exact(8)
        .map(|chunk| {
            let channel = |i: usize| f32::from(u16::from_be_bytes([chunk[i * 2], chunk[i * 2 + 1]])) / 65535.0;
            [channel(0), channel(1), channel(2), channel(3)]
        })
        .collect();
    Ok((width, height, pixels))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in bytes {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut checked = kind.to_vec();
    checked.extend_from_slice(data);
    out.extend_from_slice(&checked);
    out.extend_from_slice(&crc32(&checked).to_be_bytes());
}

/// PNG 8 бит RGB (альфа отбрасывается), deflate без сжатия (stored-блоки ≤ 65535 байт).
pub fn encode_png(width: u32, height: u32, pixels: &[[f32; 4]]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((width * height * 3 + height) as usize);
    for row in pixels.chunks(width as usize) {
        raw.push(0); // фильтр строки «None»
        for pixel in row {
            raw.extend(pixel[..3].iter().map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none())); // BFINAL, BTYPE = 00 (stored)
        zlib.extend_from_slice(&(block.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    push_chunk(&mut png, b"IHDR", &header);
    push_chunk(&mut png, b"IDAT", &zlib);
    push_chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pam_round_trips_within_one_sixteen_bit_step() {
        let pixels = vec![[0.0, 0.25, 0.5, 1.0], [1.0, 0.123_456, 0.9, 0.5]];
        let (w, h, decoded) = decode_pam(&encode_pam(2, 1, &pixels)).unwrap();
        assert_eq!((w, h), (2, 1));
        for (a, b) in pixels.iter().zip(&decoded) {
            for c in 0..4 {
                assert!((a[c] - b[c]).abs() <= 1.0 / 65535.0, "{a:?} {b:?}");
            }
        }
        assert!(decode_pam(b"P7\nWIDTH 1\n").is_err());
    }

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
