//! Read the quantization tables actually used by an 8-bit grayscale/YCbCr JPEG.
//! Only headers are inspected; unsupported color layouts fall back to normal encoding.

#[derive(Debug, Clone)]
pub struct JpegQuantization {
    pub channels: u8,
    pub luma: [u16; 64],
    pub chroma: [u16; 64],
}

// JPEG stores DQT entries in zigzag order; the encoder expects row-major order.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

impl JpegQuantization {
    pub fn read(jpeg: &[u8]) -> Option<Self> {
        if !jpeg.starts_with(&[0xff, 0xd8]) {
            return None;
        }
        let mut tables = [None; 4];
        let mut components: Option<Vec<(u8, usize)>> = None;
        let mut adobe_transform = None;
        let mut offset = 2usize;
        loop {
            if *jpeg.get(offset)? != 0xff {
                return None;
            }
            while *jpeg.get(offset)? == 0xff {
                offset += 1;
            }
            let marker = *jpeg.get(offset)?;
            offset += 1;
            if marker == 0xd9 || marker == 0xd8 || marker == 0x00 {
                return None;
            }
            if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
                return None;
            }
            let length =
                u16::from_be_bytes(jpeg.get(offset..offset + 2)?.try_into().ok()?) as usize;
            if length < 2 {
                return None;
            }
            let segment = jpeg.get(offset + 2..offset.checked_add(length)?)?;
            offset += length;
            match marker {
                0xdb => {
                    let mut entries = segment;
                    while let Some((&spec, rest)) = entries.split_first() {
                        let slot = (spec & 0x0f) as usize;
                        // The encoder writes 8-bit DQT values. Never truncate a 16-bit table.
                        if spec >> 4 != 0 || slot >= tables.len() {
                            return None;
                        }
                        let values = rest.get(..64)?;
                        let mut table = [0u16; 64];
                        for (index, &value) in values.iter().enumerate() {
                            if value == 0 {
                                return None;
                            }
                            table[ZIGZAG[index]] = u16::from(value);
                        }
                        tables[slot] = Some(table);
                        entries = &rest[64..];
                    }
                }
                0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                    if !matches!(marker, 0xc0..=0xc2) || components.is_some() {
                        return None;
                    }
                    if *segment.first()? != 8 {
                        return None;
                    }
                    let count = *segment.get(5)? as usize;
                    if !matches!(count, 1 | 3) || segment.len() != 6 + count * 3 {
                        return None;
                    }
                    components = Some(
                        segment[6..]
                            .chunks_exact(3)
                            .map(|component| (component[0], component[2] as usize))
                            .collect(),
                    );
                }
                0xee if segment.starts_with(b"Adobe") => {
                    adobe_transform = Some(*segment.get(11)?);
                }
                0xda => {
                    let components = components?;
                    let luma = *tables.get(components[0].1)?.as_ref()?;
                    let channels = components.len() as u8;
                    let chroma = if channels == 3 {
                        if adobe_transform.is_some_and(|value| value != 1)
                            || components.iter().map(|item| item.0).eq(*b"RGB")
                        {
                            return None;
                        }
                        let cb = *tables.get(components[1].1)?.as_ref()?;
                        let cr = *tables.get(components[2].1)?.as_ref()?;
                        if cb != cr {
                            return None;
                        }
                        cb
                    } else {
                        luma
                    };
                    return Some(Self {
                        channels,
                        luma,
                        chroma,
                    });
                }
                _ => {}
            }
        }
    }

    pub fn for_quality(&self, quality: u8) -> Self {
        // Q80 (the existing default) preserves the source table. Other quality
        // settings adjust its strength using the same relative IJG quality scale.
        let quality = u32::from(quality.clamp(1, 100));
        let scale = if quality < 50 {
            5000 / quality
        } else {
            200 - quality * 2
        };
        let adjust = |table: [u16; 64]| {
            table.map(|value| ((u32::from(value) * scale + 20) / 40).clamp(1, 255) as u16)
        };
        Self {
            channels: self.channels,
            luma: adjust(self.luma),
            chroma: adjust(self.chroma),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jpeg_encoder::{ColorType, Encoder, QuantizationTableType, SamplingFactor};

    #[test]
    fn reads_nonuniform_tables_in_natural_order_and_follows_component_slots() {
        let luma = std::array::from_fn(|i| i as u16 + 1);
        let chroma = std::array::from_fn(|i| 128 - i as u16);
        for (channels, color) in [(1, ColorType::Luma), (3, ColorType::Rgb)] {
            let mut jpeg = Vec::new();
            let mut encoder = Encoder::new(&mut jpeg, 80);
            encoder.set_sampling_factor(SamplingFactor::F_1_1);
            encoder.set_quantization_tables(
                QuantizationTableType::Custom(Box::new(luma)),
                QuantizationTableType::Custom(Box::new(chroma)),
            );
            encoder
                .encode(&vec![127; 16 * 16 * channels], 16, 16, color)
                .unwrap();
            let parsed = JpegQuantization::read(&jpeg).unwrap();
            assert_eq!(parsed.channels as usize, channels);
            assert_eq!(parsed.luma, luma);
            assert_eq!(parsed.chroma, if channels == 1 { luma } else { chroma });
            assert_eq!(parsed.for_quality(80).luma, luma);
            assert!(parsed
                .for_quality(40)
                .luma
                .iter()
                .zip(luma)
                .all(|(a, b)| *a >= b));
            assert_eq!(parsed.for_quality(100).luma, [1; 64]);
            for length in 0..jpeg.iter().position(|&byte| byte == 0xdb).unwrap() + 65 {
                assert!(JpegQuantization::read(&jpeg[..length]).is_none());
            }
        }
    }

    #[test]
    fn rejects_zero_and_16_bit_quantizers_and_rgb_component_layout() {
        let mut jpeg = Vec::new();
        Encoder::new(&mut jpeg, 80)
            .encode(&[10, 20, 30], 1, 1, ColorType::Rgb)
            .unwrap();
        let dqt = jpeg.windows(2).position(|v| v == [0xff, 0xdb]).unwrap();
        let mut invalid = jpeg.clone();
        invalid[dqt + 5] = 0;
        assert!(JpegQuantization::read(&invalid).is_none());
        let mut invalid = jpeg.clone();
        invalid[dqt + 4] = 0x10;
        assert!(JpegQuantization::read(&invalid).is_none());
        let sof = jpeg.windows(2).position(|v| v == [0xff, 0xc0]).unwrap();
        let mut invalid = jpeg.clone();
        invalid[sof + 10] = b'R';
        invalid[sof + 13] = b'G';
        invalid[sof + 16] = b'B';
        assert!(JpegQuantization::read(&invalid).is_none());
        assert!(JpegQuantization::read(b"not a jpeg").is_none());
    }
}
