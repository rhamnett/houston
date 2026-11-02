//! The status item icon: a capsule hanging from the top edge, like the island
//! under the notch, drawn in code as a template image so macOS tints it.

use tray_icon::Icon;

const SIZE: u32 = 36;
const SAMPLES: u32 = 4;

/// Unit coordinates: x in -0.5..0.5, y from 0 (bottom) to 1 (top).
fn covered(x: f64, y: f64) -> bool {
    let bar = (0.80..=0.88).contains(&y) && x.abs() <= 0.46;
    // A capsule 0.64 wide and 0.34 tall, rounded at both ends.
    let (cy, half_w, r) = (0.56, 0.32, 0.17);
    let dx = (x.abs() - (half_w - r)).max(0.0);
    let dy = y - cy;
    let capsule = dx * dx + dy * dy <= r * r;
    // A dot inside the capsule, cut out.
    let dot = (x + 0.15).powi(2) + (y - cy).powi(2) <= 0.06 * 0.06;
    bar || (capsule && !dot)
}

/// How much of each pixel of a `size`² image `inside` covers, 0 to 255.
fn coverage(size: u32, inside: impl Fn(f64, f64) -> bool) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f64 + (sx as f64 + 0.5) / SAMPLES as f64) / size as f64 - 0.5;
                    let y = 1.0 - (py as f64 + (sy as f64 + 0.5) / SAMPLES as f64) / size as f64;
                    if inside(x, y) {
                        hits += 1;
                    }
                }
            }
            out.push((hits * 255 / (SAMPLES * SAMPLES)) as u8);
        }
    }
    out
}

pub fn rgba() -> Vec<u8> {
    coverage(SIZE, covered)
        .into_iter()
        .flat_map(|a| [0, 0, 0, a])
        .collect()
}

/// A rounded square inset like a macOS app icon.
fn tile(x: f64, y: f64) -> bool {
    let (half, r) = (0.41, 0.09);
    let (u, v) = (x.abs(), (y - 0.5).abs());
    let (dx, dy) = ((u - (half - r)).max(0.0), (v - (half - r)).max(0.0));
    u <= half && v <= half && dx * dx + dy * dy <= r * r
}

/// The app icon, for the About panel: the glyph in white on a black tile,
/// the colours of the island itself. A PNG, `size` pixels square.
pub fn app_icon_png(size: u32) -> Vec<u8> {
    let tile = coverage(size, tile);
    // The glyph spans y 0.39..0.88; scale it to 70% and centre it.
    let glyph = coverage(size, |x, y| covered(x / 0.7, (y - 0.5) / 0.7 + 0.635));
    let rgba: Vec<u8> = tile
        .iter()
        .zip(&glyph)
        .flat_map(|(&a, &g)| [g, g, g, a])
        .collect();
    png(size, &rgba)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &d in data {
        a = (a + d as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// An 8-bit RGBA PNG, stored without compression: it's small and made once,
/// so a deflate crate isn't worth it.
fn png(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(rgba.len() + size as usize);
    for row in rgba.chunks(size as usize * 4) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks = raw.chunks(0xFFFF).count();
    for (i, block) in raw.chunks(0xFFFF).enumerate() {
        let len = block.len() as u16;
        zlib.push(u8::from(i + 1 == blocks)); // BFINAL, BTYPE 00 (stored)
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&size.to_be_bytes());
    header.extend_from_slice(&size.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(b"IHDR", header), (b"IDAT", zlib), (b"IEND", Vec::new())] {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(&data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    out
}

pub fn island() -> Option<Icon> {
    Icon::from_rgba(rgba(), SIZE, SIZE).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_capsule_with_a_dot() {
        let px = rgba();
        let alpha = |x: u32, y: u32| px[((y * SIZE + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, SIZE - 1), 0, "bottom corner is clear");
        assert_eq!(alpha(SIZE / 2 + 6, 16), 255, "capsule body");
        assert_eq!(alpha(SIZE / 2 - 5, 16), 0, "dot is cut out");
    }

    #[test]
    fn checksums() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn app_icon_is_a_png() {
        let png = app_icon_png(64);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..24], &[0, 0, 0, 64, 0, 0, 0, 64]);
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82]), "IEND's CRC");
    }
}
