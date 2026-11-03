//! Tray and window icon: a rounded square in the Alert Level colour, filled
//! from the bottom in proportion to Commit Usage.

use crate::alert::AlertLevel;

pub fn colour(level: AlertLevel) -> [u8; 3] {
    match level {
        AlertLevel::Normal => [46, 160, 67],
        AlertLevel::Warning => [240, 180, 0],
        AlertLevel::Critical => [220, 50, 47],
    }
}

/// Straight-alpha RGBA, top-down rows.
pub fn rgba(size: usize, level: AlertLevel, usage: f64) -> Vec<u8> {
    let [r, g, b] = colour(level);
    let fill_rows = (usage.clamp(0.0, 1.0) * size as f64).round() as usize;
    let radius = (size as f64 / 6.0).max(2.0);
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let inside = inside_rounded(x as f64 + 0.5, y as f64 + 0.5, size as f64, radius);
            let border = inside
                && !inside_rounded(x as f64 + 0.5, y as f64 + 0.5, size as f64, radius)
                    .then(|| inner(x, y, size))
                    .unwrap_or(false);
            let from_bottom = size - 1 - y;
            let alpha = if !inside {
                0
            } else if border || from_bottom < fill_rows {
                255
            } else {
                96
            };
            let i = (y * size + x) * 4;
            out[i] = r;
            out[i + 1] = g;
            out[i + 2] = b;
            out[i + 3] = alpha;
        }
    }
    out
}

fn inner(x: usize, y: usize, size: usize) -> bool {
    let edge = (size / 16).max(1);
    x >= edge && y >= edge && x < size - edge && y < size - edge
}

fn inside_rounded(x: f64, y: f64, size: f64, r: f64) -> bool {
    if x < 0.0 || y < 0.0 || x > size || y > size {
        return false;
    }
    let cx = x.clamp(r, size - r);
    let cy = y.clamp(r, size - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// Icon image resource (BITMAPINFOHEADER + BGRA bottom-up + empty AND mask),
/// the format `CreateIconFromResourceEx` accepts.
pub fn ico_image(size: usize, level: AlertLevel, usage: f64) -> Vec<u8> {
    let rgba = rgba(size, level, usage);
    let mask_stride = ((size + 31) / 32) * 4;
    let xor_len = size * size * 4;
    let and_len = mask_stride * size;
    let mut out = Vec::with_capacity(40 + xor_len + and_len);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((xor_len + and_len) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for y in (0..size).rev() {
        for x in 0..size {
            let i = (y * size + x) * 4;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    out.extend(std::iter::repeat_n(0u8, and_len));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_follows_usage() {
        let full = rgba(32, AlertLevel::Normal, 1.0);
        let empty = rgba(32, AlertLevel::Normal, 0.0);
        // centre pixel near the top: opaque when full, translucent when empty
        let i = (4 * 32 + 16) * 4 + 3;
        assert_eq!(full[i], 255);
        assert_eq!(empty[i], 96);
        // corners are transparent
        assert_eq!(full[3], 0);
    }

    #[test]
    fn ico_image_has_expected_length() {
        let img = ico_image(32, AlertLevel::Critical, 0.5);
        assert_eq!(img.len(), 40 + 32 * 32 * 4 + 4 * 32);
        assert_eq!(&img[0..4], &40u32.to_le_bytes());
    }
}
