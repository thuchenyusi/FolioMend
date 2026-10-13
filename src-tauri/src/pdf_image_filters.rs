//! Bounded decoding of PDF image filter chains. Color interpretation stays in pdf_processor.
use lopdf::{Dictionary, Document, Object, Stream};
use std::borrow::Cow;

pub enum ImageData<'a> {
    Samples(Cow<'a, [u8]>),
    Jpeg(Cow<'a, [u8]>),
    Jpeg2000(Cow<'a, [u8]>),
}

fn resolved<'a>(doc: &'a Document, value: &'a Object) -> Option<&'a Object> {
    doc.dereference(value).ok().map(|(_, value)| value)
}

pub fn filter_names(doc: &Document, stream: &Stream) -> Option<Vec<String>> {
    let names = match stream.dict.get(b"Filter") {
        Err(_) => return Some(Vec::new()),
        Ok(value) => match resolved(doc, value)? {
            Object::Name(name) => vec![name.as_slice()],
            Object::Array(values) => values
                .iter()
                .map(|value| resolved(doc, value)?.as_name().ok())
                .collect::<Option<Vec<_>>>()?,
            _ => return None,
        },
    };
    if names.len() > 8 {
        return None;
    }
    names
        .into_iter()
        .map(|name| {
            let name = std::str::from_utf8(name).ok()?;
            Some(
                match name {
                    "AHx" => "ASCIIHexDecode",
                    "A85" => "ASCII85Decode",
                    "LZW" => "LZWDecode",
                    "Fl" => "FlateDecode",
                    "RL" => "RunLengthDecode",
                    "DCT" => "DCTDecode",
                    "CCF" => "CCITTFaxDecode",
                    other => other,
                }
                .to_string(),
            )
        })
        .collect()
}

pub fn is_bilevel_codec(doc: &Document, stream: &Stream) -> bool {
    filter_names(doc, stream).is_some_and(|filters| {
        filters
            .iter()
            .any(|name| matches!(name.as_str(), "CCITTFaxDecode" | "JBIG2Decode"))
    })
}

fn parameters<'a>(
    doc: &'a Document,
    stream: &'a Stream,
    count: usize,
) -> Option<Vec<Option<&'a Dictionary>>> {
    let parameter = |value| match resolved(doc, value)? {
        Object::Null => Some(None),
        Object::Dictionary(dict) => Some(Some(dict)),
        _ => None,
    };
    match stream.dict.get(b"DecodeParms") {
        Err(_) => Some(vec![None; count]),
        Ok(value) => match resolved(doc, value)? {
            Object::Null => Some(vec![None; count]),
            Object::Dictionary(dict) if count == 1 => Some(vec![Some(dict)]),
            Object::Array(values) if values.len() == count => {
                values.iter().map(parameter).collect()
            }
            _ => None,
        },
    }
}

fn integer(params: Option<&Dictionary>, name: &[u8], default: i64) -> Option<i64> {
    match params.and_then(|dict| dict.get(name).ok()) {
        None => Some(default),
        Some(value) => value.as_i64().ok(),
    }
}

pub fn decode<'a>(
    doc: &Document,
    stream: &'a Stream,
    width: u32,
    height: u32,
    channels: Option<u8>,
    bits: u8,
) -> Option<ImageData<'a>> {
    let filters = filter_names(doc, stream)?;
    let params = parameters(doc, stream, filters.len())?;
    // Bound every intermediate buffer, including nested compression and predictors.
    let limit = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(channels.unwrap_or(3) as usize)?
        .checked_mul(2)?
        .checked_add(height as usize)?
        .checked_add(1_048_576)?
        .min(256_000_000);
    if stream.content.len() > limit {
        return None;
    }
    let mut data = Cow::Borrowed(stream.content.as_slice());
    for (index, filter) in filters.iter().enumerate() {
        let param = params[index];
        let is_last = index + 1 == filters.len();
        let raw = match filter.as_str() {
            "DCTDecode" | "JPXDecode" if is_last && param.is_none_or(Dictionary::is_empty) => {
                return Some(if filter == "DCTDecode" {
                    ImageData::Jpeg(data)
                } else {
                    ImageData::Jpeg2000(data)
                });
            }
            "FlateDecode" | "LZWDecode" => {
                if param.is_some_and(|dict| {
                    dict.iter().any(|(name, _)| {
                        !matches!(
                            name.as_slice(),
                            b"Predictor"
                                | b"Colors"
                                | b"BitsPerComponent"
                                | b"Columns"
                                | b"EarlyChange"
                        )
                    })
                }) {
                    return None;
                }
                let raw = if filter == "FlateDecode" {
                    decode_flate(data.as_ref(), limit)?
                } else {
                    decode_lzw(data.as_ref(), integer(param, b"EarlyChange", 1)?, limit)?
                };
                undo_predictor(raw, param, width, height, channels, bits)?
            }
            _ if param.is_some_and(|dict| !dict.is_empty()) => return None,
            "ASCIIHexDecode" => decode_hex(data.as_ref(), limit)?,
            "ASCII85Decode" => decode_ascii85(data.as_ref(), limit)?,
            "RunLengthDecode" => decode_run_length(data.as_ref(), limit)?,
            _ => return None,
        };
        data = Cow::Owned(raw);
    }
    Some(ImageData::Samples(data))
}

fn pdf_space(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

fn decode_hex(input: &[u8], limit: usize) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut high = None;
    let mut ended = false;
    for &byte in input {
        if pdf_space(byte) {
            continue;
        }
        if ended {
            return None;
        }
        if byte == b'>' {
            ended = true;
            continue;
        }
        let digit = (byte as char).to_digit(16)? as u8;
        if let Some(value) = high.take() {
            output.push(value * 16 + digit);
        } else {
            high = Some(digit);
        }
        if output.len() > limit {
            return None;
        }
    }
    if !ended {
        return None;
    }
    if let Some(value) = high {
        output.push(value * 16);
    }
    (output.len() <= limit).then_some(output)
}

fn decode_ascii85(input: &[u8], limit: usize) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut value = 0u64;
    let mut count = 0;
    let mut index = 0;
    let mut ended = false;
    while index < input.len() {
        let byte = input[index];
        index += 1;
        if pdf_space(byte) {
            continue;
        }
        if byte == b'~' {
            if input.get(index) != Some(&b'>')
                || input[index + 1..].iter().any(|byte| !pdf_space(*byte))
            {
                return None;
            }
            ended = true;
            break;
        }
        if byte == b'z' && count == 0 {
            output.extend_from_slice(&[0; 4]);
        } else if (b'!'..=b'u').contains(&byte) {
            value = value * 85 + u64::from(byte - b'!');
            count += 1;
            if count == 5 {
                output.extend_from_slice(&u32::try_from(value).ok()?.to_be_bytes());
                value = 0;
                count = 0;
            }
        } else {
            return None;
        }
        if output.len() > limit {
            return None;
        }
    }
    if !ended || count == 1 {
        return None;
    }
    if count > 1 {
        for _ in count..5 {
            value = value * 85 + 84;
        }
        output.extend_from_slice(&u32::try_from(value).ok()?.to_be_bytes()[..count - 1]);
    }
    (output.len() <= limit).then_some(output)
}

fn decode_run_length(input: &[u8], limit: usize) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut index = 0usize;
    loop {
        let length = *input.get(index)?;
        index += 1;
        if length == 128 {
            return Some(output);
        }
        let count = if length < 128 {
            usize::from(length) + 1
        } else {
            257 - usize::from(length)
        };
        if output.len().checked_add(count)? > limit {
            return None;
        }
        if length < 128 {
            output.extend_from_slice(input.get(index..index.checked_add(count)?)?);
            index += count;
        } else {
            output.resize(output.len() + count, *input.get(index)?);
            index += 1;
        }
    }
}

fn decode_flate(input: &[u8], limit: usize) -> Option<Vec<u8>> {
    use flate2::{Decompress, FlushDecompress, Status};
    let mut decoder = Decompress::new(true);
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let room = buffer
            .len()
            .min(limit.checked_sub(output.len())?.checked_add(1)?);
        let status = decoder
            .decompress(
                &input[before_in as usize..],
                &mut buffer[..room],
                FlushDecompress::None,
            )
            .ok()?;
        let produced = usize::try_from(decoder.total_out() - before_out).ok()?;
        output.extend_from_slice(&buffer[..produced]);
        if output.len() > limit {
            return None;
        }
        if status == Status::StreamEnd {
            return Some(output);
        }
        if decoder.total_in() == before_in && produced == 0 {
            return None;
        }
    }
}

fn decode_lzw(input: &[u8], early_change: i64, limit: usize) -> Option<Vec<u8>> {
    use weezl::{decode::Decoder, BitOrder, LzwStatus};
    let mut decoder = match early_change {
        0 => Decoder::new(BitOrder::Msb, 8),
        1 => Decoder::with_tiff_size_switch(BitOrder::Msb, 8),
        _ => return None,
    };
    let mut output = Vec::new();
    let mut consumed = 0;
    let mut buffer = [0u8; 8192];
    loop {
        let room = buffer
            .len()
            .min(limit.checked_sub(output.len())?.checked_add(1)?);
        let result = decoder.decode_bytes(&input[consumed..], &mut buffer[..room]);
        consumed += result.consumed_in;
        output.extend_from_slice(&buffer[..result.consumed_out]);
        if output.len() > limit {
            return None;
        }
        match result.status.ok()? {
            LzwStatus::Done => return Some(output),
            LzwStatus::NoProgress => return None,
            LzwStatus::Ok => {}
        }
    }
}

fn paeth(left: u8, up: u8, upper_left: u8) -> u8 {
    let p = i32::from(left) + i32::from(up) - i32::from(upper_left);
    let a = (p - i32::from(left)).abs();
    let b = (p - i32::from(up)).abs();
    let c = (p - i32::from(upper_left)).abs();
    if a <= b && a <= c {
        left
    } else if b <= c {
        up
    } else {
        upper_left
    }
}

fn sample(data: &[u8], index: usize, bits: usize) -> u16 {
    let offset = index * bits;
    if bits == 16 {
        u16::from_be_bytes([data[offset / 8], data[offset / 8 + 1]])
    } else {
        u16::from((data[offset / 8] >> (8 - bits - offset % 8)) & ((1u16 << bits) - 1) as u8)
    }
}

fn set_sample(data: &mut [u8], index: usize, bits: usize, value: u16) {
    let offset = index * bits;
    if bits == 16 {
        data[offset / 8..offset / 8 + 2].copy_from_slice(&value.to_be_bytes());
    } else {
        let shift = 8 - bits - offset % 8;
        let mask = (((1u16 << bits) - 1) as u8) << shift;
        data[offset / 8] = (data[offset / 8] & !mask) | ((value as u8) << shift);
    }
}

fn undo_predictor(
    mut data: Vec<u8>,
    params: Option<&Dictionary>,
    width: u32,
    height: u32,
    channels: Option<u8>,
    bits: u8,
) -> Option<Vec<u8>> {
    let predictor = integer(params, b"Predictor", 1)?;
    if predictor == 1 {
        return Some(data);
    }
    let channels = usize::from(channels?);
    let bits = usize::from(bits);
    if !matches!(bits, 1 | 2 | 4 | 8 | 16)
        || integer(params, b"Colors", 1)? != channels as i64
        || integer(params, b"BitsPerComponent", 8)? != bits as i64
        || integer(params, b"Columns", 1)? != i64::from(width)
    {
        return None;
    }
    let samples = (width as usize).checked_mul(channels)?;
    let row_bytes = samples.checked_mul(bits)?.div_ceil(8);
    if predictor == 2 {
        if data.len() != row_bytes.checked_mul(height as usize)? {
            return None;
        }
        let mask = ((1u32 << bits) - 1) as u16;
        for row in data.chunks_exact_mut(row_bytes) {
            for index in channels..samples {
                let value =
                    sample(row, index, bits).wrapping_add(sample(row, index - channels, bits))
                        & mask;
                set_sample(row, index, bits, value);
            }
        }
        return Some(data);
    }
    if !(10..=15).contains(&predictor)
        || data.len() != (row_bytes + 1).checked_mul(height as usize)?
    {
        return None;
    }
    let stride = channels.checked_mul(bits)?.div_ceil(8);
    let mut output = vec![0u8; row_bytes.checked_mul(height as usize)?];
    for (y, row) in data.chunks_exact(row_bytes + 1).enumerate() {
        let filter = row[0];
        if filter > 4 {
            return None;
        }
        for x in 0..row_bytes {
            let offset = y * row_bytes + x;
            let left = if x >= stride {
                output[offset - stride]
            } else {
                0
            };
            let up = if y > 0 { output[offset - row_bytes] } else { 0 };
            let upper_left = if y > 0 && x >= stride {
                output[offset - row_bytes - stride]
            } else {
                0
            };
            let adjustment = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth(left, up, upper_left),
                _ => unreachable!(),
            };
            output[offset] = row[x + 1].wrapping_add(adjustment);
        }
    }
    Some(output)
}

pub fn unpack_samples(
    data: &[u8],
    width: u32,
    height: u32,
    channels: u8,
    bits: u8,
) -> Option<Vec<u8>> {
    if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        return None;
    }
    let samples = (width as usize).checked_mul(channels as usize)?;
    let row_bytes = samples.checked_mul(bits as usize)?.div_ceil(8);
    if data.len() != row_bytes.checked_mul(height as usize)? {
        return None;
    }
    if bits == 8 {
        return Some(data.to_vec());
    }
    let mut pixels = Vec::with_capacity(samples.checked_mul(height as usize)?);
    let max = (1u32 << bits) - 1;
    for row in data.chunks_exact(row_bytes) {
        for index in 0..samples {
            let value = u32::from(sample(row, index, bits as usize));
            pixels.push(((value * 255 + max / 2) / max) as u8);
        }
    }
    Some(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;
    use std::io::Write;

    fn flate(input: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(input).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn ascii_and_run_length_validate_groups_end_markers_and_bounds() {
        assert_eq!(decode_hex(b"61 62\0 6>", 3).unwrap(), [97, 98, 96]);
        assert!(decode_hex(b"6162", 3).is_none());
        assert!(decode_hex(b"61x>", 3).is_none());
        assert!(decode_hex(b"616263>", 2).is_none());
        assert!(decode_hex(b"61>FF", 3).is_none());
        assert_eq!(decode_ascii85(b"z!!~>", 5).unwrap(), [0; 5]);
        assert_eq!(decode_ascii85(b"!!!!~>", 3).unwrap(), [0; 3]);
        for bytes in [
            b"!~>".as_slice(),
            b"!z~>",
            b"uuuuu~>",
            b"!!!!!",
            b"z~>x",
            b"z~",
        ] {
            assert!(decode_ascii85(bytes, 10).is_none(), "{bytes:?}");
        }
        assert!(decode_ascii85(b"z~>", 3).is_none());
        assert_eq!(
            decode_run_length(&[2, 97, 98, 99, 254, 100, 128], 6).unwrap(),
            b"abcddd"
        );
        assert!(decode_run_length(&[2, 97], 6).is_none());
        assert!(decode_run_length(&[255, 10], 6).is_none());
        assert!(decode_run_length(&[254, 100, 128], 2).is_none());
    }

    #[test]
    fn lzw_handles_both_width_switches_resets_truncation_and_limits() {
        use weezl::{encode::Encoder, BitOrder};
        let input: Vec<_> = (0..80_000u32)
            .map(|index| ((index * 173 + (index >> 3) * 71) % 256) as u8)
            .collect();
        for early in [0, 1] {
            let mut encoder = if early == 1 {
                Encoder::with_tiff_size_switch(BitOrder::Msb, 8)
            } else {
                Encoder::new(BitOrder::Msb, 8)
            };
            let encoded = encoder.encode(&input).unwrap();
            assert_eq!(decode_lzw(&encoded, early, input.len()).unwrap(), input);
            assert!(decode_lzw(&encoded, early, input.len() - 1).is_none());
            assert!(decode_lzw(&encoded[..encoded.len() / 2], early, input.len()).is_none());
        }
        assert!(decode_lzw(&[], 1, 100).is_none());
        assert!(decode_lzw(&[255; 8], 1, 100).is_none());
        assert!(decode_lzw(&[0], 2, 100).is_none());
    }

    #[test]
    fn flate_rejects_incomplete_checksums_and_expansion_beyond_limit() {
        let input = vec![42; 20_000];
        let encoded = flate(&input);
        assert_eq!(decode_flate(&encoded, input.len()).unwrap(), input);
        assert!(decode_flate(&encoded, 100).is_none());
        assert!(decode_flate(&encoded[..encoded.len() - 2], input.len()).is_none());
        let mut corrupt = encoded;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(decode_flate(&corrupt, input.len()).is_none());
    }

    #[test]
    fn predictors_restore_known_png_rows_and_packed_tiff_samples() {
        // All five PNG filters restore the same two grayscale rows.
        let cases = [
            vec![0, 10, 20, 30, 0, 15, 25, 35],
            vec![1, 10, 10, 10, 1, 15, 10, 10],
            vec![2, 10, 20, 30, 2, 5, 5, 5],
            vec![3, 10, 15, 20, 3, 10, 8, 8],
            vec![4, 10, 10, 10, 4, 5, 5, 5],
        ];
        for (index, encoded) in cases.into_iter().enumerate() {
            let params = dictionary! {"Predictor" => 10 + index as i64, "Columns" => 3};
            assert_eq!(
                undo_predictor(encoded, Some(&params), 3, 2, Some(1), 8).unwrap(),
                [10, 20, 30, 15, 25, 35]
            );
        }
        let params = dictionary! {"Predictor" => 15, "Columns" => 2, "Colors" => 3};
        assert_eq!(
            undo_predictor(
                vec![1, 10, 20, 30, 30, 30, 30],
                Some(&params),
                2,
                1,
                Some(3),
                8
            )
            .unwrap(),
            [10, 20, 30, 40, 50, 60]
        );
        let params = dictionary! {"Predictor" => 2, "Columns" => 3, "BitsPerComponent" => 4};
        assert_eq!(
            undo_predictor(vec![0x12, 0x40], Some(&params), 3, 1, Some(1), 4).unwrap(),
            [0x13, 0x70]
        );
        let params = dictionary! {"Predictor" => 2, "Columns" => 3, "BitsPerComponent" => 16};
        assert_eq!(
            undo_predictor(
                vec![0, 100, 0, 200, 255, 156],
                Some(&params),
                3,
                1,
                Some(1),
                16
            )
            .unwrap(),
            [0, 100, 1, 44, 0, 200]
        );
        let params = dictionary! {"Predictor" => 15, "Columns" => 3};
        assert!(undo_predictor(vec![5, 1, 2, 3], Some(&params), 3, 1, Some(1), 8).is_none());
        assert!(undo_predictor(vec![0, 1, 2], Some(&params), 3, 1, Some(1), 8).is_none());
        assert!(undo_predictor(vec![0, 1, 2, 3], Some(&params), 4, 1, Some(1), 8).is_none());
    }

    #[test]
    fn unpacking_respects_row_padding_gray_rgb_and_16_bit_samples() {
        assert_eq!(
            unpack_samples(&[0x18, 0xe4], 3, 2, 1, 2).unwrap(),
            [0, 85, 170, 255, 170, 85]
        );
        assert_eq!(
            unpack_samples(&[0x05, 0xf0], 3, 1, 1, 4).unwrap(),
            [0, 85, 255]
        );
        assert_eq!(
            unpack_samples(&[0x18, 0xc0], 2, 1, 3, 2).unwrap(),
            [0, 85, 170, 0, 255, 0]
        );
        assert_eq!(
            unpack_samples(&[0, 0, 0x80, 0, 255, 255], 3, 1, 1, 16).unwrap(),
            [0, 128, 255]
        );
        assert!(unpack_samples(&[0], 2, 1, 1, 8).is_none());
        assert!(unpack_samples(&[0, 1, 2], 2, 1, 1, 8).is_none());
    }

    #[test]
    fn chains_resolve_abbreviations_and_per_filter_indirect_parameters() {
        let mut doc = Document::new();
        let params_id = doc.add_object(dictionary! {"Predictor" => 15, "Columns" => 3});
        let content: Vec<u8> = flate(&[1, 10, 10, 10])
            .iter()
            .flat_map(|byte| format!("{byte:02x}").into_bytes())
            .chain([b'>'])
            .collect();
        let mut stream = Stream::new(
            dictionary! {
                "Filter" => vec![Object::Name(b"AHx".to_vec()), Object::Name(b"Fl".to_vec())],
                "DecodeParms" => vec![Object::Null, Object::Reference(params_id)],
            },
            content,
        );
        let decoded = decode(&doc, &stream, 3, 1, Some(1), 8).unwrap();
        let ImageData::Samples(samples) = decoded else {
            panic!("expected samples")
        };
        assert_eq!(samples.as_ref(), [10, 20, 30]);
        stream.dict.set("DecodeParms", vec![Object::Null]);
        assert!(decode(&doc, &stream, 3, 1, Some(1), 8).is_none());
        stream.dict.set("Filter", Object::Null);
        assert!(decode(&doc, &stream, 3, 1, Some(1), 8).is_none());
        stream.dict.set(
            "Filter",
            vec![
                Object::Name(b"DCTDecode".to_vec()),
                Object::Name(b"FlateDecode".to_vec()),
            ],
        );
        stream.dict.remove(b"DecodeParms");
        assert!(decode(&doc, &stream, 3, 1, Some(1), 8).is_none());
    }
}
