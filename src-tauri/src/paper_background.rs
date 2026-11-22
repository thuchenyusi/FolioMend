// Paper whitening is independent of color reduction: dark ink stays unchanged,
// while neutral highlights approach white. Saturated annotations retain color.
pub fn whiten(pixels: &mut [u8], channels: u8, white_point: u8) -> usize {
    if !matches!(channels, 1 | 3) || white_point <= 96 {
        return 0;
    }
    let mut changed = 0;
    for pixel in pixels.chunks_exact_mut(channels as usize) {
        let low = *pixel.iter().min().unwrap() as u32;
        let high = *pixel.iter().max().unwrap() as u32;
        let chroma = high - low;
        let gray = if channels == 1 {
            low
        } else {
            (77 * pixel[0] as u32 + 150 * pixel[1] as u32 + 29 * pixel[2] as u32 + 128) / 256
        };
        if gray <= 96 || gray == 255 || chroma >= 40 {
            continue;
        }
        let target = (96 + (gray - 96) * 159 / (white_point as u32 - 96)).min(255);
        let gain = (target - gray) * (40 - chroma).min(16);
        let denominator = (255 - gray) * 16;
        let mut modified = false;
        for value in pixel {
            let next = (*value as u32
                + ((255 - *value as u32) * gain + denominator / 2) / denominator)
                .min(255) as u8;
            modified |= next != *value;
            *value = next;
        }
        changed += usize::from(modified);
    }
    changed
}

// Only a pale, predominantly neutral, narrow-tone layer may bypass photo
// protection when it is drawn underneath a matching masked foreground image.
pub fn looks_like_paper(pixels: &[u8], channels: u8) -> bool {
    if !matches!(channels, 1 | 3) || pixels.is_empty() {
        return false;
    }
    let step = (pixels.len() / channels as usize).div_ceil(50_000).max(1);
    let mut histogram = [0usize; 256];
    let (mut count, mut paper) = (0usize, 0usize);
    for pixel in pixels.chunks_exact(channels as usize).step_by(step) {
        let low = *pixel.iter().min().unwrap();
        let high = *pixel.iter().max().unwrap();
        paper += usize::from(high - low <= 24 && low >= 128);
        histogram[(u16::from(low) + u16::from(high)) as usize / 2] += 1;
        count += 1;
    }
    if count == 0 || paper * 100 < count * 95 {
        return false;
    }
    let percentile = |percent: usize| {
        let mut total = 0;
        histogram
            .iter()
            .position(|n| {
                total += n;
                total * 100 >= count * percent
            })
            .unwrap_or(255)
    };
    percentile(90) - percentile(10) <= 45
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitening_preserves_dark_ink_and_saturated_annotations() {
        let mut gray = vec![0, 32, 96, 128, 180, 205, 255];
        whiten(&mut gray, 1, 205);
        assert_eq!(&gray[..3], &[0, 32, 96]);
        assert!(gray[3] > 128 && gray[4] > 180);
        assert_eq!(&gray[5..], &[255, 255]);
        assert!(gray.windows(2).all(|w| w[0] <= w[1]));
        let mut rgb = vec![200, 198, 190, 240, 100, 100, 30, 40, 35];
        let original = rgb.clone();
        whiten(&mut rgb, 3, 195);
        assert_eq!(&rgb[..3], &[255, 255, 255]);
        assert_eq!(&rgb[3..], &original[3..]);
    }

    #[test]
    fn lower_white_point_whitens_more_and_paper_guard_rejects_photos() {
        let mut mild: Vec<u8> = (0..=255).collect();
        let mut strong = mild.clone();
        whiten(&mut mild, 1, 220);
        whiten(&mut strong, 1, 190);
        assert!(strong.iter().zip(&mild).all(|(a, b)| a >= b));
        assert!(looks_like_paper(&[195; 300], 3));
        assert!(!looks_like_paper(&(0..=255).collect::<Vec<u8>>(), 1));
        assert!(!looks_like_paper(&[220, 150, 120].repeat(100), 3));
        assert!(!looks_like_paper(&[], 1));
    }
}
