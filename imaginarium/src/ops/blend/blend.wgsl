// Format types:
// 0 = GRAY_U8 (1 byte per pixel)
// 1 = GRAY_ALPHA_U8 (2 bytes per pixel)
// 2 = RGB_U8 (3 bytes per pixel)
// 3 = RGBA_U8 (4 bytes per pixel)
// 4 = GRAY_F32 (4 bytes per pixel, 1 float)
// 5 = GRAY_ALPHA_F32 (8 bytes per pixel, 2 floats)
// 6 = RGB_F32 (12 bytes per pixel, 3 floats)
// 7 = RGBA_F32 (16 bytes per pixel, 4 floats)
// 8 = GRAY_U16 (2 bytes per pixel)
// 9 = GRAY_ALPHA_U16 (4 bytes per pixel)
// 10 = RGB_U16 (6 bytes per pixel)
// 11 = RGBA_U16 (8 bytes per pixel)

struct Params {
    mode: u32,           // 0=Normal, 1=Add, 2=Subtract, 3=Multiply, 4=Screen, 5=Overlay
    alpha: f32,
    width: u32,
    height: u32,
    stride: u32,
    format_type: u32,
    _padding: vec2<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read> dst: array<u32>;
@group(0) @binding(3) var<storage, read_write> output: array<u32>;

// Read a pixel from src buffer
fn read_src_pixel(x: u32, y: u32) -> vec4<f32> {
    switch params.format_type {
        case 0u: {
            // GRAY_U8: 1 byte per pixel
            let byte_offset = y * params.stride + x;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word = src[u32_idx];
            let gray = f32((word >> (byte_in_u32 * 8u)) & 0xFFu) / 255.0;
            return vec4<f32>(gray, gray, gray, 1.0);
        }
        case 1u: {
            // GRAY_ALPHA_U8: 2 bytes per pixel
            let byte_offset = y * params.stride + x * 2u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word0 = src[u32_idx];
            var gray: f32;
            var alpha: f32;
            if byte_in_u32 <= 2u {
                gray = f32((word0 >> (byte_in_u32 * 8u)) & 0xFFu) / 255.0;
                alpha = f32((word0 >> ((byte_in_u32 + 1u) * 8u)) & 0xFFu) / 255.0;
            } else {
                let word1 = src[u32_idx + 1u];
                gray = f32((word0 >> 24u) & 0xFFu) / 255.0;
                alpha = f32(word1 & 0xFFu) / 255.0;
            }
            return vec4<f32>(gray, gray, gray, alpha);
        }
        case 2u: {
            // RGB_U8: 3 bytes per pixel
            let byte_offset = y * params.stride + x * 3u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word0 = src[u32_idx];
            let word1 = src[u32_idx + 1u];
            var r: u32;
            var g: u32;
            var b: u32;
            if byte_in_u32 == 0u {
                r = word0 & 0xFFu;
                g = (word0 >> 8u) & 0xFFu;
                b = (word0 >> 16u) & 0xFFu;
            } else if byte_in_u32 == 1u {
                r = (word0 >> 8u) & 0xFFu;
                g = (word0 >> 16u) & 0xFFu;
                b = (word0 >> 24u) & 0xFFu;
            } else if byte_in_u32 == 2u {
                r = (word0 >> 16u) & 0xFFu;
                g = (word0 >> 24u) & 0xFFu;
                b = word1 & 0xFFu;
            } else {
                r = (word0 >> 24u) & 0xFFu;
                g = word1 & 0xFFu;
                b = (word1 >> 8u) & 0xFFu;
            }
            return vec4<f32>(f32(r) / 255.0, f32(g) / 255.0, f32(b) / 255.0, 1.0);
        }
        case 3u: {
            // RGBA_U8: 4 bytes per pixel
            let stride_pixels = params.stride / 4u;
            let idx = y * stride_pixels + x;
            let packed = src[idx];
            return vec4<f32>(
                f32(packed & 0xFFu) / 255.0,
                f32((packed >> 8u) & 0xFFu) / 255.0,
                f32((packed >> 16u) & 0xFFu) / 255.0,
                f32((packed >> 24u) & 0xFFu) / 255.0
            );
        }
        case 4u: {
            // GRAY_F32: 4 bytes per pixel (1 float)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x;
            let gray = bitcast<f32>(src[idx]);
            return vec4<f32>(gray, gray, gray, 1.0);
        }
        case 5u: {
            // GRAY_ALPHA_F32: 8 bytes per pixel (2 floats)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x * 2u;
            let gray = bitcast<f32>(src[idx]);
            let alpha = bitcast<f32>(src[idx + 1u]);
            return vec4<f32>(gray, gray, gray, alpha);
        }
        case 6u: {
            // RGB_F32: 12 bytes per pixel (3 floats)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x * 3u;
            return vec4<f32>(
                bitcast<f32>(src[idx]),
                bitcast<f32>(src[idx + 1u]),
                bitcast<f32>(src[idx + 2u]),
                1.0
            );
        }
        case 7u: {
            // RGBA_F32: 16 bytes per pixel (4 floats)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x * 4u;
            return vec4<f32>(
                bitcast<f32>(src[idx]),
                bitcast<f32>(src[idx + 1u]),
                bitcast<f32>(src[idx + 2u]),
                bitcast<f32>(src[idx + 3u])
            );
        }
        case 8u: {
            // GRAY_U16: 2 bytes per pixel
            let byte_offset = y * params.stride + x * 2u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word = src[u32_idx];
            var gray: u32;
            if byte_in_u32 == 0u {
                gray = word & 0xFFFFu;
            } else {
                gray = (word >> 16u) & 0xFFFFu;
            }
            let g = f32(gray) / 65535.0;
            return vec4<f32>(g, g, g, 1.0);
        }
        case 9u: {
            // GRAY_ALPHA_U16: 4 bytes per pixel (word-aligned)
            let stride_u32 = params.stride / 4u;
            let idx = y * stride_u32 + x;
            let word = src[idx];
            let gray = f32(word & 0xFFFFu) / 65535.0;
            let alpha = f32((word >> 16u) & 0xFFFFu) / 65535.0;
            return vec4<f32>(gray, gray, gray, alpha);
        }
        case 10u: {
            // RGB_U16: 6 bytes per pixel
            let byte_offset = y * params.stride + x * 6u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word0 = src[u32_idx];
            let word1 = src[u32_idx + 1u];
            var r: u32;
            var g: u32;
            var b: u32;
            if byte_in_u32 == 0u {
                r = word0 & 0xFFFFu;
                g = (word0 >> 16u) & 0xFFFFu;
                b = word1 & 0xFFFFu;
            } else {
                r = (word0 >> 16u) & 0xFFFFu;
                g = word1 & 0xFFFFu;
                b = (word1 >> 16u) & 0xFFFFu;
            }
            return vec4<f32>(f32(r) / 65535.0, f32(g) / 65535.0, f32(b) / 65535.0, 1.0);
        }
        case 11u: {
            // RGBA_U16: 8 bytes per pixel (2 u32s per pixel)
            let stride_u32 = params.stride / 4u;
            let idx = y * stride_u32 + x * 2u;
            let word0 = src[idx];
            let word1 = src[idx + 1u];
            return vec4<f32>(
                f32(word0 & 0xFFFFu) / 65535.0,
                f32((word0 >> 16u) & 0xFFFFu) / 65535.0,
                f32(word1 & 0xFFFFu) / 65535.0,
                f32((word1 >> 16u) & 0xFFFFu) / 65535.0
            );
        }
        default: {
            return vec4<f32>(0.0, 0.0, 0.0, 1.0);
        }
    }
}

// Read a pixel from dst buffer
fn read_dst_pixel(x: u32, y: u32) -> vec4<f32> {
    switch params.format_type {
        case 0u: {
            // GRAY_U8: 1 byte per pixel
            let byte_offset = y * params.stride + x;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word = dst[u32_idx];
            let gray = f32((word >> (byte_in_u32 * 8u)) & 0xFFu) / 255.0;
            return vec4<f32>(gray, gray, gray, 1.0);
        }
        case 1u: {
            // GRAY_ALPHA_U8: 2 bytes per pixel
            let byte_offset = y * params.stride + x * 2u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word0 = dst[u32_idx];
            var gray: f32;
            var alpha: f32;
            if byte_in_u32 <= 2u {
                gray = f32((word0 >> (byte_in_u32 * 8u)) & 0xFFu) / 255.0;
                alpha = f32((word0 >> ((byte_in_u32 + 1u) * 8u)) & 0xFFu) / 255.0;
            } else {
                let word1 = dst[u32_idx + 1u];
                gray = f32((word0 >> 24u) & 0xFFu) / 255.0;
                alpha = f32(word1 & 0xFFu) / 255.0;
            }
            return vec4<f32>(gray, gray, gray, alpha);
        }
        case 2u: {
            // RGB_U8: 3 bytes per pixel
            let byte_offset = y * params.stride + x * 3u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word0 = dst[u32_idx];
            let word1 = dst[u32_idx + 1u];
            var r: u32;
            var g: u32;
            var b: u32;
            if byte_in_u32 == 0u {
                r = word0 & 0xFFu;
                g = (word0 >> 8u) & 0xFFu;
                b = (word0 >> 16u) & 0xFFu;
            } else if byte_in_u32 == 1u {
                r = (word0 >> 8u) & 0xFFu;
                g = (word0 >> 16u) & 0xFFu;
                b = (word0 >> 24u) & 0xFFu;
            } else if byte_in_u32 == 2u {
                r = (word0 >> 16u) & 0xFFu;
                g = (word0 >> 24u) & 0xFFu;
                b = word1 & 0xFFu;
            } else {
                r = (word0 >> 24u) & 0xFFu;
                g = word1 & 0xFFu;
                b = (word1 >> 8u) & 0xFFu;
            }
            return vec4<f32>(f32(r) / 255.0, f32(g) / 255.0, f32(b) / 255.0, 1.0);
        }
        case 3u: {
            // RGBA_U8: 4 bytes per pixel
            let stride_pixels = params.stride / 4u;
            let idx = y * stride_pixels + x;
            let packed = dst[idx];
            return vec4<f32>(
                f32(packed & 0xFFu) / 255.0,
                f32((packed >> 8u) & 0xFFu) / 255.0,
                f32((packed >> 16u) & 0xFFu) / 255.0,
                f32((packed >> 24u) & 0xFFu) / 255.0
            );
        }
        case 4u: {
            // GRAY_F32: 4 bytes per pixel (1 float)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x;
            let gray = bitcast<f32>(dst[idx]);
            return vec4<f32>(gray, gray, gray, 1.0);
        }
        case 5u: {
            // GRAY_ALPHA_F32: 8 bytes per pixel (2 floats)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x * 2u;
            let gray = bitcast<f32>(dst[idx]);
            let alpha = bitcast<f32>(dst[idx + 1u]);
            return vec4<f32>(gray, gray, gray, alpha);
        }
        case 6u: {
            // RGB_F32: 12 bytes per pixel (3 floats)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x * 3u;
            return vec4<f32>(
                bitcast<f32>(dst[idx]),
                bitcast<f32>(dst[idx + 1u]),
                bitcast<f32>(dst[idx + 2u]),
                1.0
            );
        }
        case 7u: {
            // RGBA_F32: 16 bytes per pixel (4 floats)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x * 4u;
            return vec4<f32>(
                bitcast<f32>(dst[idx]),
                bitcast<f32>(dst[idx + 1u]),
                bitcast<f32>(dst[idx + 2u]),
                bitcast<f32>(dst[idx + 3u])
            );
        }
        case 8u: {
            // GRAY_U16: 2 bytes per pixel
            let byte_offset = y * params.stride + x * 2u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word = dst[u32_idx];
            var gray: u32;
            if byte_in_u32 == 0u {
                gray = word & 0xFFFFu;
            } else {
                gray = (word >> 16u) & 0xFFFFu;
            }
            let g = f32(gray) / 65535.0;
            return vec4<f32>(g, g, g, 1.0);
        }
        case 9u: {
            // GRAY_ALPHA_U16: 4 bytes per pixel (word-aligned)
            let stride_u32 = params.stride / 4u;
            let idx = y * stride_u32 + x;
            let word = dst[idx];
            let gray = f32(word & 0xFFFFu) / 65535.0;
            let alpha = f32((word >> 16u) & 0xFFFFu) / 65535.0;
            return vec4<f32>(gray, gray, gray, alpha);
        }
        case 10u: {
            // RGB_U16: 6 bytes per pixel
            let byte_offset = y * params.stride + x * 6u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word0 = dst[u32_idx];
            let word1 = dst[u32_idx + 1u];
            var r: u32;
            var g: u32;
            var b: u32;
            if byte_in_u32 == 0u {
                r = word0 & 0xFFFFu;
                g = (word0 >> 16u) & 0xFFFFu;
                b = word1 & 0xFFFFu;
            } else {
                r = (word0 >> 16u) & 0xFFFFu;
                g = word1 & 0xFFFFu;
                b = (word1 >> 16u) & 0xFFFFu;
            }
            return vec4<f32>(f32(r) / 65535.0, f32(g) / 65535.0, f32(b) / 65535.0, 1.0);
        }
        case 11u: {
            // RGBA_U16: 8 bytes per pixel (2 u32s per pixel)
            let stride_u32 = params.stride / 4u;
            let idx = y * stride_u32 + x * 2u;
            let word0 = dst[idx];
            let word1 = dst[idx + 1u];
            return vec4<f32>(
                f32(word0 & 0xFFFFu) / 65535.0,
                f32((word0 >> 16u) & 0xFFFFu) / 65535.0,
                f32(word1 & 0xFFFFu) / 65535.0,
                f32((word1 >> 16u) & 0xFFFFu) / 65535.0
            );
        }
        default: {
            return vec4<f32>(0.0, 0.0, 0.0, 1.0);
        }
    }
}

fn blend_channel(s: f32, d: f32, mode: u32) -> f32 {
    switch mode {
        case 0u: { // Normal
            return s;
        }
        case 1u: { // Add
            return min(s + d, 1.0);
        }
        case 2u: { // Subtract
            return max(d - s, 0.0);
        }
        case 3u: { // Multiply
            return s * d;
        }
        case 4u: { // Screen
            return 1.0 - (1.0 - s) * (1.0 - d);
        }
        case 5u: { // Overlay
            if d < 0.5 {
                return 2.0 * s * d;
            } else {
                return 1.0 - 2.0 * (1.0 - s) * (1.0 - d);
            }
        }
        default: {
            return s;
        }
    }
}

fn blend_pixel(x: u32, y: u32) -> vec4<f32> {
    let src_color = read_src_pixel(x, y);
    let dst_color = read_dst_pixel(x, y);

    // Blend RGB channels with blend mode
    let blended_r = blend_channel(src_color.r, dst_color.r, params.mode);
    let blended_g = blend_channel(src_color.g, dst_color.g, params.mode);
    let blended_b = blend_channel(src_color.b, dst_color.b, params.mode);

    // Alpha uses normal blend (weighted average)
    let blended_a = src_color.a;

    // Apply alpha mixing: result = blended * alpha + dst * (1 - alpha)
    let alpha = params.alpha;
    let one_minus_alpha = 1.0 - alpha;

    return vec4<f32>(
        blended_r * alpha + dst_color.r * one_minus_alpha,
        blended_g * alpha + dst_color.g * one_minus_alpha,
        blended_b * alpha + dst_color.b * one_minus_alpha,
        blended_a * alpha + dst_color.a * one_minus_alpha
    );
}

// For GRAY_U8: Process 4 pixels at once to write a full u32
fn process_gray_u8_quad(base_x: u32, y: u32) {
    var packed: u32 = 0u;
    for (var i: u32 = 0u; i < 4u; i++) {
        let x = base_x + i;
        if x < params.width {
            let result = blend_pixel(x, y);
            let gray = u32(clamp(result.r * 255.0, 0.0, 255.0));
            packed = packed | (gray << (i * 8u));
        }
    }
    let byte_offset = y * params.stride + base_x;
    let u32_idx = byte_offset / 4u;
    output[u32_idx] = packed;
}

// For GRAY_ALPHA_U8: Process 2 pixels at once to write a full u32
fn process_gray_alpha_u8_pair(base_x: u32, y: u32) {
    var packed: u32 = 0u;
    for (var i: u32 = 0u; i < 2u; i++) {
        let x = base_x + i;
        if x < params.width {
            let result = blend_pixel(x, y);
            let gray = u32(clamp(result.r * 255.0, 0.0, 255.0));
            let alpha = u32(clamp(result.a * 255.0, 0.0, 255.0));
            packed = packed | ((gray | (alpha << 8u)) << (i * 16u));
        }
    }
    let byte_offset = y * params.stride + base_x * 2u;
    let u32_idx = byte_offset / 4u;
    output[u32_idx] = packed;
}

// For RGB_U8: Process pixels and write the bytes
fn process_rgb_u8_pixel(x: u32, y: u32) {
    let result = blend_pixel(x, y);
    let r = u32(clamp(result.r * 255.0, 0.0, 255.0));
    let g = u32(clamp(result.g * 255.0, 0.0, 255.0));
    let b = u32(clamp(result.b * 255.0, 0.0, 255.0));

    let byte_offset = y * params.stride + x * 3u;
    let u32_idx = byte_offset / 4u;
    let byte_in_u32 = byte_offset % 4u;

    // Write 3 bytes - this may span two u32s
    // We need to be careful about race conditions, so we use a simple approach
    // that writes full u32s when possible
    if byte_in_u32 == 0u {
        // Bytes 0,1,2 of word - can write without affecting byte 3
        let mask = 0xFF000000u;
        let old_val = output[u32_idx] & mask;
        output[u32_idx] = old_val | r | (g << 8u) | (b << 16u);
    } else if byte_in_u32 == 1u {
        // Bytes 1,2,3 of word - can write without affecting byte 0
        let mask = 0x000000FFu;
        let old_val = output[u32_idx] & mask;
        output[u32_idx] = old_val | (r << 8u) | (g << 16u) | (b << 24u);
    } else if byte_in_u32 == 2u {
        // Bytes 2,3 of word0 and byte 0 of word1
        let mask0 = 0x0000FFFFu;
        let mask1 = 0xFFFFFF00u;
        output[u32_idx] = (output[u32_idx] & mask0) | (r << 16u) | (g << 24u);
        output[u32_idx + 1u] = (output[u32_idx + 1u] & mask1) | b;
    } else {
        // Byte 3 of word0 and bytes 0,1 of word1
        let mask0 = 0x00FFFFFFu;
        let mask1 = 0xFFFF0000u;
        output[u32_idx] = (output[u32_idx] & mask0) | (r << 24u);
        output[u32_idx + 1u] = (output[u32_idx + 1u] & mask1) | g | (b << 8u);
    }
}

// For GRAY_U16: Process 2 pixels at once to write a full u32
fn process_gray_u16_pair(base_x: u32, y: u32) {
    var packed: u32 = 0u;
    for (var i: u32 = 0u; i < 2u; i++) {
        let x = base_x + i;
        if x < params.width {
            let result = blend_pixel(x, y);
            let gray = u32(clamp(result.r * 65535.0, 0.0, 65535.0));
            packed = packed | (gray << (i * 16u));
        }
    }
    let byte_offset = y * params.stride + base_x * 2u;
    let u32_idx = byte_offset / 4u;
    output[u32_idx] = packed;
}

// For RGB_U16: Process pixels and write the bytes
fn process_rgb_u16_pixel(x: u32, y: u32) {
    let result = blend_pixel(x, y);
    let r = u32(clamp(result.r * 65535.0, 0.0, 65535.0));
    let g = u32(clamp(result.g * 65535.0, 0.0, 65535.0));
    let b = u32(clamp(result.b * 65535.0, 0.0, 65535.0));

    let byte_offset = y * params.stride + x * 6u;
    let u32_idx = byte_offset / 4u;
    let byte_in_u32 = byte_offset % 4u;

    if byte_in_u32 == 0u {
        // R at bytes 0-1, G at bytes 2-3, B at bytes 4-5
        output[u32_idx] = r | (g << 16u);
        let mask1 = 0xFFFF0000u;
        output[u32_idx + 1u] = (output[u32_idx + 1u] & mask1) | b;
    } else {
        // byte_in_u32 == 2: R at bytes 2-3, G at bytes 4-5, B at bytes 6-7
        let mask0 = 0x0000FFFFu;
        output[u32_idx] = (output[u32_idx] & mask0) | (r << 16u);
        output[u32_idx + 1u] = g | (b << 16u);
    }
}

// For aligned formats (RGBA_U8, all F32, GRAY_ALPHA_U16, RGBA_U16): Simple per-pixel write
fn process_aligned_pixel(x: u32, y: u32) {
    let result = blend_pixel(x, y);

    switch params.format_type {
        case 3u: {
            // RGBA_U8: 4 bytes per pixel
            let stride_pixels = params.stride / 4u;
            let idx = y * stride_pixels + x;
            let r = u32(clamp(result.r * 255.0, 0.0, 255.0));
            let g = u32(clamp(result.g * 255.0, 0.0, 255.0));
            let b = u32(clamp(result.b * 255.0, 0.0, 255.0));
            let a = u32(clamp(result.a * 255.0, 0.0, 255.0));
            output[idx] = r | (g << 8u) | (b << 16u) | (a << 24u);
        }
        case 4u: {
            // GRAY_F32: 4 bytes per pixel (1 float)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x;
            output[idx] = bitcast<u32>(result.r);
        }
        case 5u: {
            // GRAY_ALPHA_F32: 8 bytes per pixel (2 floats)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x * 2u;
            output[idx] = bitcast<u32>(result.r);
            output[idx + 1u] = bitcast<u32>(result.a);
        }
        case 6u: {
            // RGB_F32: 12 bytes per pixel (3 floats)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x * 3u;
            output[idx] = bitcast<u32>(result.r);
            output[idx + 1u] = bitcast<u32>(result.g);
            output[idx + 2u] = bitcast<u32>(result.b);
        }
        case 7u: {
            // RGBA_F32: 16 bytes per pixel (4 floats)
            let stride_floats = params.stride / 4u;
            let idx = y * stride_floats + x * 4u;
            output[idx] = bitcast<u32>(result.r);
            output[idx + 1u] = bitcast<u32>(result.g);
            output[idx + 2u] = bitcast<u32>(result.b);
            output[idx + 3u] = bitcast<u32>(result.a);
        }
        case 9u: {
            // GRAY_ALPHA_U16: 4 bytes per pixel (word-aligned)
            let stride_u32 = params.stride / 4u;
            let idx = y * stride_u32 + x;
            let gray = u32(clamp(result.r * 65535.0, 0.0, 65535.0));
            let alpha = u32(clamp(result.a * 65535.0, 0.0, 65535.0));
            output[idx] = gray | (alpha << 16u);
        }
        case 11u: {
            // RGBA_U16: 8 bytes per pixel (2 u32s per pixel)
            let stride_u32 = params.stride / 4u;
            let idx = y * stride_u32 + x * 2u;
            let r = u32(clamp(result.r * 65535.0, 0.0, 65535.0));
            let g = u32(clamp(result.g * 65535.0, 0.0, 65535.0));
            let b = u32(clamp(result.b * 65535.0, 0.0, 65535.0));
            let a = u32(clamp(result.a * 65535.0, 0.0, 65535.0));
            output[idx] = r | (g << 16u);
            output[idx + 1u] = b | (a << 16u);
        }
        default: {}
    }
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    switch params.format_type {
        case 0u: {
            // GRAY_U8: Each thread processes 4 pixels (one u32)
            let quad_idx = global_id.x;
            let quads_per_row = (params.width + 3u) / 4u;
            let total_quads = quads_per_row * params.height;
            if quad_idx >= total_quads {
                return;
            }
            let y = quad_idx / quads_per_row;
            let quad_x = quad_idx % quads_per_row;
            let base_x = quad_x * 4u;
            process_gray_u8_quad(base_x, y);
        }
        case 1u: {
            // GRAY_ALPHA_U8: Each thread processes 2 pixels (one u32)
            let pair_idx = global_id.x;
            let pairs_per_row = (params.width + 1u) / 2u;
            let total_pairs = pairs_per_row * params.height;
            if pair_idx >= total_pairs {
                return;
            }
            let y = pair_idx / pairs_per_row;
            let pair_x = pair_idx % pairs_per_row;
            let base_x = pair_x * 2u;
            process_gray_alpha_u8_pair(base_x, y);
        }
        case 2u: {
            // RGB_U8: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_rgb_u8_pixel(x, y);
        }
        case 8u: {
            // GRAY_U16: Each thread processes 2 pixels (one u32)
            let pair_idx = global_id.x;
            let pairs_per_row = (params.width + 1u) / 2u;
            let total_pairs = pairs_per_row * params.height;
            if pair_idx >= total_pairs {
                return;
            }
            let y = pair_idx / pairs_per_row;
            let pair_x = pair_idx % pairs_per_row;
            let base_x = pair_x * 2u;
            process_gray_u16_pair(base_x, y);
        }
        case 10u: {
            // RGB_U16: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_rgb_u16_pixel(x, y);
        }
        default: {
            // Aligned formats: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_aligned_pixel(x, y);
        }
    }
}
