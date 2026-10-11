struct Params {
    width: u32,
    height: u32,
    channels: u32,
    target_width: u32,
    target_height: u32,
    output_bytes: u32,
    padding0: u32,
    padding1: u32,
}
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> source: array<u32>;
@group(0) @binding(2) var<storage, read_write> temporary: array<f32>;
@group(0) @binding(3) var<storage, read_write> output: array<u32>;

fn index(id: vec3<u32>) -> u32 { return id.x + id.y * 65535u * 64u; }
fn byte_at(offset: u32) -> u32 { return (source[offset / 4u] >> ((offset % 4u) * 8u)) & 255u; }
fn gray_at(pixel: u32) -> u32 {
    if params.channels == 1u { return byte_at(pixel); }
    let offset = pixel * 3u;
    return (byte_at(offset) * 77u + byte_at(offset + 1u) * 150u + byte_at(offset + 2u) * 29u) / 256u;
}

@compute @workgroup_size(64)
fn grayscale(@builtin(global_invocation_id) id: vec3<u32>) {
    let word = index(id);
    if word * 4u >= params.output_bytes { return; }
    var value = 0u;
    for (var i = 0u; i < 4u; i++) {
        let pixel = word * 4u + i;
        if pixel < params.output_bytes { value |= gray_at(pixel) << (i * 8u); }
    }
    output[word] = value;
}

@compute @workgroup_size(64)
fn monochrome(@builtin(global_invocation_id) id: vec3<u32>) {
    let word = index(id);
    if word * 4u >= params.output_bytes { return; }
    let row_bytes = (params.width + 7u) / 8u;
    var value = 0u;
    for (var i = 0u; i < 4u; i++) {
        let offset = word * 4u + i;
        if offset >= params.output_bytes { break; }
        let y = offset / row_bytes;
        let x = (offset % row_bytes) * 8u;
        var packed = 0u;
        for (var bit = 0u; bit < 8u; bit++) {
            if x + bit < params.width && gray_at(y * params.width + x + bit) >= 160u {
                packed |= 1u << (7u - bit);
            }
        }
        value |= packed << (i * 8u);
    }
    output[word] = value;
}

// Separable triangle filter, with the same pixel centers and edge normalization
// as image::imageops::resize. Keep the vertical intermediate in floating point.
fn triangle(i: u32, center: f32, ratio: f32) -> f32 {
    return max(0.0, 1.0 - abs((f32(i) - center) / ratio));
}
@compute @workgroup_size(64)
fn vertical(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = index(id);
    if pixel >= params.width * params.target_height { return; }
    let x = pixel % params.width;
    let y = pixel / params.width;
    let ratio = f32(params.height) / f32(params.target_height);
    let support = max(ratio, 1.0);
    let center = (f32(y) + 0.5) * ratio;
    let left = u32(clamp(floor(center - support), 0.0, f32(params.height - 1u)));
    let right = u32(clamp(ceil(center + support), f32(left + 1u), f32(params.height)));
    var sum = 0.0;
    for (var i = left; i < right; i++) { sum += triangle(i, center - 0.5, support); }
    var value = 0.0;
    for (var i = left; i < right; i++) {
        value += f32(gray_at(i * params.width + x)) * (triangle(i, center - 0.5, support) / sum);
    }
    temporary[pixel] = value;
}
@compute @workgroup_size(64)
fn horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    let word = index(id);
    if word * 4u >= params.output_bytes { return; }
    let ratio = f32(params.width) / f32(params.target_width);
    let support = max(ratio, 1.0);
    var packed = 0u;
    for (var byte = 0u; byte < 4u; byte++) {
        let pixel = word * 4u + byte;
        if pixel >= params.output_bytes { break; }
        let x = pixel % params.target_width;
        let y = pixel / params.target_width;
        let center = (f32(x) + 0.5) * ratio;
        let left = u32(clamp(floor(center - support), 0.0, f32(params.width - 1u)));
        let right = u32(clamp(ceil(center + support), f32(left + 1u), f32(params.width)));
        var sum = 0.0;
        for (var i = left; i < right; i++) { sum += triangle(i, center - 0.5, support); }
        var value = 0.0;
        for (var i = left; i < right; i++) {
            value += temporary[y * params.width + i] * (triangle(i, center - 0.5, support) / sum);
        }
        packed |= u32(floor(clamp(value, 0.0, 255.0) + 0.5)) << (byte * 8u);
    }
    output[word] = packed;
}