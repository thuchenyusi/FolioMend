// Detect continuous tones on a bounded grid before resizing or reducing colors.
// Nearest samples preserve the black/white structure of text; interpolating a
// thumbnail here would turn fine text into apparent continuous gray regions.
pub fn continuous_tone_fraction(pixels: &[u8], width: u32, height: u32, channels: u8) -> f32 {
    if width == 0
        || height == 0
        || !matches!(channels, 1 | 3)
        || u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(u64::from(channels)))
            != Some(pixels.len() as u64)
    {
        return 0.0;
    }
    let edge = width.max(height);
    let sample_size = |n: u32| ((u64::from(n) * 768 / u64::from(edge)).max(1) as u32).min(n);
    let sw = sample_size(width);
    let sh = sample_size(height);
    let mut tone_pixels = 0u32;
    for ty in (0..sh).step_by(24) {
        for tx in (0..sw).step_by(24) {
            let mut histograms = [[0u16; 32]; 3];
            let mut mid = [0u32; 3];
            let mut inner = [0u32; 3];
            let count = (sw - tx).min(24) * (sh - ty).min(24);
            for sy in ty..(ty + 24).min(sh) {
                let y = u64::from(sy) * u64::from(height) / u64::from(sh);
                for sx in tx..(tx + 24).min(sw) {
                    let x = u64::from(sx) * u64::from(width) / u64::from(sw);
                    let offset = ((y * u64::from(width) + x) * u64::from(channels)) as usize;
                    for c in 0..channels as usize {
                        let value = pixels[offset + c];
                        histograms[c][value as usize / 8] += 1;
                        mid[c] += u32::from((24..=231).contains(&value));
                        inner[c] += u32::from((8..=247).contains(&value));
                    }
                }
            }
            let continuous = (0..channels as usize).any(|c| {
                let bins = histograms[c]
                    .iter()
                    .filter(|&&n| u32::from(n) >= (count / 100).max(2))
                    .count();
                (mid[c] * 100 >= count * 35 && bins >= 6)
                    || (inner[c] * 100 >= count * 85 && bins >= 3)
            });
            if continuous {
                tone_pixels += count;
            }
        }
    }
    tone_pixels as f32 / (sw * sh) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradients_and_small_photo_regions_are_detected() {
        let mut gray = vec![255; 384 * 384];
        // A picture covering one ninth of a text page must trigger protection.
        for y in 96..224 {
            for x in 96..224 {
                gray[y * 384 + x] = (20 + (x - 96) * 210 / 128) as u8;
            }
        }
        assert!(continuous_tone_fraction(&gray, 384, 384, 1) >= 0.08);
        let rgb: Vec<_> = gray.iter().flat_map(|&v| [255, v, 255]).collect();
        assert!(continuous_tone_fraction(&rgb, 384, 384, 3) >= 0.08);
    }

    #[test]
    fn antialiased_text_and_line_drawings_are_not_photos() {
        let mut text = vec![255; 384 * 384];
        for y in 20..364 {
            for x in 20..364 {
                if y % 16 < 5 && x % 9 < 5 {
                    text[y * 384 + x] = if x % 9 == 4 { 150 } else { 0 };
                }
                if x == 100 || y == 200 {
                    text[y * 384 + x] = 0;
                }
            }
        }
        assert!(continuous_tone_fraction(&text, 384, 384, 1) < 0.03);
        assert_eq!(
            continuous_tone_fraction(&vec![255; 384 * 384], 384, 384, 1),
            0.0
        );
    }

    #[test]
    fn faint_shading_and_malformed_inputs() {
        let faint: Vec<_> = (0..384 * 384).map(|n| (200 + n % 48) as u8).collect();
        assert!(continuous_tone_fraction(&faint, 384, 384, 1) > 0.8);
        for (p, w, h, c) in [(&[][..], 0, 0, 1), (&[0][..], 1, 1, 3), (&[0][..], 1, 1, 4)] {
            assert_eq!(continuous_tone_fraction(p, w, h, c), 0.0);
        }
        assert_eq!(continuous_tone_fraction(&[128], 1, 1, 1), 0.0);
    }
}
