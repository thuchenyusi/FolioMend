use flate2::read::ZlibDecoder;
use image::codecs::jpeg::JpegEncoder;
use image::ExtendedColorType;
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessResult {
    pub input_path: String,
    pub output_path: String,
    pub page_count: usize,
    pub original_widths: Vec<f64>,
    pub target_width: f64,
    pub color_images: usize,
    pub grayscale_images: usize,
    pub monochrome_images: usize,
    pub optimized_images: usize,
    pub skipped_images: usize,
    pub original_bytes: u64,
    pub output_bytes: u64,
    pub bookmark_status: String,
    pub bookmarks_fit_width: usize,
    pub bookmarks_fit_width_skipped: usize,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessingProgress {
    pub input_path: String,
    pub phase: String,
    pub page: usize,
    pub page_count: usize,
    pub image: usize,
    pub image_count: usize,
    pub progress: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CropBoxMode {
    AlignVisible,
    TrueCrop,
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProcessOptions {
    pub compress_images: bool,
    pub normalize_pages: bool,
    pub crop_mode: CropBoxMode,
    pub repair_bookmarks: bool,
    pub fit_bookmarks_to_width: bool,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            compress_images: true,
            normalize_pages: true,
            crop_mode: CropBoxMode::AlignVisible,
            repair_bookmarks: true,
            fit_bookmarks_to_width: false,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct ImageOptimizationSummary {
    pub color_images: usize,
    pub grayscale_images: usize,
    pub monochrome_images: usize,
    pub optimized_images: usize,
    pub skipped_images: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageKind {
    Color,
    Grayscale,
    Monochrome,
}

#[derive(Debug)]
struct DecodedImage {
    width: u32,
    height: u32,
    channels: u8,
    pixels: Vec<u8>,
}

#[derive(Debug)]
struct ImageTransform {
    stream: Stream,
}

#[derive(Debug)]
struct ImageWorkResult {
    image_id: ObjectId,
    kind: Option<ImageKind>,
    transform: Option<ImageTransform>,
    skipped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BoxRect {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl BoxRect {
    fn width(self) -> f64 {
        self.x2 - self.x1
    }

    fn height(self) -> f64 {
        self.y2 - self.y1
    }

    fn transformed(self, transform: PageTransform) -> Self {
        Self {
            x1: self.x1 * transform.scale + transform.tx,
            y1: self.y1 * transform.scale + transform.ty,
            x2: self.x2 * transform.scale + transform.tx,
            y2: self.y2 * transform.scale + transform.ty,
        }
    }

    fn as_object(self) -> Object {
        Object::Array(vec![
            Object::Real(self.x1 as f32),
            Object::Real(self.y1 as f32),
            Object::Real(self.x2 as f32),
            Object::Real(self.y2 as f32),
        ])
    }

    fn approximately_equals(self, other: Self) -> bool {
        (self.x1 - other.x1).abs() < 0.01
            && (self.y1 - other.y1).abs() < 0.01
            && (self.x2 - other.x2).abs() < 0.01
            && (self.y2 - other.y2).abs() < 0.01
    }
}

#[derive(Debug, Clone, Copy)]
struct PageTransform {
    scale: f64,
    tx: f64,
    ty: f64,
}

// Conservative thresholds: pages with a small amount of anti-aliasing remain grayscale.
const COLOR_DIFFERENCE_THRESHOLD: u8 = 10;
const MONOCHROME_MID_TONE_RATIO: f32 = 0.02;
const MIN_SAVING_RATIO: f32 = 0.02;
const EBOOK_MAX_LONG_EDGE: u32 = 2000;
const GRAYSCALE_JPEG_QUALITY: u8 = 60;

fn resolve_array<'a>(doc: &'a Document, obj: &'a Object) -> Option<Vec<Object>> {
    match obj {
        Object::Array(arr) => Some(arr.clone()),
        Object::Reference(id) => {
            if let Ok(resolved) = doc.get_object(*id) {
                if let Object::Array(arr) = resolved {
                    return Some(arr.clone());
                }
            }
            None
        }
        _ => None,
    }
}

fn get_number(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(f) => Some((*f).into()),
        _ => None,
    }
}

fn array_to_box(array: &[Object]) -> Option<BoxRect> {
    if array.len() < 4 {
        return None;
    }
    let rect = BoxRect {
        x1: get_number(&array[0])?,
        y1: get_number(&array[1])?,
        x2: get_number(&array[2])?,
        y2: get_number(&array[3])?,
    };
    (rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
}

fn get_inherited_box(doc: &Document, mut node_id: ObjectId, key: &[u8]) -> Option<BoxRect> {
    loop {
        let dict = doc.get_object(node_id).ok()?.as_dict().ok()?;
        if let Ok(value) = dict.get(key) {
            return resolve_array(doc, value).and_then(|array| array_to_box(&array));
        }
        node_id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
}

fn get_page_boxes(doc: &Document, page_id: ObjectId) -> Option<(BoxRect, BoxRect, bool)> {
    let media_box = get_inherited_box(doc, page_id, b"MediaBox")?;
    let crop_box = get_inherited_box(doc, page_id, b"CropBox");
    Some((media_box, crop_box.unwrap_or(media_box), crop_box.is_some()))
}

fn get_visible_page_width(doc: &Document, page_id: ObjectId) -> Option<f64> {
    let (_, crop_box, _) = get_page_boxes(doc, page_id)?;
    Some(if page_has_quarter_turn(doc, page_id) {
        crop_box.height()
    } else {
        crop_box.width()
    })
}

// Rotate is inheritable. A quarter turn swaps the displayed width and height.
fn page_has_quarter_turn(doc: &Document, mut node_id: ObjectId) -> bool {
    let mut visited = BTreeSet::new();
    while visited.insert(node_id) {
        let Ok(dict) = doc.get_dictionary(node_id) else {
            break;
        };
        if let Ok(value) = dict.get(b"Rotate") {
            let rotation = doc
                .dereference(value)
                .ok()
                .and_then(|(_, value)| value.as_i64().ok())
                .unwrap_or(0)
                .rem_euclid(360);
            return rotation == 90 || rotation == 270;
        }
        let Some(parent) = dict.get(b"Parent").ok().and_then(|v| v.as_reference().ok()) else {
            break;
        };
        node_id = parent;
    }
    false
}

/// 使用设计文档中的 P95 稳健策略计算目标宽度。
fn calculate_target_width(widths: &[f64]) -> f64 {
    if widths.is_empty() {
        return 0.0;
    }
    if widths.len() == 1 {
        return widths[0];
    }

    let mut sorted = widths.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let max_width = *sorted.last().unwrap();
    let p95_idx = ((sorted.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    let p95_width = sorted[p95_idx.min(sorted.len() - 1)];

    if max_width > 1.2 * p95_width {
        p95_width
    } else {
        max_width
    }
}

fn wrap_page_contents(
    doc: &mut Document,
    page_id: ObjectId,
    transform: PageTransform,
) -> Result<(), String> {
    let original_contents = doc
        .get_object(page_id)
        .map_err(|e| format!("无法获取页面对象: {e}"))?
        .as_dict()
        .map_err(|_| "页面对象不是字典".to_string())?
        .get(b"Contents")
        .ok()
        .cloned();
    let Some(original_contents) = original_contents else {
        return Ok(());
    };

    let pre_content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    Object::Real(transform.scale as f32),
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(transform.scale as f32),
                    Object::Real(transform.tx as f32),
                    Object::Real(transform.ty as f32),
                ],
            ),
        ],
    };
    let post_content = Content {
        operations: vec![Operation::new("Q", vec![])],
    };
    let pre_id = doc.add_object(Stream::new(
        Dictionary::new(),
        pre_content
            .encode()
            .map_err(|e| format!("编码前置流失败: {e}"))?,
    ));
    let post_id = doc.add_object(Stream::new(
        Dictionary::new(),
        post_content
            .encode()
            .map_err(|e| format!("编码后置流失败: {e}"))?,
    ));

    let mut new_contents = vec![Object::Reference(pre_id)];
    match original_contents {
        Object::Array(contents) => new_contents.extend(contents),
        content => new_contents.push(content),
    }
    new_contents.push(Object::Reference(post_id));
    doc.get_object_mut(page_id)
        .map_err(|e| format!("无法获取页面对象: {e}"))?
        .as_dict_mut()
        .map_err(|_| "页面对象不是字典".to_string())?
        .set("Contents", Object::Array(new_contents));
    Ok(())
}

/// 以实际可见框为基准处理页面。返回的矩阵用于同步书签目标坐标。
fn transform_page_geometry(
    doc: &mut Document,
    page_id: ObjectId,
    target_visible_width: Option<f64>,
    crop_mode: CropBoxMode,
) -> Result<Option<PageTransform>, String> {
    let (media_box, crop_box, has_crop_box) =
        get_page_boxes(doc, page_id).ok_or("无法获取页面框")?;
    let scale = target_visible_width
        .map(|target| {
            target
                / if page_has_quarter_turn(doc, page_id) {
                    crop_box.height()
                } else {
                    crop_box.width()
                }
        })
        .unwrap_or(1.0);
    let transform = PageTransform {
        scale,
        tx: crop_box.x1 * (1.0 - scale),
        ty: crop_box.y1 * (1.0 - scale),
    };
    let needs_scaling = (scale - 1.0).abs() >= 0.000_01;
    let needs_true_crop =
        crop_mode == CropBoxMode::TrueCrop && !media_box.approximately_equals(crop_box);
    if !needs_scaling && !needs_true_crop {
        return Ok(None);
    }

    if needs_scaling {
        wrap_page_contents(doc, page_id, transform)?;
    }

    let transformed_crop = crop_box.transformed(transform);
    let transformed_media = if crop_mode == CropBoxMode::TrueCrop {
        transformed_crop
    } else {
        media_box.transformed(transform)
    };
    let page = doc
        .get_object_mut(page_id)
        .map_err(|e| format!("无法获取页面对象: {e}"))?
        .as_dict_mut()
        .map_err(|_| "页面对象不是字典".to_string())?;
    page.set("MediaBox", transformed_media.as_object());
    if has_crop_box || crop_mode == CropBoxMode::TrueCrop {
        page.set("CropBox", transformed_crop.as_object());
    }

    Ok(needs_scaling.then_some(transform))
}

fn transform_destination_array(
    array: &mut [Object],
    transforms: &std::collections::BTreeMap<ObjectId, PageTransform>,
) {
    let Some(page_id) = array.first().and_then(|value| value.as_reference().ok()) else {
        return;
    };
    let Some(transform) = transforms.get(&page_id).copied() else {
        return;
    };
    let Some(kind) = array
        .get(1)
        .and_then(|value| value.as_name().ok())
        .map(<[u8]>::to_vec)
    else {
        return;
    };
    let mut transform_number = |index: usize, horizontal: bool| {
        let Some(value) = array.get_mut(index) else {
            return;
        };
        let Some(number) = get_number(value) else {
            return;
        };
        *value = Object::Real(
            (number * transform.scale
                + if horizontal {
                    transform.tx
                } else {
                    transform.ty
                }) as f32,
        );
    };
    match kind.as_slice() {
        b"XYZ" => {
            transform_number(2, true);
            transform_number(3, false);
        }
        b"FitH" | b"FitBH" => transform_number(2, false),
        b"FitV" | b"FitBV" => transform_number(2, true),
        b"FitR" => {
            transform_number(2, true);
            transform_number(3, false);
            transform_number(4, true);
            transform_number(5, false);
        }
        _ => {}
    }
}

fn update_destination_coordinates(
    object: &mut Object,
    transforms: &std::collections::BTreeMap<ObjectId, PageTransform>,
) {
    match object {
        Object::Array(array) => {
            transform_destination_array(array, transforms);
            for value in array {
                update_destination_coordinates(value, transforms);
            }
        }
        Object::Dictionary(dict) => {
            for (_, value) in dict.iter_mut() {
                update_destination_coordinates(value, transforms);
            }
        }
        Object::Stream(stream) => {
            for (_, value) in stream.dict.iter_mut() {
                update_destination_coordinates(value, transforms);
            }
        }
        _ => {}
    }
}

/// 生成输出文件名：添加 "_normalized" 后缀，重名时自动递增。
fn image_filter_names(stream: &Stream) -> Vec<String> {
    stream.filters().unwrap_or_default()
}

fn image_color_space(doc: &Document, stream: &Stream) -> Option<Vec<u8>> {
    let value = stream.dict.get(b"ColorSpace").ok()?;
    let value = match value {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        other => other,
    };
    match value {
        Object::Name(name) => Some(name.clone()),
        Object::Array(array) => array.first().and_then(|obj| match obj {
            Object::Name(name) => Some(name.clone()),
            _ => None,
        }),
        _ => None,
    }
}

fn has_non_default_decode(stream: &Stream) -> bool {
    stream.dict.get(b"Decode").is_ok() || stream.dict.get(b"DecodeParms").is_ok()
}

fn decode_flate_image(
    doc: &Document,
    stream: &Stream,
    width: u32,
    height: u32,
    bits_per_component: u8,
) -> Option<DecodedImage> {
    if has_non_default_decode(stream) || !matches!(bits_per_component, 1 | 8) {
        return None;
    }

    let color_space = image_color_space(doc, stream)?;
    let channels = match color_space.as_slice() {
        b"DeviceGray" => 1,
        b"DeviceRGB" => 3,
        _ => return None,
    };

    let mut decoder = ZlibDecoder::new(stream.content.as_slice());
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).ok()?;

    let pixel_count = usize::try_from(width.checked_mul(height)?).ok()?;
    if bits_per_component == 8 {
        let expected = pixel_count.checked_mul(channels as usize)?;
        if decoded.len() < expected {
            return None;
        }
        return Some(DecodedImage {
            width,
            height,
            channels,
            pixels: decoded[..expected].to_vec(),
        });
    }

    if channels != 1 {
        return None;
    }
    let row_bytes = (width as usize + 7) / 8;
    if decoded.len() < row_bytes.checked_mul(height as usize)? {
        return None;
    }

    // Expand a 1-bit PDF image to 8-bit grayscale for classification.
    let mut pixels = Vec::with_capacity(pixel_count);
    for y in 0..height as usize {
        let row = &decoded[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..width as usize {
            let bit = (row[x / 8] >> (7 - (x % 8))) & 1;
            pixels.push(if bit == 0 { 0 } else { 255 });
        }
    }
    Some(DecodedImage {
        width,
        height,
        channels: 1,
        pixels,
    })
}

fn decode_image(doc: &Document, stream: &Stream) -> Option<DecodedImage> {
    let width = stream.dict.get(b"Width").ok()?.as_i64().ok()?;
    let height = stream.dict.get(b"Height").ok()?.as_i64().ok()?;
    let bits = stream
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|obj| obj.as_i64().ok())
        .unwrap_or(8);
    let width = u32::try_from(width).ok()?;
    let height = u32::try_from(height).ok()?;
    let bits = u8::try_from(bits).ok()?;

    // Avoid allocating an unexpectedly large buffer from a malformed PDF.
    if width == 0 || height == 0 || width.checked_mul(height)? > 80_000_000 {
        return None;
    }

    let filters = image_filter_names(stream);
    if filters.len() == 1 && filters[0] == "DCTDecode" {
        let image = image::load_from_memory(stream.content.as_slice()).ok()?;
        let rgb = image.to_rgb8();
        let (decoded_width, decoded_height) = rgb.dimensions();
        return Some(DecodedImage {
            width: decoded_width,
            height: decoded_height,
            channels: 3,
            pixels: rgb.into_raw(),
        });
    }

    if filters.len() == 1 && filters[0] == "FlateDecode" {
        return decode_flate_image(doc, stream, width, height, bits);
    }

    None
}

fn classify_image(image: &DecodedImage) -> (ImageKind, Vec<u8>) {
    let pixel_count = (image.width as usize).saturating_mul(image.height as usize);
    let sample_step = (pixel_count / 50_000).max(1);
    let mut samples = 0usize;
    let mut colored = 0usize;
    let mut mid_tones = 0usize;

    // First pass is sampled. Color pages can return immediately without allocating a gray plane.
    for pixel_index in (0..pixel_count).step_by(sample_step) {
        let offset = pixel_index * image.channels as usize;
        let value = if image.channels == 1 {
            image.pixels[offset]
        } else {
            let r = image.pixels[offset];
            let g = image.pixels[offset + 1];
            let b = image.pixels[offset + 2];
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            if max.saturating_sub(min) > COLOR_DIFFERENCE_THRESHOLD {
                colored += 1;
            }
            ((u16::from(r) * 77 + u16::from(g) * 150 + u16::from(b) * 29) / 256) as u8
        };
        if (32..224).contains(&value) {
            mid_tones += 1;
        }
        samples += 1;
    }

    if samples == 0 {
        return (ImageKind::Grayscale, Vec::new());
    }
    if colored as f32 / samples as f32 > 0.005 {
        return (ImageKind::Color, Vec::new());
    }

    let kind = if mid_tones as f32 / samples as f32 <= MONOCHROME_MID_TONE_RATIO {
        ImageKind::Monochrome
    } else {
        ImageKind::Grayscale
    };

    // Only neutral images need a complete grayscale plane for re-encoding.
    let gray = if image.channels == 1 {
        image.pixels[..pixel_count].to_vec()
    } else {
        let mut gray = Vec::with_capacity(pixel_count);
        for pixel in image.pixels.chunks_exact(3).take(pixel_count) {
            gray.push(
                ((u16::from(pixel[0]) * 77 + u16::from(pixel[1]) * 150 + u16::from(pixel[2]) * 29)
                    / 256) as u8,
            );
        }
        gray
    };
    (kind, gray)
}

fn pack_monochrome(gray: &[u8], width: u32) -> Vec<u8> {
    if width == 0 || gray.is_empty() {
        return Vec::new();
    }
    let row_bytes = (width as usize + 7) / 8;
    let height = gray.len().div_ceil(width as usize);
    let mut packed = vec![0u8; row_bytes * height];
    for (index, value) in gray.iter().enumerate() {
        if *value >= 160 {
            packed[index / width as usize * row_bytes + (index % width as usize) / 8] |=
                1 << (7 - (index % width as usize) % 8);
        }
    }
    packed
}

fn compress_flate(data: &[u8]) -> Option<Vec<u8>> {
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(data).ok()?;
    encoder.finish().ok()
}

fn gray_jpeg(gray: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut encoder =
        JpegEncoder::new_with_quality(Cursor::new(&mut output), GRAYSCALE_JPEG_QUALITY);
    encoder
        .encode(gray, width, height, ExtendedColorType::L8)
        .ok()?;
    Some(output)
}

fn downsample_gray(gray: Vec<u8>, width: u32, height: u32) -> Option<(Vec<u8>, u32, u32)> {
    let long_edge = width.max(height);
    if long_edge <= EBOOK_MAX_LONG_EDGE {
        return Some((gray, width, height));
    }

    let scale = EBOOK_MAX_LONG_EDGE as f32 / long_edge as f32;
    let target_width = ((width as f32 * scale).round() as u32).max(1);
    let target_height = ((height as f32 * scale).round() as u32).max(1);
    let source = image::GrayImage::from_raw(width, height, gray)?;
    let resized = image::imageops::resize(
        &source,
        target_width,
        target_height,
        image::imageops::FilterType::Triangle,
    );
    Some((resized.into_raw(), target_width, target_height))
}

fn transformed_stream(
    original: &Stream,
    image: &DecodedImage,
    kind: ImageKind,
    gray: Vec<u8>,
) -> Option<Stream> {
    // Do not discard transparency or soft masks while reducing colors.
    if original.dict.get(b"SMask").is_ok() || original.dict.get(b"Mask").is_ok() {
        return None;
    }

    let (content, filter, bits, output_width, output_height) = match kind {
        ImageKind::Color => return None,
        ImageKind::Monochrome => (
            compress_flate(&pack_monochrome(&gray, image.width))?,
            "FlateDecode",
            1,
            image.width,
            image.height,
        ),
        ImageKind::Grayscale => {
            let (gray, width, height) = downsample_gray(gray, image.width, image.height)?;
            if image_filter_names(original)
                .iter()
                .any(|f| f == "DCTDecode")
            {
                (
                    gray_jpeg(&gray, width, height)?,
                    "DCTDecode",
                    8,
                    width,
                    height,
                )
            } else {
                (compress_flate(&gray)?, "FlateDecode", 8, width, height)
            }
        }
    };

    let mut dict = original.dict.clone();
    dict.set("Width", output_width as i64);
    dict.set("Height", output_height as i64);
    dict.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
    dict.set("BitsPerComponent", bits as i64);
    dict.set("Filter", Object::Name(filter.as_bytes().to_vec()));
    dict.remove(b"Decode");
    dict.remove(b"DecodeParms");
    Some(Stream::new(dict, content))
}

fn collect_images_from_resources(
    doc: &Document,
    resources: &Dictionary,
    visited_xobjects: &mut BTreeSet<ObjectId>,
    image_ids: &mut BTreeSet<ObjectId>,
) {
    let xobjects = match resources.get(b"XObject") {
        Ok(Object::Dictionary(dict)) => Some(dict),
        Ok(Object::Reference(id)) => doc.get_dictionary(*id).ok(),
        _ => None,
    };
    let Some(xobjects) = xobjects else {
        return;
    };

    for value in xobjects.iter().map(|(_, value)| value) {
        let Ok(id) = value.as_reference() else {
            continue;
        };
        if !visited_xobjects.insert(id) {
            continue;
        }
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
            continue;
        };
        match stream.dict.get(b"Subtype").and_then(Object::as_name).ok() {
            Some(b"Image") => {
                image_ids.insert(id);
            }
            Some(b"Form") => {
                let form_resources = match stream.dict.get(b"Resources") {
                    Ok(Object::Dictionary(dict)) => Some(dict),
                    Ok(Object::Reference(resources_id)) => doc.get_dictionary(*resources_id).ok(),
                    _ => None,
                };
                if let Some(form_resources) = form_resources {
                    collect_images_from_resources(doc, form_resources, visited_xobjects, image_ids);
                }
            }
            _ => {}
        }
    }
}

fn referenced_image_ids(doc: &Document, page_ids: &[ObjectId]) -> Vec<ObjectId> {
    let mut image_ids = BTreeSet::new();
    let mut visited_xobjects = BTreeSet::new();
    for &page_id in page_ids {
        if let Ok((direct_resources, resource_ids)) = doc.get_page_resources(page_id) {
            if let Some(resources) = direct_resources {
                collect_images_from_resources(
                    doc,
                    resources,
                    &mut visited_xobjects,
                    &mut image_ids,
                );
            }
            for resources_id in resource_ids {
                if let Ok(resources) = doc.get_dictionary(resources_id) {
                    collect_images_from_resources(
                        doc,
                        resources,
                        &mut visited_xobjects,
                        &mut image_ids,
                    );
                }
            }
        }
    }
    image_ids.into_iter().collect()
}

fn process_image(doc: &Document, image_id: ObjectId) -> ImageWorkResult {
    let Ok(stream) = doc.get_object(image_id).and_then(Object::as_stream) else {
        return ImageWorkResult {
            image_id,
            kind: None,
            transform: None,
            skipped: true,
        };
    };
    let Some(image) = decode_image(doc, stream) else {
        return ImageWorkResult {
            image_id,
            kind: None,
            transform: None,
            skipped: true,
        };
    };

    let (kind, gray) = classify_image(&image);
    let transform = transformed_stream(stream, &image, kind, gray).and_then(|transformed| {
        ((transformed.content.len() as f32)
            < (stream.content.len() as f32 * (1.0 - MIN_SAVING_RATIO)))
            .then_some(ImageTransform {
                stream: transformed,
            })
    });
    ImageWorkResult {
        image_id,
        kind: Some(kind),
        transform,
        skipped: false,
    }
}

fn optimize_images(
    doc: &mut Document,
    page_ids: &[ObjectId],
    on_progress: &mut dyn FnMut(usize, usize),
) -> ImageOptimizationSummary {
    let image_ids = referenced_image_ids(doc, page_ids);
    let mut summary = ImageOptimizationSummary::default();
    let image_count = image_ids.len();
    if image_count == 0 {
        return summary;
    }

    // JPEG decode/resize/encode is CPU-heavy. Work in bounded parallel batches so large books
    // use all available cores without retaining every transformed image in memory at once.
    let worker_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, 6)
        .min(image_count);
    let batch_size = (worker_count * 8).max(16);
    let mut completed = 0usize;

    for batch in image_ids.chunks(batch_size) {
        let results = std::thread::scope(|scope| {
            use std::sync::atomic::{AtomicUsize, Ordering};
            use std::sync::{mpsc, Arc};

            let next_index = Arc::new(AtomicUsize::new(0));
            let (sender, receiver) = mpsc::channel();
            let immutable_doc: &Document = doc;
            for _ in 0..worker_count.min(batch.len()) {
                let next_index = Arc::clone(&next_index);
                let sender = sender.clone();
                scope.spawn(move || loop {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(&image_id) = batch.get(index) else {
                        break;
                    };
                    if sender.send(process_image(immutable_doc, image_id)).is_err() {
                        break;
                    }
                });
            }
            drop(sender);

            let mut results = Vec::with_capacity(batch.len());
            for result in receiver {
                completed += 1;
                on_progress(completed, image_count);
                results.push(result);
            }
            results
        });

        for result in results {
            if result.skipped {
                summary.skipped_images += 1;
            }
            match result.kind {
                Some(ImageKind::Color) => summary.color_images += 1,
                Some(ImageKind::Grayscale) => summary.grayscale_images += 1,
                Some(ImageKind::Monochrome) => summary.monochrome_images += 1,
                None => {}
            }
            if let Some(transform) = result.transform {
                if let Some(object) = doc.objects.get_mut(&result.image_id) {
                    *object = Object::Stream(transform.stream);
                    summary.optimized_images += 1;
                }
            }
        }
    }
    summary
}

fn generate_output_path(input_path: &Path) -> PathBuf {
    let stem = input_path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = input_path.extension().unwrap_or_default().to_string_lossy();
    let parent = input_path.parent().unwrap_or(Path::new("."));
    let extension = if ext.is_empty() {
        String::new()
    } else {
        format!(".{}", ext)
    };

    for index in 0.. {
        let suffix = if index == 0 {
            "_normalized".to_string()
        } else {
            format!("_normalized_{}", index)
        };
        let candidate = parent.join(format!("{}{}{}", stem, suffix, extension));
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!()
}

/// 在 PDF 数据中查找 xref 表的实际字节偏移量
/// 修复非标准 startxref 格式：
/// 有些 PDF 生成器将偏移值写在 startxref 同一行（如 "startxref 12345\r%%EOF"），
/// 而 lopdf 要求其各占一行（"startxref\n12345\n%%EOF"）。
fn repair_pdf_startxref(data: &[u8]) -> Option<Vec<u8>> {
    // 只在末尾 2KB 内查找，避免全文扫描
    let window_size = 2048.min(data.len());
    let window_start = data.len() - window_size;
    let window = &data[window_start..];

    // 找最后一个 "startxref"
    let sxref_rel = window
        .windows(9)
        .enumerate()
        .rev()
        .find(|(_, w)| *w == b"startxref")
        .map(|(i, _)| i)?;
    let sxref_abs = window_start + sxref_rel;

    let after = sxref_abs + 9;
    if after >= data.len() {
        return None;
    }

    // 判断 "startxref" 后紧跟的是空格还是换行符
    // 若已经是换行符，则格式正常，无需修复
    let next_byte = data[after];
    if next_byte == b'\r' || next_byte == b'\n' {
        return None;
    }
    // 若不是空格/制表符，无法识别，放弃
    if next_byte != b' ' && next_byte != b'\t' {
        return None;
    }

    // 跳过空白，定位数字开始
    let mut num_start = after;
    while num_start < data.len() && matches!(data[num_start], b' ' | b'\t') {
        num_start += 1;
    }

    // 读取数字
    let mut num_end = num_start;
    while num_end < data.len() && data[num_end].is_ascii_digit() {
        num_end += 1;
    }
    if num_start == num_end {
        return None;
    }

    // 验证该数字是一个有效偏移（指向文件内部）
    let offset_str = std::str::from_utf8(&data[num_start..num_end]).ok()?;
    let offset: usize = offset_str.parse().ok()?;
    if offset == 0 || offset >= data.len() {
        return None;
    }

    // 跳过数字后的行尾（可能有尾随空格，再跟 \r 或 \n 或 \r\n）
    let mut rest_start = num_end;
    while rest_start < data.len() && matches!(data[rest_start], b' ' | b'\t') {
        rest_start += 1;
    }
    if rest_start < data.len() && data[rest_start] == b'\r' {
        rest_start += 1;
        if rest_start < data.len() && data[rest_start] == b'\n' {
            rest_start += 1;
        }
    } else if rest_start < data.len() && data[rest_start] == b'\n' {
        rest_start += 1;
    }

    // 重建：将 "startxref <spaces><number><eol>" 替换为标准的三行格式
    let mut result = Vec::with_capacity(data.len() + 8);
    result.extend_from_slice(&data[..sxref_abs]);
    result.extend_from_slice(b"startxref\r\n");
    result.extend_from_slice(&data[num_start..num_end]);
    result.extend_from_slice(b"\r\n");
    result.extend_from_slice(&data[rest_start..]);
    Some(result)
}

/// 修复 PDF 后写入临时文件并重新加载
fn try_repair_pdf_and_load(path: &Path) -> Result<Document, String> {
    let data = std::fs::read(path).map_err(|e| format!("读取文件失败: {}", e))?;

    let repaired = repair_pdf_startxref(&data)
        .ok_or_else(|| "无法自动修复 PDF startxref 格式（未找到可修复的格式）".to_string())?;

    let tmp_path = std::env::temp_dir().join(format!(
        "foliomend_repair_{}.pdf",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    std::fs::write(&tmp_path, &repaired).map_err(|e| format!("写入临时文件失败: {}", e))?;

    let result = Document::load(&tmp_path).map_err(|e| format!("修复后仍无法加载: {}", e));
    let _ = std::fs::remove_file(&tmp_path);
    result
}

/// Inspect only outline links, never follow arbitrary metadata or page references.
#[derive(Debug, Default)]
pub struct BookmarkInspection {
    pub abnormal: bool,
    pub may_have_bookmarks: bool,
}

pub fn inspect_bookmarks(doc: &Document) -> BookmarkInspection {
    let mut result = BookmarkInspection::default();
    let Ok(catalog) = doc.catalog() else {
        return result;
    };
    let Ok(outlines) = catalog.get(b"Outlines") else {
        return result;
    };
    let Ok(root_id) = outlines.as_reference() else {
        return BookmarkInspection {
            abnormal: true,
            may_have_bookmarks: true,
        };
    };
    let Ok(root) = doc.get_dictionary(root_id) else {
        return BookmarkInspection {
            abnormal: true,
            may_have_bookmarks: true,
        };
    };
    result.abnormal = root.get(b"Type").is_ok()
        && root.get(b"Type").and_then(Object::as_name).ok() != Some(b"Outlines");
    if root.get(b"Title").is_ok() {
        result.abnormal = true;
        result.may_have_bookmarks = true;
    }
    let mut pending = vec![(root_id, true)];
    let mut seen = BTreeSet::new();
    while let Some((id, is_root)) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Ok(node) = doc.get_dictionary(id) else {
            result.abnormal = true;
            // Recognized streams (e.g. XML metadata) cannot be bookmarks.
            result.may_have_bookmarks |= !matches!(doc.get_object(id), Ok(Object::Stream(_)));
            continue;
        };
        if !is_root {
            if node.get(b"Title").and_then(Object::as_str).is_ok() {
                result.may_have_bookmarks = true;
            } else {
                result.abnormal = true;
                result.may_have_bookmarks = true;
            }
        }
        let first = node.get(b"First").ok();
        let last = node.get(b"Last").ok();
        if first.is_some() != last.is_some() {
            result.abnormal = true;
        }
        if first.is_none() && node.get(b"Count").and_then(Object::as_i64).unwrap_or(0) != 0 {
            result.abnormal = true;
            result.may_have_bookmarks = true;
        }
        if node.get(b"Prev").is_ok() && node.get(b"Prev").and_then(Object::as_reference).is_err() {
            result.abnormal = true;
        }
        // Visit both endpoints to avoid silently deleting an orphaned last bookmark.
        for link in [first, last].into_iter().flatten() {
            if let Ok(child) = link.as_reference() {
                if child == root_id || child == id {
                    result.abnormal = true;
                } else {
                    pending.push((child, false));
                }
            } else {
                result.abnormal = true;
                result.may_have_bookmarks = true;
            }
        }
        if let Some(first) = first {
            let mut current = first.as_reference().ok();
            let mut previous = None;
            let mut siblings = BTreeSet::new();
            while let Some(child) = current {
                if child == root_id || !siblings.insert(child) {
                    result.abnormal = true;
                    break;
                }
                pending.push((child, false));
                let Ok(item) = doc.get_dictionary(child) else {
                    result.abnormal = true;
                    break;
                };
                if item.get(b"Parent").and_then(Object::as_reference).ok() != Some(id)
                    || item.get(b"Prev").and_then(Object::as_reference).ok() != previous
                {
                    result.abnormal = true;
                }
                previous = Some(child);
                current = match item.get(b"Next") {
                    Ok(next) => match next.as_reference() {
                        Ok(next) => Some(next),
                        Err(_) => {
                            result.abnormal = true;
                            result.may_have_bookmarks = true;
                            None
                        }
                    },
                    Err(_) => None,
                };
            }
            if previous != last.and_then(|v| v.as_reference().ok()) {
                result.abnormal = true;
            }
        }
    }
    result
}

// Resolve named destinations without changing shared destinations used by links.
fn bookmark_destination(doc: &Document, value: &Object) -> Option<Vec<Object>> {
    let (_, value) = doc.dereference(value).ok()?;
    match value {
        Object::Array(array) => Some(array.clone()),
        Object::Dictionary(dict) => {
            let (_, value) = doc.dereference(dict.get(b"D").ok()?).ok()?;
            value.as_array().ok().cloned()
        }
        Object::Name(name) | Object::String(name, _) => {
            let catalog = doc.catalog().ok()?;
            if let Ok(old) = catalog.get(b"Dests") {
                if let Ok((_, Object::Dictionary(dict))) = doc.dereference(old) {
                    if let Ok(value) = dict.get(name) {
                        let (_, value) = doc.dereference(value).ok()?;
                        return match value {
                            Object::Array(a) => Some(a.clone()),
                            Object::Dictionary(d) => doc
                                .dereference(d.get(b"D").ok()?)
                                .ok()?
                                .1
                                .as_array()
                                .ok()
                                .cloned(),
                            _ => None,
                        };
                    }
                }
            }
            let (_, names) = doc.dereference(catalog.get(b"Names").ok()?).ok()?;
            let tree = names.as_dict().ok()?.get(b"Dests").ok()?;
            let mut pending = vec![tree.clone()];
            let mut seen = BTreeSet::new();
            while let Some(node) = pending.pop() {
                if let Object::Reference(id) = &node {
                    if !seen.insert(*id) {
                        continue;
                    }
                }
                let (_, node) = doc.dereference(&node).ok()?;
                let node = node.as_dict().ok()?;
                if let Ok(entries) = node.get(b"Names").and_then(Object::as_array) {
                    for pair in entries.chunks_exact(2) {
                        if pair[0].as_str().ok() == Some(name.as_slice()) {
                            let (_, target) = doc.dereference(&pair[1]).ok()?;
                            return match target {
                                Object::Array(a) => Some(a.clone()),
                                Object::Dictionary(d) => doc
                                    .dereference(d.get(b"D").ok()?)
                                    .ok()?
                                    .1
                                    .as_array()
                                    .ok()
                                    .cloned(),
                                _ => None,
                            };
                        }
                    }
                }
                if let Ok(kids) = node.get(b"Kids").and_then(Object::as_array) {
                    pending.extend(kids.iter().cloned());
                }
            }
            None
        }
        _ => None,
    }
}

fn fit_bookmarks_to_width(doc: &mut Document) -> (usize, usize) {
    let Some(root) = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Outlines").ok())
        .and_then(|o| o.as_reference().ok())
    else {
        return (0, 0);
    };
    let pages = doc.get_pages().into_values().collect::<BTreeSet<_>>();
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    let (mut updated, mut skipped) = (0, 0);
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Ok(node) = doc.get_dictionary(id).cloned() else {
            continue;
        };
        for key in [b"First".as_slice(), b"Next", b"Last"] {
            if let Ok(next) = node.get(key).and_then(Object::as_reference) {
                pending.push(next);
            }
        }
        if id == root || node.get(b"Title").is_err() {
            continue;
        }
        let action = node
            .get(b"A")
            .ok()
            .and_then(|a| doc.dereference(a).ok())
            .and_then(|(_, a)| a.as_dict().ok())
            .cloned();
        let destination = if let Ok(dest) = node.get(b"Dest") {
            Some((dest, false))
        } else if let Some(a) = action.as_ref() {
            if a.get(b"S").and_then(Object::as_name).ok() == Some(b"GoTo") {
                a.get(b"D").ok().map(|d| (d, true))
            } else {
                None
            }
        } else {
            None
        };
        let Some((dest, via_action)) = destination else {
            skipped += 1;
            continue;
        };
        let Some(array) = bookmark_destination(doc, dest) else {
            skipped += 1;
            continue;
        };
        let Some(page) = array
            .first()
            .and_then(|p| p.as_reference().ok())
            .filter(|p| pages.contains(p))
        else {
            skipped += 1;
            continue;
        };
        let kind = array.get(1).and_then(|v| v.as_name().ok());
        let top = match kind {
            Some(b"XYZ") => array.get(3).cloned(),
            Some(b"FitH" | b"FitBH") => array.get(2).cloned(),
            Some(b"FitR") => array.get(5).cloned(),
            Some(b"Fit" | b"FitB" | b"FitV" | b"FitBV") => Some(Object::Null),
            _ => None,
        };
        let Some(top) =
            top.filter(|v| matches!(v, Object::Null | Object::Integer(_) | Object::Real(_)))
        else {
            skipped += 1;
            continue;
        };
        if kind == Some(b"FitH")
            && array.len() == 3
            && !matches!(dest, Object::Name(_) | Object::String(_, _))
        {
            continue;
        }
        let fitted = Object::Array(vec![
            Object::Reference(page),
            Object::Name(b"FitH".to_vec()),
            top,
        ]);
        if via_action {
            // Copy the action onto the bookmark, so a shared action/link remains unchanged.
            let mut a = action.unwrap();
            a.set("D", fitted);
            doc.get_dictionary_mut(id).unwrap().set("A", a);
        } else {
            doc.get_dictionary_mut(id).unwrap().set("Dest", fitted);
        }
        updated += 1;
    }
    (updated, skipped)
}

/// 处理单个 PDF 文件
#[allow(dead_code)]
pub fn process_pdf(input_path: &str) -> ProcessResult {
    let mut ignore_progress = |_progress: ProcessingProgress| {};
    process_pdf_with_options_and_progress(
        input_path,
        ProcessOptions::default(),
        &mut ignore_progress,
    )
}

/// 处理单个 PDF 文件，并在耗时阶段报告细粒度进度。
#[allow(dead_code)]
pub fn process_pdf_with_progress(
    input_path: &str,
    on_progress: &mut dyn FnMut(ProcessingProgress),
) -> ProcessResult {
    process_pdf_with_options_and_progress(input_path, ProcessOptions::default(), on_progress)
}

/// 按界面选项处理单个 PDF 文件，并在耗时阶段报告细粒度进度。
pub fn process_pdf_with_options_and_progress(
    input_path: &str,
    options: ProcessOptions,
    on_progress: &mut dyn FnMut(ProcessingProgress),
) -> ProcessResult {
    process_pdf_with_bookmark_confirmation(input_path, options, on_progress, &mut || false)
}

pub fn process_pdf_with_bookmark_confirmation(
    input_path: &str,
    options: ProcessOptions,
    on_progress: &mut dyn FnMut(ProcessingProgress),
    confirm_repair: &mut dyn FnMut() -> bool,
) -> ProcessResult {
    let path = Path::new(input_path);

    let mut report = |phase: &str,
                      page: usize,
                      page_count: usize,
                      image: usize,
                      image_count: usize,
                      progress: f32| {
        on_progress(ProcessingProgress {
            input_path: input_path.to_string(),
            phase: phase.to_string(),
            page,
            page_count,
            image,
            image_count,
            progress: progress.clamp(0.0, 1.0),
        });
    };

    report("读取 PDF", 0, 0, 0, 0, 0.01);

    let mut result = ProcessResult {
        input_path: input_path.to_string(),
        output_path: String::new(),
        page_count: 0,
        original_widths: Vec::new(),
        target_width: 0.0,
        color_images: 0,
        grayscale_images: 0,
        monochrome_images: 0,
        optimized_images: 0,
        skipped_images: 0,
        original_bytes: std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
        output_bytes: 0,
        bookmark_status: "未启用".to_string(),
        bookmarks_fit_width: 0,
        bookmarks_fit_width_skipped: 0,
        success: false,
        error: None,
    };

    // 完整加载并重写文档，避免增量保存把旧图像字节继续保留在输出中。
    let mut doc = match Document::load(path) {
        Ok(doc) => doc,
        Err(e) => {
            let err_str = e.to_string();
            // 若为 xref 无效错误，尝试自动修复后重新加载
            if err_str.contains("invalid start value") || err_str.contains("cross-reference") {
                match try_repair_pdf_and_load(path) {
                    Ok(doc) => doc,
                    Err(_) => {
                        result.error = Some(format!("无法加载 PDF 文件: {}", e));
                        return result;
                    }
                }
            } else {
                result.error = Some(format!("无法加载 PDF 文件: {}", e));
                return result;
            }
        }
    };

    let mut bookmarks_repaired = false;
    if options.repair_bookmarks {
        report("检查书签结构", 0, 0, 0, 0, 0.02);
        let inspection = inspect_bookmarks(&doc);
        result.bookmark_status = if !inspection.abnormal {
            "结构正常".to_string()
        } else if !inspection.may_have_bookmarks || confirm_repair() {
            match doc.catalog_mut() {
                Ok(catalog) => {
                    catalog.remove(b"Outlines");
                    bookmarks_repaired = true;
                    "已清理异常书签结构".to_string()
                }
                Err(_) => "发现异常，无法清理书签入口".to_string(),
            }
        } else {
            "发现异常，已按选择保留书签".to_string()
        };
    }

    // Phase 1: 从原始文档中读取页面信息
    let page_ids = doc.get_pages().into_values().collect::<Vec<ObjectId>>();

    result.page_count = page_ids.len();
    report("分析页面", 0, result.page_count, 0, 0, 0.03);

    if page_ids.is_empty() {
        result.error = Some("PDF 文件没有页面".to_string());
        return result;
    }

    let mut widths = Vec::with_capacity(page_ids.len());
    let mut has_true_crop_work = false;
    for (index, &page_id) in page_ids.iter().enumerate() {
        if let Some(width) = get_visible_page_width(&doc, page_id) {
            widths.push(width);
        }
        if options.crop_mode == CropBoxMode::TrueCrop {
            if let Some((media_box, crop_box, _)) = get_page_boxes(&doc, page_id) {
                has_true_crop_work |= !media_box.approximately_equals(crop_box);
            }
        }
        report(
            "分析页面",
            index + 1,
            result.page_count,
            0,
            0,
            0.03 + 0.17 * (index + 1) as f32 / result.page_count.max(1) as f32,
        );
    }

    if widths.is_empty() {
        result.error = Some("无法获取任何页面的宽度".to_string());
        return result;
    }

    result.original_widths = widths.clone();

    // 归一化以页面实际可见宽度（CropBox，没有时回退到 MediaBox）为准。
    let target_width = if options.normalize_pages {
        calculate_target_width(&widths)
    } else {
        0.0
    };
    result.target_width = target_width;

    if options.compress_images {
        report("分析图像", 0, result.page_count, 0, 0, 0.20);
        let mut image_progress = |image: usize, image_count: usize| {
            report(
                "优化图像",
                0,
                result.page_count,
                image,
                image_count,
                0.20 + 0.60 * image as f32 / image_count.max(1) as f32,
            );
        };
        let image_summary = optimize_images(&mut doc, &page_ids, &mut image_progress);
        result.color_images = image_summary.color_images;
        result.grayscale_images = image_summary.grayscale_images;
        result.monochrome_images = image_summary.monochrome_images;
        result.optimized_images = image_summary.optimized_images;
        result.skipped_images = image_summary.skipped_images;
        report("图像处理完成", 0, result.page_count, 0, 0, 0.80);
    } else {
        report("已跳过图片压缩", 0, result.page_count, 0, 0, 0.80);
    }

    if options.fit_bookmarks_to_width {
        report("设置书签适合宽度", 0, result.page_count, 0, 0, 0.80);
        (
            result.bookmarks_fit_width,
            result.bookmarks_fit_width_skipped,
        ) = fit_bookmarks_to_width(&mut doc);
    }

    // 检查是否有页面需要修改
    let needs_page_normalization = options.normalize_pages
        && widths
            .iter()
            .any(|&width| (width - target_width).abs() >= 0.01);
    let needs_page_geometry_change = needs_page_normalization || has_true_crop_work;
    let needs_modification = needs_page_geometry_change
        || result.optimized_images > 0
        || bookmarks_repaired
        || result.bookmarks_fit_width > 0;

    if !needs_modification {
        result.output_path = "无需处理（所选操作不会改变此文件）".to_string();
        result.success = true;
        report("处理完成", result.page_count, result.page_count, 0, 0, 1.0);
        return result;
    }

    // Phase 2: 以可见框为基准归一化，或把 MediaBox 真正裁到 CropBox。
    let mut destination_transforms = std::collections::BTreeMap::new();
    if options.normalize_pages || options.crop_mode == CropBoxMode::TrueCrop {
        for (index, &page_id) in page_ids.iter().enumerate() {
            let target = options.normalize_pages.then_some(target_width);
            match transform_page_geometry(&mut doc, page_id, target, options.crop_mode) {
                Ok(Some(transform)) => {
                    destination_transforms.insert(page_id, transform);
                }
                Ok(None) => {}
                Err(e) => {
                    result.error = Some(format!("处理页面时出错: {e}"));
                    return result;
                }
            }
            report(
                if options.crop_mode == CropBoxMode::TrueCrop {
                    "裁剪并归一化页面"
                } else {
                    "按可见宽度归一化页面"
                },
                index + 1,
                result.page_count,
                0,
                0,
                0.80 + 0.18 * (index + 1) as f32 / result.page_count.max(1) as f32,
            );
        }
    }
    if !destination_transforms.is_empty() {
        for object in doc.objects.values_mut() {
            update_destination_coordinates(object, &destination_transforms);
        }
    }

    // 移除不可达对象并完整重写，确保旧图像流不会残留在输出文件中。
    doc.prune_objects();
    // A full rewrite must not retain offsets into an earlier incremental revision.
    doc.trailer.remove(b"Prev");
    doc.trailer.remove(b"XRefStm");
    let output_path = generate_output_path(path);
    result.output_path = output_path.to_string_lossy().to_string();
    report("保存 PDF", result.page_count, result.page_count, 0, 0, 0.99);

    match doc.save(&output_path) {
        Ok(_) => {
            result.output_bytes = std::fs::metadata(&output_path)
                .map(|meta| meta.len())
                .unwrap_or(0);

            // A compression-only run must never leave the user with a larger file.
            if !needs_page_geometry_change
                && !bookmarks_repaired
                && result.bookmarks_fit_width == 0
                && result.original_bytes > 0
                && result.output_bytes >= result.original_bytes
                && std::fs::remove_file(&output_path).is_ok()
            {
                result.output_path = "无需输出（压缩后体积没有减小）".to_string();
                result.output_bytes = 0;
                result.optimized_images = 0;
            }
            result.success = true;
            report("处理完成", result.page_count, result.page_count, 0, 0, 1.0);
        }
        Err(e) => {
            result.error = Some(format!("保存文件失败: {}", e));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn cropped_page_document() -> (Document, ObjectId) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let content_id =
            doc.add_object(Stream::new(Dictionary::new(), b"0 0 450 600 re f".to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), 600.into(), 850.into()],
            "CropBox" => vec![0.into(), 200.into(), 450.into(), 800.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        (doc, page_id)
    }

    fn outline_fixture(with_title: bool) -> (Document, ObjectId, ObjectId) {
        let (mut doc, _) = cropped_page_document();
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => (1, 0) });
        doc.trailer.set("Root", catalog);
        let root = doc.add_object(dictionary! { "Type" => "Outlines", "Count" => 0 });
        let child = if with_title {
            doc.add_object(
                dictionary! { "Title" => Object::string_literal("Chapter"), "Parent" => root },
            )
        } else {
            doc.add_object(Stream::new(
                dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
                vec![],
            ))
        };
        doc.get_dictionary_mut(root).unwrap().set("First", child);
        doc.get_dictionary_mut(root)
            .unwrap()
            .set("Last", if with_title { child } else { root });
        doc.catalog_mut().unwrap().set("Outlines", root);
        (doc, root, child)
    }

    #[test]
    fn fit_width_handles_named_destinations_shared_actions_and_nested_bookmarks() {
        let (mut doc, root, first) = outline_fixture(true);
        let page = *doc.get_pages().values().next().unwrap();
        let xyz = Object::Array(vec![
            page.into(),
            Object::Name(b"XYZ".to_vec()),
            20.into(),
            650.into(),
            Object::Real(1.5),
        ]);
        let shared_action = doc.add_object(dictionary! { "S" => "GoTo", "D" => xyz.clone() });
        doc.get_dictionary_mut(first)
            .unwrap()
            .set("A", shared_action);
        let child = doc.add_object(dictionary! { "Title" => Object::string_literal("Child"), "Parent" => first, "Dest" => Object::Name(b"chapter".to_vec()) });
        doc.get_dictionary_mut(first).unwrap().set("First", child);
        doc.get_dictionary_mut(first).unwrap().set("Last", child);
        let names = doc.add_object(
            dictionary! { "Names" => vec![Object::string_literal("chapter"), xyz.clone()] },
        );
        doc.catalog_mut()
            .unwrap()
            .set("Names", dictionary! { "Dests" => names });
        let external = doc.add_object(dictionary! { "Title" => Object::string_literal("Web"), "Parent" => root, "Prev" => first, "A" => dictionary! { "S" => "URI", "URI" => Object::string_literal("https://example.com") } });
        doc.get_dictionary_mut(first).unwrap().set("Next", external);
        doc.get_dictionary_mut(root).unwrap().set("Last", external);
        assert_eq!(fit_bookmarks_to_width(&mut doc), (2, 1));
        let a = doc
            .get_dictionary(first)
            .unwrap()
            .get(b"A")
            .unwrap()
            .as_dict()
            .unwrap();
        let dest = a.get(b"D").unwrap().as_array().unwrap();
        assert_eq!(dest[1].as_name().unwrap(), b"FitH");
        assert_eq!(dest[2].as_i64().unwrap(), 650);
        assert_eq!(
            doc.get_dictionary(shared_action)
                .unwrap()
                .get(b"D")
                .unwrap()
                .as_array()
                .unwrap()[1]
                .as_name()
                .unwrap(),
            b"XYZ"
        );
        assert_eq!(
            doc.get_dictionary(child)
                .unwrap()
                .get(b"Dest")
                .unwrap()
                .as_array()
                .unwrap()[1]
                .as_name()
                .unwrap(),
            b"FitH"
        );
        assert_eq!(
            doc.get_dictionary(first)
                .unwrap()
                .get(b"First")
                .unwrap()
                .as_reference()
                .unwrap(),
            child
        );
        assert_eq!(fit_bookmarks_to_width(&mut doc), (0, 1));
    }

    #[test]
    fn fit_width_only_run_saves_output_and_switch_off_preserves_file() {
        let dir = std::env::temp_dir().join(format!(
            "foliomend_fit_width_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("book.pdf");
        let (mut doc, _, first) = outline_fixture(true);
        let page = *doc.get_pages().values().next().unwrap();
        doc.get_dictionary_mut(first).unwrap().set(
            "Dest",
            vec![
                page.into(),
                Object::Name(b"XYZ".to_vec()),
                Object::Null,
                500.into(),
                Object::Null,
            ],
        );
        doc.save(&input).unwrap();
        let original = std::fs::read(&input).unwrap();
        let options = ProcessOptions {
            compress_images: false,
            normalize_pages: false,
            repair_bookmarks: false,
            fit_bookmarks_to_width: false,
            ..ProcessOptions::default()
        };
        let result =
            process_pdf_with_options_and_progress(input.to_str().unwrap(), options, &mut |_| {});
        assert!(result.success);
        assert!(result.output_path.starts_with("无需处理"));
        let result = process_pdf_with_options_and_progress(
            input.to_str().unwrap(),
            ProcessOptions {
                fit_bookmarks_to_width: true,
                ..options
            },
            &mut |_| {},
        );
        assert!(result.success);
        assert_eq!(result.bookmarks_fit_width, 1);
        let output = Document::load(&result.output_path).unwrap();
        assert_eq!(output.get_pages().len(), 1);
        let first = output
            .catalog()
            .unwrap()
            .get(b"Outlines")
            .unwrap()
            .as_reference()
            .unwrap();
        let first = output
            .get_dictionary(first)
            .unwrap()
            .get(b"First")
            .unwrap()
            .as_reference()
            .unwrap();
        assert_eq!(
            output
                .get_dictionary(first)
                .unwrap()
                .get(b"Dest")
                .unwrap()
                .as_array()
                .unwrap()[1]
                .as_name()
                .unwrap(),
            b"FitH"
        );
        assert_eq!(std::fs::read(&input).unwrap(), original);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bookmark_detection_handles_empty_corrupt_valid_and_cycles() {
        let (doc, _, _) = outline_fixture(false);
        let inspection = inspect_bookmarks(&doc);
        assert!(inspection.abnormal);
        assert!(!inspection.may_have_bookmarks);
        let (mut doc, root, child) = outline_fixture(true);
        assert!(!inspect_bookmarks(&doc).abnormal);
        doc.get_dictionary_mut(child).unwrap().set("Next", child);
        assert!(inspect_bookmarks(&doc).abnormal);
        assert!(inspect_bookmarks(&doc).may_have_bookmarks);
        doc.get_dictionary_mut(child).unwrap().remove(b"Next");
        doc.get_dictionary_mut(root).unwrap().set("Last", (999, 0));
        assert!(inspect_bookmarks(&doc).may_have_bookmarks);
        doc.catalog_mut().unwrap().remove(b"Outlines");
        assert!(!inspect_bookmarks(&doc).abnormal);
    }

    #[test]
    fn bookmark_repair_respects_switch_confirmation_and_writes_repair_only_output() {
        let dir = std::env::temp_dir().join(format!(
            "foliomend_bookmarks_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("book.pdf");
        let options = ProcessOptions {
            compress_images: false,
            normalize_pages: false,
            ..ProcessOptions::default()
        };
        let (mut doc, _, _) = outline_fixture(false);
        doc.save(&path).unwrap();
        let result = process_pdf_with_bookmark_confirmation(
            path.to_str().unwrap(),
            options,
            &mut |_| {},
            &mut || panic!("empty corrupt tree must repair automatically"),
        );
        assert!(result.success);
        let repaired = Document::load(&result.output_path).unwrap();
        assert!(repaired.catalog().unwrap().get(b"Outlines").is_err());
        assert_eq!(repaired.get_pages().len(), 1);
        assert!(Document::load(&path)
            .unwrap()
            .catalog()
            .unwrap()
            .get(b"Outlines")
            .is_ok());
        let disabled = ProcessOptions {
            repair_bookmarks: false,
            ..options
        };
        let result = process_pdf_with_bookmark_confirmation(
            path.to_str().unwrap(),
            disabled,
            &mut |_| {},
            &mut || panic!("disabled"),
        );
        assert_eq!(result.bookmark_status, "未启用");
        assert!(result.output_path.starts_with("无需处理"));
        let (mut doc, _, child) = outline_fixture(true);
        doc.get_dictionary_mut(child).unwrap().set("Next", child);
        doc.save(&path).unwrap();
        let mut asked = false;
        let result = process_pdf_with_bookmark_confirmation(
            path.to_str().unwrap(),
            options,
            &mut |_| {},
            &mut || {
                asked = true;
                false
            },
        );
        assert!(asked);
        assert!(result.output_path.starts_with("无需处理"));
        let result = process_pdf_with_bookmark_confirmation(
            path.to_str().unwrap(),
            options,
            &mut |_| {},
            &mut || true,
        );
        assert!(result.success);
        assert!(Document::load(&result.output_path)
            .unwrap()
            .catalog()
            .unwrap()
            .get(b"Outlines")
            .is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_calculate_target_width_normal() {
        let widths = vec![595.0, 595.0, 595.0, 595.0, 595.0];
        assert!((calculate_target_width(&widths) - 595.0).abs() < 0.01);
    }

    #[test]
    fn test_calculate_target_width_with_outlier() {
        // 模拟一本扫描书籍：大部分页面595pt宽，有一页是1190pt（折叠页）
        let mut widths = vec![595.0; 99];
        widths.push(1190.0);
        let target = calculate_target_width(&widths);
        // 应该排除离群值，目标宽度应接近595
        assert!((target - 595.0).abs() < 1.0);
    }

    #[test]
    fn test_calculate_target_width_varied() {
        // 模拟宽度有变化的情况
        let widths = vec![590.0, 595.0, 598.0, 592.0, 600.0];
        let target = calculate_target_width(&widths);
        assert!((target - 600.0).abs() < 0.01);
    }

    #[test]
    fn test_align_visible_uses_cropbox_width() {
        let (mut doc, page_id) = cropped_page_document();
        let transform =
            transform_page_geometry(&mut doc, page_id, Some(600.0), CropBoxMode::AlignVisible)
                .unwrap()
                .expect("page should be scaled");
        let (media, crop, _) = get_page_boxes(&doc, page_id).unwrap();

        assert!((transform.scale - 4.0 / 3.0).abs() < 0.0001);
        assert!((crop.width() - 600.0).abs() < 0.01);
        assert!((media.width() - 800.0).abs() < 0.01);
        assert!(!media.approximately_equals(crop));
    }

    #[test]
    fn test_true_crop_matches_media_and_crop_without_scaling() {
        let (mut doc, page_id) = cropped_page_document();
        let transform =
            transform_page_geometry(&mut doc, page_id, None, CropBoxMode::TrueCrop).unwrap();
        let (media, crop, _) = get_page_boxes(&doc, page_id).unwrap();

        assert!(transform.is_none());
        assert!(media.approximately_equals(crop));
        assert!((media.width() - 450.0).abs() < 0.01);
        assert!((media.height() - 600.0).abs() < 0.01);
        assert!((media.y1 - 200.0).abs() < 0.01);
    }

    #[test]
    fn rotated_pages_normalize_displayed_width_in_both_crop_modes() {
        for rotation in [0, 90, 180, 270, -90, 450] {
            for mode in [CropBoxMode::AlignVisible, CropBoxMode::TrueCrop] {
                let (mut doc, page_id) = cropped_page_document();
                doc.get_dictionary_mut(page_id)
                    .unwrap()
                    .set("Rotate", rotation);
                let quarter_turn =
                    rotation == 90 || rotation == 270 || rotation == -90 || rotation == 450;
                let original_width = if quarter_turn { 600.0 } else { 450.0 };
                assert_eq!(get_visible_page_width(&doc, page_id), Some(original_width));
                let transform = transform_page_geometry(&mut doc, page_id, Some(900.0), mode)
                    .unwrap()
                    .unwrap();
                assert!((transform.scale - 900.0 / original_width).abs() < 0.0001);
                assert!((get_visible_page_width(&doc, page_id).unwrap() - 900.0).abs() < 0.01);
                assert_eq!(
                    doc.get_dictionary(page_id)
                        .unwrap()
                        .get(b"Rotate")
                        .unwrap()
                        .as_i64()
                        .unwrap(),
                    rotation
                );
                let (media, crop, _) = get_page_boxes(&doc, page_id).unwrap();
                assert_eq!(
                    media.approximately_equals(crop),
                    mode == CropBoxMode::TrueCrop
                );
            }
        }
    }

    #[test]
    fn displayed_width_respects_inherited_rotation_and_page_override() {
        let (mut doc, page_id) = cropped_page_document();
        let parent = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Parent")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_dictionary_mut(parent).unwrap().set("Rotate", 90);
        assert_eq!(get_visible_page_width(&doc, page_id), Some(600.0));
        transform_page_geometry(&mut doc, page_id, Some(900.0), CropBoxMode::AlignVisible).unwrap();
        assert_eq!(get_visible_page_width(&doc, page_id), Some(900.0));
        doc.get_dictionary_mut(page_id).unwrap().set("Rotate", 0);
        assert_eq!(get_visible_page_width(&doc, page_id), Some(675.0));
    }

    #[test]
    fn test_destination_coordinates_follow_page_scaling() {
        let page_id = (42, 0);
        let mut destination = vec![
            Object::Reference(page_id),
            Object::Name(b"FitH".to_vec()),
            Object::Integer(800),
        ];
        let transforms = std::collections::BTreeMap::from([(
            page_id,
            PageTransform {
                scale: 4.0 / 3.0,
                tx: 0.0,
                ty: -200.0 / 3.0,
            },
        )]);

        transform_destination_array(&mut destination, &transforms);
        assert!((get_number(&destination[2]).unwrap() - 1000.0).abs() < 0.01);
    }

    #[test]
    fn test_generate_output_path() {
        let path = Path::new("/some/path/book.pdf");
        let output = generate_output_path(path);
        assert!(output.to_string_lossy().contains("book_normalized.pdf"));
    }

    #[test]
    fn test_generate_output_path_avoids_overwrite() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("foliomend_test_{}", unique));
        std::fs::create_dir_all(&dir).unwrap();

        let input = dir.join("book.pdf");
        let first_output = dir.join("book_normalized.pdf");
        std::fs::write(&first_output, b"existing").unwrap();

        let output = generate_output_path(&input);
        assert_eq!(output, dir.join("book_normalized_1.pdf"));

        let _ = std::fs::remove_file(first_output);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn test_classify_image_distinguishes_color_gray_and_monochrome() {
        let monochrome = DecodedImage {
            width: 4,
            height: 1,
            channels: 1,
            pixels: vec![0, 255, 0, 255],
        };
        assert_eq!(classify_image(&monochrome).0, ImageKind::Monochrome);

        let grayscale = DecodedImage {
            width: 4,
            height: 1,
            channels: 1,
            pixels: vec![0, 80, 160, 255],
        };
        let (grayscale_kind, grayscale_plane) = classify_image(&grayscale);
        assert_eq!(grayscale_kind, ImageKind::Grayscale);
        assert_eq!(grayscale_plane.len(), 4);

        let color = DecodedImage {
            width: 2,
            height: 1,
            channels: 3,
            pixels: vec![255, 0, 0, 0, 0, 255],
        };
        assert_eq!(classify_image(&color).0, ImageKind::Color);
    }

    #[test]
    fn test_pack_monochrome_uses_pdf_msb_first_bits() {
        let packed = pack_monochrome(&[0, 255, 0, 255, 255, 255, 0, 0, 255], 9);
        assert_eq!(packed, vec![0b0101_1100, 0b1000_0000]);
    }

    #[test]
    fn test_full_rewrite_actually_reduces_pdf_size() {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("foliomend_compression_test_{}", unique));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("scan.pdf");

        let width = 128u32;
        let height = 128u32;
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for pixel in 0..width * height {
            let value = if pixel % 16 < 8 { 0 } else { 255 };
            rgb.extend_from_slice(&[value, value, value]);
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::none());
        encoder.write_all(&rgb).unwrap();
        let image_data = encoder.finish().unwrap();

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let image_id = doc.add_object(Stream::new(
            lopdf::dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => width as i64,
                "Height" => height as i64,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
                "Filter" => "FlateDecode",
            },
            image_data,
        ));
        let resources_id = doc.add_object(lopdf::dictionary! {
            "XObject" => lopdf::dictionary! { "Im0" => image_id },
        });
        let content_id = doc.add_object(Stream::new(
            Dictionary::new(),
            b"q 128 0 0 128 0 0 cm /Im0 Do Q".to_vec(),
        ));
        let page_id = doc.add_object(lopdf::dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Resources" => resources_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), 128.into(), 128.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(lopdf::dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(lopdf::dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.save(&input).unwrap();

        let result = process_pdf(input.to_string_lossy().as_ref());
        assert!(result.success, "{:?}", result.error);
        assert!(result.optimized_images > 0);
        assert!(result.output_bytes < result.original_bytes);

        let output = PathBuf::from(result.output_path);
        assert!(output.exists());
        let rewritten = Document::load(&output).expect("rewritten PDF should be loadable");
        assert_eq!(rewritten.get_pages().len(), 1);
        assert!(!rewritten.trailer.has(b"Prev"));
        assert!(!rewritten.trailer.has(b"XRefStm"));
        let _ = std::fs::remove_file(output);
        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_dir(dir);
    }
}
