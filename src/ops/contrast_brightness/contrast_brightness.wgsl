// Contrast and brightness adjustment shader
// Formula: output = (input - mid) * contrast + mid + brightness
// For u8: mid = 127.5, for f32: mid = 0.5
// Alpha channel is preserved unchanged.

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
    contrast: f32,
    brightness: f32,
    width: u32,
    height: u32,
    stride: u32,
    format_type: u32,
    _padding: vec2<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> input: array<u32>;
@group(0) @binding(2) var<storage, read_write> output: array<u32>;

// Apply contrast/brightness to a single channel (normalized 0-1 range)
fn adjust_channel(value: f32) -> f32 {
    // For normalized values, mid = 0.5
    return (value - 0.5) * params.contrast + 0.5 + params.brightness;
}

// Apply contrast/brightness to a u8 channel (0-255 range)
fn adjust_channel_u8(value: f32) -> f32 {
    // offset = 127.5 * (1 - contrast) + brightness * 255
    let offset = 127.5 * (1.0 - params.contrast) + params.brightness * 255.0;
    return clamp(value * params.contrast + offset, 0.0, 255.0);
}

// Apply contrast/brightness to a u16 channel (0-65535 range)
fn adjust_channel_u16(value: f32) -> f32 {
    // offset = 32767.5 * (1 - contrast) + brightness * 65535
    let offset = 32767.5 * (1.0 - params.contrast) + params.brightness * 65535.0;
    return clamp(value * params.contrast + offset, 0.0, 65535.0);
}

// Process GRAY_U8: Each thread handles 4 pixels (one u32)
fn process_gray_u8_quad(base_x: u32, y: u32) {
    let byte_offset = y * params.stride + base_x;
    let u32_idx = byte_offset / 4u;
    let word = input[u32_idx];

    var result: u32 = 0u;
    for (var i: u32 = 0u; i < 4u; i++) {
        let x = base_x + i;
        if x < params.width {
            let gray = f32((word >> (i * 8u)) & 0xFFu);
            let adjusted = u32(adjust_channel_u8(gray));
            result = result | (adjusted << (i * 8u));
        }
    }
    output[u32_idx] = result;
}

// Process GRAY_ALPHA_U8: Each thread handles 2 pixels (one u32)
fn process_gray_alpha_u8_pair(base_x: u32, y: u32) {
    let byte_offset = y * params.stride + base_x * 2u;
    let u32_idx = byte_offset / 4u;
    let word = input[u32_idx];

    var result: u32 = 0u;
    for (var i: u32 = 0u; i < 2u; i++) {
        let x = base_x + i;
        if x < params.width {
            let gray = f32((word >> (i * 16u)) & 0xFFu);
            let alpha = (word >> (i * 16u + 8u)) & 0xFFu;
            let adjusted = u32(adjust_channel_u8(gray));
            result = result | ((adjusted | (alpha << 8u)) << (i * 16u));
        }
    }
    output[u32_idx] = result;
}

// Process RGB_U8: Each thread handles 1 pixel
fn process_rgb_u8_pixel(x: u32, y: u32) {
    let byte_offset = y * params.stride + x * 3u;
    let u32_idx = byte_offset / 4u;
    let byte_in_u32 = byte_offset % 4u;
    let word0 = input[u32_idx];
    let word1 = input[u32_idx + 1u];

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

    let r_out = u32(adjust_channel_u8(f32(r)));
    let g_out = u32(adjust_channel_u8(f32(g)));
    let b_out = u32(adjust_channel_u8(f32(b)));

    // Write back
    if byte_in_u32 == 0u {
        let mask = 0xFF000000u;
        output[u32_idx] = (output[u32_idx] & mask) | r_out | (g_out << 8u) | (b_out << 16u);
    } else if byte_in_u32 == 1u {
        let mask = 0x000000FFu;
        output[u32_idx] = (output[u32_idx] & mask) | (r_out << 8u) | (g_out << 16u) | (b_out << 24u);
    } else if byte_in_u32 == 2u {
        let mask0 = 0x0000FFFFu;
        let mask1 = 0xFFFFFF00u;
        output[u32_idx] = (output[u32_idx] & mask0) | (r_out << 16u) | (g_out << 24u);
        output[u32_idx + 1u] = (output[u32_idx + 1u] & mask1) | b_out;
    } else {
        let mask0 = 0x00FFFFFFu;
        let mask1 = 0xFFFF0000u;
        output[u32_idx] = (output[u32_idx] & mask0) | (r_out << 24u);
        output[u32_idx + 1u] = (output[u32_idx + 1u] & mask1) | g_out | (b_out << 8u);
    }
}

// Process RGBA_U8: Each thread handles 1 pixel
fn process_rgba_u8_pixel(x: u32, y: u32) {
    let stride_pixels = params.stride / 4u;
    let idx = y * stride_pixels + x;
    let pixel = input[idx];

    let r = f32(pixel & 0xFFu);
    let g = f32((pixel >> 8u) & 0xFFu);
    let b = f32((pixel >> 16u) & 0xFFu);
    let a = (pixel >> 24u) & 0xFFu;

    let r_out = u32(adjust_channel_u8(r));
    let g_out = u32(adjust_channel_u8(g));
    let b_out = u32(adjust_channel_u8(b));

    output[idx] = r_out | (g_out << 8u) | (b_out << 16u) | (a << 24u);
}

// Process GRAY_F32: Each thread handles 1 pixel
fn process_gray_f32_pixel(x: u32, y: u32) {
    let stride_floats = params.stride / 4u;
    let idx = y * stride_floats + x;
    let gray = bitcast<f32>(input[idx]);
    let adjusted = adjust_channel(gray);
    output[idx] = bitcast<u32>(adjusted);
}

// Process GRAY_ALPHA_F32: Each thread handles 1 pixel
fn process_gray_alpha_f32_pixel(x: u32, y: u32) {
    let stride_floats = params.stride / 4u;
    let idx = y * stride_floats + x * 2u;
    let gray = bitcast<f32>(input[idx]);
    let alpha = input[idx + 1u]; // Preserve alpha as-is
    let adjusted = adjust_channel(gray);
    output[idx] = bitcast<u32>(adjusted);
    output[idx + 1u] = alpha;
}

// Process RGB_F32: Each thread handles 1 pixel
fn process_rgb_f32_pixel(x: u32, y: u32) {
    let stride_floats = params.stride / 4u;
    let idx = y * stride_floats + x * 3u;
    let r = bitcast<f32>(input[idx]);
    let g = bitcast<f32>(input[idx + 1u]);
    let b = bitcast<f32>(input[idx + 2u]);
    output[idx] = bitcast<u32>(adjust_channel(r));
    output[idx + 1u] = bitcast<u32>(adjust_channel(g));
    output[idx + 2u] = bitcast<u32>(adjust_channel(b));
}

// Process RGBA_F32: Each thread handles 1 pixel
fn process_rgba_f32_pixel(x: u32, y: u32) {
    let stride_floats = params.stride / 4u;
    let idx = y * stride_floats + x * 4u;
    let r = bitcast<f32>(input[idx]);
    let g = bitcast<f32>(input[idx + 1u]);
    let b = bitcast<f32>(input[idx + 2u]);
    let a = input[idx + 3u]; // Preserve alpha as-is
    output[idx] = bitcast<u32>(adjust_channel(r));
    output[idx + 1u] = bitcast<u32>(adjust_channel(g));
    output[idx + 2u] = bitcast<u32>(adjust_channel(b));
    output[idx + 3u] = a;
}

// Process GRAY_U16: Each thread handles 2 pixels (one u32)
fn process_gray_u16_pair(base_x: u32, y: u32) {
    let byte_offset = y * params.stride + base_x * 2u;
    let u32_idx = byte_offset / 4u;
    let word = input[u32_idx];

    var result: u32 = 0u;
    for (var i: u32 = 0u; i < 2u; i++) {
        let x = base_x + i;
        if x < params.width {
            let gray = f32((word >> (i * 16u)) & 0xFFFFu);
            let adjusted = u32(adjust_channel_u16(gray));
            result = result | (adjusted << (i * 16u));
        }
    }
    output[u32_idx] = result;
}

// Process GRAY_ALPHA_U16: Each thread handles 1 pixel (one u32)
fn process_gray_alpha_u16_pixel(x: u32, y: u32) {
    let stride_u32 = params.stride / 4u;
    let idx = y * stride_u32 + x;
    let word = input[idx];

    let gray = f32(word & 0xFFFFu);
    let alpha = (word >> 16u) & 0xFFFFu; // Preserve alpha

    let adjusted = u32(adjust_channel_u16(gray));
    output[idx] = adjusted | (alpha << 16u);
}

// Process RGB_U16: Each thread handles 1 pixel
fn process_rgb_u16_pixel(x: u32, y: u32) {
    let byte_offset = y * params.stride + x * 6u;
    let u32_idx = byte_offset / 4u;
    let byte_in_u32 = byte_offset % 4u;
    let word0 = input[u32_idx];
    let word1 = input[u32_idx + 1u];

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

    let r_out = u32(adjust_channel_u16(f32(r)));
    let g_out = u32(adjust_channel_u16(f32(g)));
    let b_out = u32(adjust_channel_u16(f32(b)));

    if byte_in_u32 == 0u {
        output[u32_idx] = r_out | (g_out << 16u);
        let mask1 = 0xFFFF0000u;
        output[u32_idx + 1u] = (output[u32_idx + 1u] & mask1) | b_out;
    } else {
        let mask0 = 0x0000FFFFu;
        output[u32_idx] = (output[u32_idx] & mask0) | (r_out << 16u);
        output[u32_idx + 1u] = g_out | (b_out << 16u);
    }
}

// Process RGBA_U16: Each thread handles 1 pixel (2 u32s)
fn process_rgba_u16_pixel(x: u32, y: u32) {
    let stride_u32 = params.stride / 4u;
    let idx = y * stride_u32 + x * 2u;
    let word0 = input[idx];
    let word1 = input[idx + 1u];

    let r = f32(word0 & 0xFFFFu);
    let g = f32((word0 >> 16u) & 0xFFFFu);
    let b = f32(word1 & 0xFFFFu);
    let a = (word1 >> 16u) & 0xFFFFu; // Preserve alpha

    let r_out = u32(adjust_channel_u16(r));
    let g_out = u32(adjust_channel_u16(g));
    let b_out = u32(adjust_channel_u16(b));

    output[idx] = r_out | (g_out << 16u);
    output[idx + 1u] = b_out | (a << 16u);
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
        case 3u: {
            // RGBA_U8: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_rgba_u8_pixel(x, y);
        }
        case 4u: {
            // GRAY_F32: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_gray_f32_pixel(x, y);
        }
        case 5u: {
            // GRAY_ALPHA_F32: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_gray_alpha_f32_pixel(x, y);
        }
        case 6u: {
            // RGB_F32: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_rgb_f32_pixel(x, y);
        }
        case 7u: {
            // RGBA_F32: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_rgba_f32_pixel(x, y);
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
        case 9u: {
            // GRAY_ALPHA_U16: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_gray_alpha_u16_pixel(x, y);
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
        case 11u: {
            // RGBA_U16: Each thread processes 1 pixel
            let pixel_idx = global_id.x;
            let total_pixels = params.width * params.height;
            if pixel_idx >= total_pixels {
                return;
            }
            let x = pixel_idx % params.width;
            let y = pixel_idx / params.width;
            process_rgba_u16_pixel(x, y);
        }
        default: {}
    }
}
