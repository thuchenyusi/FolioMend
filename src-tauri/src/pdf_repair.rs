use lopdf::{Document, ObjectId};

struct XrefEntry {
    id: ObjectId,
    offset: usize,
    field: usize,
}

fn number(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn token<'a>(data: &'a [u8], cursor: &mut usize) -> Option<(usize, &'a [u8])> {
    while data.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
        *cursor += 1;
    }
    let start = *cursor;
    while data.get(*cursor).is_some_and(|b| !b.is_ascii_whitespace()) {
        *cursor += 1;
    }
    (start < *cursor).then(|| (start, &data[start..*cursor]))
}

fn object_at(data: &[u8], offset: usize, id: ObjectId) -> bool {
    let mut cursor = offset;
    let Some((start, first)) = token(data, &mut cursor) else {
        return false;
    };
    start == offset
        && number(first) == Some(id.0 as usize)
        && token(data, &mut cursor).and_then(|(_, t)| number(t)) == Some(id.1 as usize)
        && token(data, &mut cursor).is_some_and(|(_, t)| {
            t.starts_with(b"obj") && (t.len() == 3 || matches!(t[3], b'<' | b'[' | b'(' | b'/'))
        })
}

fn xref_entries(data: &[u8], offset: usize, end: usize) -> Result<Vec<XrefEntry>, String> {
    let mut cursor = offset;
    if token(data, &mut cursor).map(|(_, t)| t) != Some(b"xref".as_slice()) {
        return Err("未找到可修复的普通索引表".into());
    }
    let mut entries = Vec::new();
    loop {
        let (_, first) = token(data, &mut cursor).ok_or("索引表不完整")?;
        if first == b"trailer" {
            return Ok(entries);
        }
        let first = number(first).ok_or("索引表段起点无效")?;
        let count = token(data, &mut cursor)
            .and_then(|(_, t)| number(t))
            .ok_or("索引表段长度无效")?;
        // Bound allocations and work by the actual bytes left in the table.
        if count > end.saturating_sub(cursor) / 18 || first.checked_add(count).is_none() {
            return Err("索引表段长度超出文件范围".into());
        }
        for id in first..first + count {
            let (field, off) = token(data, &mut cursor).ok_or("索引条目不完整")?;
            let offset = number(off).ok_or("索引偏移无效")?;
            let generation = token(data, &mut cursor)
                .and_then(|(_, t)| number(t))
                .ok_or("对象代数无效")?;
            let (_, state) = token(data, &mut cursor).ok_or("索引条目状态缺失")?;
            if off.len() != 10 || generation > u16::MAX as usize || id > u32::MAX as usize {
                return Err("不支持的索引条目格式".into());
            }
            match state {
                b"n" => entries.push(XrefEntry {
                    id: (id as u32, generation as u16),
                    offset,
                    field,
                }),
                b"f" => {}
                _ => return Err("索引条目状态无效".into()),
            }
            if cursor >= end {
                return Err("索引表缺少 trailer".into());
            }
        }
    }
}

fn has_trailer_key(data: &[u8], key: &[u8]) -> bool {
    data.windows(key.len()).enumerate().any(|(i, t)| {
        t == key
            && data.get(i + key.len()).map_or(true, |b| {
                b.is_ascii_whitespace() || matches!(b, b'/' | b'<' | b'>' | b'[' | b']')
            })
    })
}

/// Repair only offsets that can be verified against the exact object ID and
/// generation. Never scan binary streams for arbitrary replacement objects.
pub fn repair_and_load(data: &[u8]) -> Result<Document, String> {
    let window = data.len().saturating_sub(1024 * 1024);
    let sx = data[window..]
        .windows(9)
        .rposition(|t| t == b"startxref")
        .map(|p| p + window)
        .ok_or("文件末尾缺少 startxref")?;
    let mut cursor = sx + 9;
    let (_, declared) = token(data, &mut cursor).ok_or("startxref 缺少偏移")?;
    let declared = number(declared).ok_or("startxref 偏移无效")?;
    let (suffix, eof) = token(data, &mut cursor).ok_or("文件末尾缺少 %%EOF")?;
    if eof != b"%%EOF" {
        return Err("文件末尾缺少 %%EOF".into());
    }

    // Use the last line-delimited classic xref preceding the final trailer.
    let xref = data[window..sx]
        .windows(4)
        .enumerate()
        .rfind(|(i, t)| {
            *t == b"xref"
                && (*i + window == 0 || matches!(data[*i + window - 1], b'\r' | b'\n'))
                && data
                    .get(*i + window + 4)
                    .is_some_and(u8::is_ascii_whitespace)
        })
        .map(|(i, _)| window + i);
    let mut repaired = data.to_vec();
    let entries = if let Some(xref) = xref {
        let entries = xref_entries(&data[..sx], xref, sx)?;
        let trailer = &data[xref..sx];
        // Offset repair of revision chains or hybrid xrefs requires separate
        // validation of every revision; refuse rather than lose older objects.
        if has_trailer_key(trailer, b"/Prev") || has_trailer_key(trailer, b"/XRefStm") {
            return Err("暂不支持自动修复增量或混合索引表".into());
        }
        let delta = xref as i128 - declared as i128;
        for entry in &entries {
            if object_at(data, entry.offset, entry.id) {
                continue;
            }
            let shifted = entry.offset as i128 + delta;
            let shifted = usize::try_from(shifted)
                .ok()
                .filter(|&p| p < xref)
                .filter(|&p| object_at(data, p, entry.id))
                .ok_or_else(|| format!("无法确认对象 {} {} 的实际位置", entry.id.0, entry.id.1))?;
            let value = format!("{shifted:010}");
            if value.len() != 10 {
                return Err("索引偏移超出普通索引表范围".into());
            }
            repaired[entry.field..entry.field + 10].copy_from_slice(value.as_bytes());
        }
        repaired.truncate(sx);
        repaired.extend_from_slice(format!("startxref\n{xref}\n").as_bytes());
        repaired.extend_from_slice(&data[suffix..]);
        entries
    } else {
        // Preserve the previous same-line startxref repair for xref streams.
        repaired.truncate(sx);
        repaired.extend_from_slice(format!("startxref\n{declared}\n").as_bytes());
        repaired.extend_from_slice(&data[suffix..]);
        Vec::new()
    };
    let doc = Document::load_mem(&repaired).map_err(|e| format!("修复后仍无法加载: {e}"))?;
    if entries.iter().any(|e| !doc.objects.contains_key(&e.id)) {
        return Err("修复后仍有对象无法解析".into());
    }
    let catalog = doc.catalog().map_err(|_| "修复后缺少有效文档目录")?;
    let pages = catalog
        .get(b"Pages")
        .and_then(|o| o.as_reference())
        .and_then(|id| doc.get_dictionary(id))
        .map_err(|_| "修复后缺少页面树")?;
    let expected = pages
        .get(b"Count")
        .and_then(|o| o.as_i64())
        .map_err(|_| "修复后缺少有效页数")?;
    let actual = doc.get_pages();
    if expected <= 0 || actual.len() as i64 != expected {
        return Err("修复后页面树不完整，已停止处理".into());
    }
    Ok(doc)
}

#[cfg(test)]
pub(crate) fn shifted_fixture() -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    doc.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
    doc.objects.insert(
        (1, 0),
        lopdf::dictionary! {
            "Type" => "Catalog", "Pages" => (2, 0)
        }
        .into(),
    );
    doc.objects.insert(
        (2, 0),
        lopdf::dictionary! {
            "Type" => "Pages", "Count" => 1, "Kids" => vec![(3, 0).into()]
        }
        .into(),
    );
    doc.objects.insert(
        (3, 0),
        lopdf::dictionary! {
            "Type" => "Page", "Parent" => (2, 0), "Contents" => (4, 0),
            "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()]
        }
        .into(),
    );
    doc.objects.insert(
        (4, 0),
        lopdf::Stream::new(lopdf::Dictionary::new(), b"q 1 0 0 1 0 0 cm Q".to_vec()).into(),
    );
    doc.max_id = 4;
    doc.trailer.set("Root", (1, 0));
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    let insert = bytes.windows(7).position(|w| w == b"2 0 obj").unwrap();
    let mut comment = vec![b'%'; 78];
    comment[77] = b'\n';
    bytes.splice(insert..insert, comment);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace(bytes: &mut Vec<u8>, old: &[u8], new: &[u8]) {
        let pos = bytes.windows(old.len()).rposition(|w| w == old).unwrap();
        bytes.splice(pos..pos + old.len(), new.iter().copied());
    }

    #[test]
    fn repairs_mixed_valid_and_shifted_offsets_without_changing_content() {
        let bytes = shifted_fixture();
        assert!(Document::load_mem(&bytes).is_err());
        let doc = repair_and_load(&bytes).unwrap();
        assert_eq!(doc.objects.len(), 4);
        assert_eq!(doc.get_pages().len(), 1);
        assert_eq!(
            doc.get_object((4, 0)).unwrap().as_stream().unwrap().content,
            b"q 1 0 0 1 0 0 cm Q"
        );
    }

    #[test]
    fn repairs_same_line_startxref_and_negative_offset_shift() {
        let shifted = shifted_fixture();
        let mut doc = repair_and_load(&shifted).unwrap();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        replace(&mut bytes, b"startxref\n", b"startxref ");
        assert_eq!(repair_and_load(&bytes).unwrap().get_pages().len(), 1);

        // Simulate deletion of 78 bytes before every object: all recorded
        // offsets now point 78 bytes beyond the actual positions.
        let xref = bytes.windows(5).rposition(|w| w == b"xref\n").unwrap();
        let sx = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
        for entry in xref_entries(&bytes[..sx], xref, sx).unwrap() {
            bytes[entry.field..entry.field + 10]
                .copy_from_slice(format!("{:010}", entry.offset + 78).as_bytes());
        }
        bytes.truncate(sx);
        bytes.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref + 78).as_bytes());
        assert_eq!(repair_and_load(&bytes).unwrap().get_pages().len(), 1);
    }

    #[test]
    fn retains_startxref_format_repair_for_xref_streams() {
        let mut doc = repair_and_load(&shifted_fixture()).unwrap();
        doc.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceStream;
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        replace(&mut bytes, b"startxref\n", b"startxref ");
        assert_eq!(repair_and_load(&bytes).unwrap().get_pages().len(), 1);
    }

    #[test]
    fn rejects_unverifiable_objects_revision_chains_and_incomplete_page_trees() {
        let mut bad = shifted_fixture();
        replace(&mut bad, b"4 0 obj", b"9 0 obj");
        assert!(repair_and_load(&bad).unwrap_err().contains("无法确认对象"));

        let mut incremental = shifted_fixture();
        replace(&mut incremental, b"/Root", b"/Prev 1 /Root");
        assert!(repair_and_load(&incremental).unwrap_err().contains("增量"));

        let mut incomplete = shifted_fixture();
        replace(&mut incomplete, b"/Count 1", b"/Count 2");
        assert!(repair_and_load(&incomplete)
            .unwrap_err()
            .contains("页面树不完整"));
        assert!(repair_and_load(b"not a PDF").is_err());
    }
}
