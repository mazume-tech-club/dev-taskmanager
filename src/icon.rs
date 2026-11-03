//! Application icon (assets/icon, from the design zip) for the window, the
//! tray and the exe resource (see build.rs). The tray icon gets a coloured
//! badge in the bottom-left corner when the Alert Level is Warning or
//! Critical; at Normal the plain brand icon is shown.

use crate::alert::AlertLevel;

const PNG_32: &[u8] = include_bytes!("../assets/icon/commitwatch_32.png");
const PNG_64: &[u8] = include_bytes!("../assets/icon/commitwatch_64.png");
const PNG_256: &[u8] = include_bytes!("../assets/icon/commitwatch_256.png");

pub fn colour(level: AlertLevel) -> [u8; 3] {
    match level {
        AlertLevel::Normal => [46, 160, 67],
        AlertLevel::Warning => [240, 180, 0],
        AlertLevel::Critical => [220, 50, 47],
    }
}

/// Decoded straight-alpha RGBA, top-down rows, with its square size.
pub struct Rgba {
    pub size: usize,
    pub data: Vec<u8>,
}

fn decode(png: &[u8]) -> Rgba {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().expect("embedded icon PNG");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("png size")];
    let info = reader
        .next_frame(&mut buf)
        .expect("embedded icon PNG frame");
    assert_eq!(info.width, info.height, "icon must be square");
    assert_eq!(info.color_type, png::ColorType::Rgba, "icon must be RGBA");
    buf.truncate(info.buffer_size());
    Rgba {
        size: info.width as usize,
        data: buf,
    }
}

pub fn brand(size: usize) -> Rgba {
    match size {
        0..=32 => decode(PNG_32),
        33..=64 => decode(PNG_64),
        _ => decode(PNG_256),
    }
}

/// Window / taskbar icon.
pub fn window_rgba() -> Rgba {
    brand(64)
}

/// Tray icon for the level: brand icon plus a badge for Warning/Critical.
pub fn tray_rgba(level: AlertLevel) -> Rgba {
    let mut img = brand(32);
    if level != AlertLevel::Normal {
        let [r, g, b] = colour(level);
        draw_badge(&mut img, [r, g, b]);
    }
    img
}

/// Filled circle with a white ring in the bottom-left corner, about 45% of
/// the icon so it survives scaling to 16 px.
fn draw_badge(img: &mut Rgba, [r, g, b]: [u8; 3]) {
    let s = img.size as f64;
    let radius = s * 0.22;
    let cx = radius + 0.5;
    let cy = s - radius - 0.5;
    for y in 0..img.size {
        for x in 0..img.size {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let i = (y * img.size + x) * 4;
            if d <= radius - 1.5 {
                img.data[i..i + 4].copy_from_slice(&[r, g, b, 255]);
            } else if d <= radius {
                img.data[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
}

/// Icon image resource (BITMAPINFOHEADER + BGRA bottom-up + empty AND mask),
/// the format `CreateIconFromResourceEx` accepts.
pub fn ico_image(img: &Rgba) -> Vec<u8> {
    let size = img.size;
    let rgba = &img.data;
    let mask_stride = size.div_ceil(32) * 4;
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

/// Tray icon as an icon resource for the given level.
pub fn tray_image(level: AlertLevel) -> Vec<u8> {
    ico_image(&tray_rgba(level))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_pngs_decode() {
        assert_eq!(brand(32).size, 32);
        assert_eq!(brand(64).size, 64);
        assert_eq!(brand(256).size, 256);
        assert_eq!(brand(32).data.len(), 32 * 32 * 4);
    }

    #[test]
    fn badge_only_when_not_normal() {
        let normal = tray_rgba(AlertLevel::Normal);
        let critical = tray_rgba(AlertLevel::Critical);
        // bottom-left corner pixel region: unchanged for Normal, red for Critical
        let i = ((32 - 7) * 32 + 7) * 4;
        assert_eq!(normal.data[i..i + 4], brand(32).data[i..i + 4]);
        assert_eq!(critical.data[i..i + 4], [220, 50, 47, 255]);
    }

    #[test]
    fn ico_image_has_expected_length() {
        let img = tray_image(AlertLevel::Warning);
        assert_eq!(img.len(), 40 + 32 * 32 * 4 + 4 * 32);
        assert_eq!(&img[0..4], &40u32.to_le_bytes());
    }
}
