//! Houston's icons, drawn in code.
//!
//! - [`island`]: the menu bar glyph, a capsule hanging from the top edge like
//!   the island under the notch, as a template image so macOS tints it
//! - [`app_icon_png`]: the "Splashdown" app icon, for the About panel and the
//!   bundle (`houston --app-icon <file.png>`, used by bundle.sh): the island
//!   dropping out of the notch, glowing amber, with signal rings under it
//!
//! The app icon's shapes are signed distance functions (negative inside), so
//! every size is antialiased from one description without an image library.

use tray_icon::Icon;

/// The menu bar glyph: 18 pt at 2x.
const SIZE: u32 = 36;
const SAMPLES: u32 = 4;

/// Distance from `(x, y)` to a rectangle `x0..x1` × `y0..y1` with corners
/// rounded by `r`.
fn rounded_rect(x: f64, y: f64, (x0, y0, x1, y1): (f64, f64, f64, f64), r: f64) -> f64 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let qx = (x - cx).abs() - ((x1 - x0) / 2.0 - r);
    let qy = (y - cy).abs() - ((y1 - y0) / 2.0 - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn circle(x: f64, y: f64, (cx, cy): (f64, f64), r: f64) -> f64 {
    (x - cx).hypot(y - cy) - r
}

/// Distance to the lower arc of a circle (y grows downwards) between the
/// angles whose sine is `rise`, i.e. from `asin(rise)` to `π - asin(rise)`,
/// stroked `width` wide with round caps.
fn lower_arc(x: f64, y: f64, (cx, cy): (f64, f64), r: f64, rise: f64, width: f64) -> f64 {
    let (dx, dy) = (x - cx, y - cy);
    let d = dx.hypot(dy);
    let on_arc = if dy >= rise * d {
        (d - r).abs()
    } else {
        let ex = r * (1.0 - rise * rise).sqrt();
        let end = (rise * r) + cy;
        (x - (cx - ex))
            .hypot(y - end)
            .min((x - (cx + ex)).hypot(y - end))
    };
    on_arc - width / 2.0
}

/// How much of a pixel a shape covers, from its distance in pixels.
fn cover(distance_px: f64) -> f64 {
    (0.5 - distance_px).clamp(0.0, 1.0)
}

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

pub fn rgba() -> Vec<u8> {
    let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for py in 0..SIZE {
        for px in 0..SIZE {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f64 + (sx as f64 + 0.5) / SAMPLES as f64) / SIZE as f64 - 0.5;
                    let y = 1.0 - (py as f64 + (sy as f64 + 0.5) / SAMPLES as f64) / SIZE as f64;
                    if covered(x, y) {
                        hits += 1;
                    }
                }
            }
            out.extend([0, 0, 0, (hits * 255 / (SAMPLES * SAMPLES)) as u8]);
        }
    }
    out
}

pub fn island() -> Option<Icon> {
    Icon::from_rgba(rgba(), SIZE, SIZE).ok()
}

type Rgb = [f64; 3];

fn hex(c: u32) -> Rgb {
    [(c >> 16) & 255, (c >> 8) & 255, c & 255].map(|v| v as f64 / 255.0)
}

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// Premultiplied RGBA, painted back to front.
struct Paint([f64; 4]);

impl Paint {
    fn over(&mut self, color: Rgb, alpha: f64) {
        let a = alpha.clamp(0.0, 1.0);
        for (i, c) in color.iter().enumerate() {
            self.0[i] = c * a + self.0[i] * (1.0 - a);
        }
        self.0[3] = a + self.0[3] * (1.0 - a);
    }
}

/// The app icon's colour at `(x, y)` on a 512-unit grid (the macOS icon
/// grid: a 412-unit tile inset 50), `px` units to a pixel.
fn splashdown(x: f64, y: f64, px: f64) -> [f64; 4] {
    let amber = |x: f64| mix(hex(0xffc247), hex(0xff6a1f), (x - 136.0) / 240.0);
    let ink = hex(0x1f1206);
    let tile = cover(rounded_rect(x, y, (50.0, 50.0, 462.0, 462.0), 92.0) / px);
    let mut p = Paint([0.0; 4]);
    p.over(mix(hex(0x1b2436), hex(0x07090f), (y - 50.0) / 412.0), tile);
    // The notch, cut into the top edge.
    let notch = rounded_rect(x, y, (166.0, -100.0, 346.0, 120.0), 28.0);
    p.over([0.0; 3], tile * cover(notch / px));
    // The island's glow: its outline, blurred.
    let island = rounded_rect(x, y, (136.0, 200.0, 376.0, 292.0), 46.0);
    let glow = 0.6 / (1.0 + (1.7 * island / 14.0).exp());
    p.over(hex(0xff8a1f), tile * glow);
    p.over(amber(x), cover(island / px));
    p.over(ink, cover(circle(x, y, (182.0, 246.0), 17.0) / px));
    p.over(
        ink,
        0.5 * cover(rounded_rect(x, y, (216.0, 238.0, 338.0, 254.0), 8.0) / px),
    );
    // Signal rings under it, fading out.
    for (r, width, alpha) in [(56.0, 12.0, 0.55), (92.0, 10.0, 0.32), (128.0, 8.0, 0.16)] {
        let ring = lower_arc(x, y, (256.0, 292.0), r, 0.5, width);
        p.over(hex(0xff9a2e), alpha * tile * cover(ring / px));
    }
    p.0
}

/// The app icon as a PNG, `size` pixels square.
pub fn app_icon_png(size: u32) -> Vec<u8> {
    let px = 512.0 / size as f64;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for row in 0..size {
        for col in 0..size {
            let [r, g, b, a] = splashdown((col as f64 + 0.5) * px, (row as f64 + 0.5) * px, px);
            let straight = |c: f64| {
                if a > 0.0 {
                    (c / a * 255.0).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            rgba.extend([
                straight(r),
                straight(g),
                straight(b),
                (a * 255.0).round() as u8,
            ]);
        }
    }
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
    fn app_icon_colours() {
        let at = |x: f64, y: f64| splashdown(x, y, 1.0);
        assert_eq!(at(20.0, 20.0)[3], 0.0, "outside the tile");
        let notch = at(256.0, 70.0);
        assert!(
            notch[..3].iter().all(|&c| c < 0.01),
            "the notch is black: {notch:?}"
        );
        let island = at(300.0, 220.0);
        assert!(island[0] > 0.95 && island[2] < 0.3, "amber: {island:?}");
        assert!(at(182.0, 246.0)[0] < 0.2, "the dot is dark");
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
