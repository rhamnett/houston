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

pub fn rgba() -> Vec<u8> {
    let mut out = vec![0u8; (SIZE * SIZE * 4) as usize];
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
            let i = ((py * SIZE + px) * 4) as usize;
            out[i + 3] = (hits * 255 / (SAMPLES * SAMPLES)) as u8;
        }
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
}
