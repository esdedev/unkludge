//! The app's icon: a key with a small screen on it, drawn at any size with no files. It's used for
//! the window and tray, and by `build.rs` for the executable. Pixels are BGRA, top to bottom, with
//! alpha that isn't premultiplied.

/// A colour as red, green and blue.
type Color = [f32; 3];

/// The keycap's sides.
const CAP: Color = [61.0, 43.0, 184.0];
/// The keycap's top, in the app's accent colour.
pub const TOP: Color = [100.0, 72.0, 240.0];
const SCREEN: Color = [18.0, 16.0, 34.0];
/// The screen's light.
const LUME: Color = [233.0, 228.0, 255.0];

/// Whether a point is in a rectangle with rounded corners. All in fractions of the icon's side.
fn in_round(x: f32, y: f32, (x0, y0, x1, y1): (f32, f32, f32, f32), radius: f32) -> bool {
    let dx = (x0 + radius - x).max(x - (x1 - radius)).max(0.0);
    let dy = (y0 + radius - y).max(y - (y1 - radius)).max(0.0);
    x >= x0 && x <= x1 && y >= y0 && y <= y1 && dx.hypot(dy) <= radius
}

/// The colour at a point, if the icon covers it.
fn color_at(x: f32, y: f32) -> Option<Color> {
    if !in_round(x, y, (0.04, 0.06, 0.96, 0.96), 0.2) {
        return None;
    }
    if !in_round(x, y, (0.13, 0.1, 0.87, 0.76), 0.13) {
        return Some(CAP);
    }
    if !in_round(x, y, (0.24, 0.22, 0.76, 0.6), 0.05) {
        return Some(TOP);
    }
    // A line of light across the screen, which is what can be seen of it at tray size.
    Some(if in_round(x, y, (0.32, 0.36, 0.68, 0.47), 0.02) {
        LUME
    } else {
        SCREEN
    })
}

/// A square icon of `size` pixels.
pub fn pixels(size: usize) -> Vec<u8> {
    const SAMPLES: usize = 4;
    let side = size as f32;
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let mut sum = [0.0; 3];
            let mut covered = 0;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = (x as f32 + (sample_x as f32 + 0.5) / SAMPLES as f32) / side;
                    let py = (y as f32 + (sample_y as f32 + 0.5) / SAMPLES as f32) / side;
                    if let Some(color) = color_at(px, py) {
                        for (total, part) in sum.iter_mut().zip(color) {
                            *total += part;
                        }
                        covered += 1;
                    }
                }
            }
            if covered > 0 {
                let at = (y * size + x) * 4;
                out[at] = (sum[2] / covered as f32) as u8;
                out[at + 1] = (sum[1] / covered as f32) as u8;
                out[at + 2] = (sum[0] / covered as f32) as u8;
                out[at + 3] = (255 * covered / (SAMPLES * SAMPLES)) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixels: &[u8], size: usize, x: usize, y: usize) -> u8 {
        pixels[(y * size + x) * 4 + 3]
    }

    #[test]
    fn it_has_four_bytes_per_pixel_at_every_size() {
        for size in [16, 20, 24, 32, 48, 64, 256] {
            assert_eq!(pixels(size).len(), size * size * 4);
        }
    }

    #[test]
    fn the_corners_are_clear_and_the_middle_is_solid() {
        for size in [16, 32, 256] {
            let icon = pixels(size);
            assert_eq!(alpha(&icon, size, 0, 0), 0);
            assert_eq!(alpha(&icon, size, size - 1, 0), 0);
            assert_eq!(alpha(&icon, size, size / 2, size / 2), 255);
        }
    }

    #[test]
    fn the_screen_shows_even_at_tray_size() {
        let icon = pixels(16);
        let at = |x: usize, y: usize| &icon[(y * 16 + x) * 4..(y * 16 + x) * 4 + 3];
        // The light in the screen, then the key's top beside the screen, which is blue before green.
        assert!(
            at(8, 6).iter().all(|&channel| channel > 120),
            "{:?}",
            at(8, 6)
        );
        assert!(at(3, 6)[0] > at(3, 6)[1], "{:?}", at(3, 6));
    }
}
