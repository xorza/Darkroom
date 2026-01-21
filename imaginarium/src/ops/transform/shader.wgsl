// Format types:
// 0 = GRAY_U8 (1 byte per pixel)
// 1 = GRAY_ALPHA_U8 (2 bytes per pixel)
// 2 = RGB_U8 (3 bytes per pixel)
// 3 = RGBA_U8 (4 bytes per pixel, packed as u32)
// 4 = GRAY_F32 (4 bytes per pixel, 1 float)
// 5 = GRAY_ALPHA_F32 (8 bytes per pixel, 2 floats)
// 6 = RGB_F32 (12 bytes per pixel, 3 floats)
// 7 = RGBA_F32 (16 bytes per pixel, 4 floats)
// 8 = GRAY_U16 (2 bytes per pixel)
// 9 = GRAY_ALPHA_U16 (4 bytes per pixel)
// 10 = RGB_U16 (6 bytes per pixel)
// 11 = RGBA_U16 (8 bytes per pixel)

struct Params {
    // Inverse transform matrix (2x2) + translation (row-major)
    // [m00, m01, tx]
    // [m10, m11, ty]
    inv_matrix: mat2x2<f32>,
    inv_translation: vec2<f32>,

    input_size: vec2<u32>,
    output_size: vec2<u32>,

    input_stride: u32,
    output_stride: u32,

    // 0 = Nearest, 1 = Bilinear
    filter_mode: u32,
    // Format type (see above)
    format_type: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> input_data: array<u32>;
@group(0) @binding(2) var<storage, read_write> output_data: array<atomic<u32>>;

// Read a pixel as vec4<f32> (normalized to 0-1 for u8 formats)
// For grayscale, returns (gray, gray, gray, alpha)
fn read_pixel(x: i32, y: i32) -> vec4<f32> {
    if x < 0 || x >= i32(params.input_size.x) || y < 0 || y >= i32(params.input_size.y) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    let ux = u32(x);
    let uy = u32(y);

    switch params.format_type {
        case 0u: {
            // GRAY_U8: 1 byte per pixel
            let byte_offset = uy * params.input_stride + ux;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word = input_data[u32_idx];
            let gray = f32((word >> (byte_in_u32 * 8u)) & 0xFFu) / 255.0;
            return vec4<f32>(gray, gray, gray, 1.0);
        }
        case 1u: {
            // GRAY_ALPHA_U8: 2 bytes per pixel
            let byte_offset = uy * params.input_stride + ux * 2u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word0 = input_data[u32_idx];
            let word1 = input_data[u32_idx + 1u];

            var gray: u32;
            var alpha: u32;
            if byte_in_u32 == 0u {
                gray = word0 & 0xFFu;
                alpha = (word0 >> 8u) & 0xFFu;
            } else if byte_in_u32 == 1u {
                gray = (word0 >> 8u) & 0xFFu;
                alpha = (word0 >> 16u) & 0xFFu;
            } else if byte_in_u32 == 2u {
                gray = (word0 >> 16u) & 0xFFu;
                alpha = (word0 >> 24u) & 0xFFu;
            } else {
                gray = (word0 >> 24u) & 0xFFu;
                alpha = word1 & 0xFFu;
            }

            let g = f32(gray) / 255.0;
            return vec4<f32>(g, g, g, f32(alpha) / 255.0);
        }
        case 2u: {
            // RGB_U8: 3 bytes per pixel
            let byte_offset = uy * params.input_stride + ux * 3u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;

            let word0 = input_data[u32_idx];
            let word1 = input_data[u32_idx + 1u];

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
            // RGBA_U8: 4 bytes per pixel, stride in bytes
            let stride_pixels = params.input_stride / 4u;
            let idx = uy * stride_pixels + ux;
            let packed = input_data[idx];
            return vec4<f32>(
                f32(packed & 0xFFu) / 255.0,
                f32((packed >> 8u) & 0xFFu) / 255.0,
                f32((packed >> 16u) & 0xFFu) / 255.0,
                f32((packed >> 24u) & 0xFFu) / 255.0
            );
        }
        case 4u: {
            // GRAY_F32: 4 bytes per pixel (1 float)
            let stride_floats = params.input_stride / 4u;
            let idx = uy * stride_floats + ux;
            let gray = bitcast<f32>(input_data[idx]);
            return vec4<f32>(gray, gray, gray, 1.0);
        }
        case 5u: {
            // GRAY_ALPHA_F32: 8 bytes per pixel (2 floats)
            let stride_floats = params.input_stride / 4u;
            let idx = uy * stride_floats + ux * 2u;
            let gray = bitcast<f32>(input_data[idx]);
            let alpha = bitcast<f32>(input_data[idx + 1u]);
            return vec4<f32>(gray, gray, gray, alpha);
        }
        case 6u: {
            // RGB_F32: 12 bytes per pixel (3 floats)
            let stride_floats = params.input_stride / 4u;
            let idx = uy * stride_floats + ux * 3u;
            return vec4<f32>(
                bitcast<f32>(input_data[idx]),
                bitcast<f32>(input_data[idx + 1u]),
                bitcast<f32>(input_data[idx + 2u]),
                1.0
            );
        }
        case 7u: {
            // RGBA_F32: 16 bytes per pixel (4 floats)
            let stride_floats = params.input_stride / 4u;
            let idx = uy * stride_floats + ux * 4u;
            return vec4<f32>(
                bitcast<f32>(input_data[idx]),
                bitcast<f32>(input_data[idx + 1u]),
                bitcast<f32>(input_data[idx + 2u]),
                bitcast<f32>(input_data[idx + 3u])
            );
        }
        case 8u: {
            // GRAY_U16: 2 bytes per pixel
            let byte_offset = uy * params.input_stride + ux * 2u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let word = input_data[u32_idx];

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
            let stride_u32 = params.input_stride / 4u;
            let idx = uy * stride_u32 + ux;
            let word = input_data[idx];
            let gray = f32(word & 0xFFFFu) / 65535.0;
            let alpha = f32((word >> 16u) & 0xFFFFu) / 65535.0;
            return vec4<f32>(gray, gray, gray, alpha);
        }
        case 10u: {
            // RGB_U16: 6 bytes per pixel
            let byte_offset = uy * params.input_stride + ux * 6u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;

            let word0 = input_data[u32_idx];
            let word1 = input_data[u32_idx + 1u];
            let word2 = input_data[u32_idx + 2u];

            var r: u32;
            var g: u32;
            var b: u32;

            if byte_in_u32 == 0u {
                // R at bytes 0-1, G at bytes 2-3, B at bytes 4-5
                r = word0 & 0xFFFFu;
                g = (word0 >> 16u) & 0xFFFFu;
                b = word1 & 0xFFFFu;
            } else {
                // byte_in_u32 == 2: R at bytes 2-3, G at bytes 4-5, B at bytes 6-7
                r = (word0 >> 16u) & 0xFFFFu;
                g = word1 & 0xFFFFu;
                b = (word1 >> 16u) & 0xFFFFu;
            }

            return vec4<f32>(f32(r) / 65535.0, f32(g) / 65535.0, f32(b) / 65535.0, 1.0);
        }
        case 11u: {
            // RGBA_U16: 8 bytes per pixel (2 u32s per pixel)
            let stride_u32 = params.input_stride / 4u;
            let idx = uy * stride_u32 + ux * 2u;
            let word0 = input_data[idx];
            let word1 = input_data[idx + 1u];
            return vec4<f32>(
                f32(word0 & 0xFFFFu) / 65535.0,
                f32((word0 >> 16u) & 0xFFFFu) / 65535.0,
                f32(word1 & 0xFFFFu) / 65535.0,
                f32((word1 >> 16u) & 0xFFFFu) / 65535.0
            );
        }
        default: {
            return vec4<f32>(0.0, 0.0, 0.0, 0.0);
        }
    }
}

// Write a pixel from vec4<f32>
// For grayscale formats, uses the red channel as gray value
// Uses atomic OR for non-word-aligned formats to avoid race conditions
fn write_pixel(x: u32, y: u32, color: vec4<f32>) {
    switch params.format_type {
        case 0u: {
            // GRAY_U8: 1 byte per pixel - use atomic OR
            let byte_offset = y * params.output_stride + x;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let gray = u32(clamp(color.r * 255.0, 0.0, 255.0));
            atomicOr(&output_data[u32_idx], gray << (byte_in_u32 * 8u));
        }
        case 1u: {
            // GRAY_ALPHA_U8: 2 bytes per pixel - use atomic OR
            let byte_offset = y * params.output_stride + x * 2u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let gray = u32(clamp(color.r * 255.0, 0.0, 255.0));
            let alpha = u32(clamp(color.a * 255.0, 0.0, 255.0));

            if byte_in_u32 == 0u {
                atomicOr(&output_data[u32_idx], gray | (alpha << 8u));
            } else if byte_in_u32 == 1u {
                atomicOr(&output_data[u32_idx], (gray << 8u) | (alpha << 16u));
            } else if byte_in_u32 == 2u {
                atomicOr(&output_data[u32_idx], (gray << 16u) | (alpha << 24u));
            } else {
                atomicOr(&output_data[u32_idx], gray << 24u);
                atomicOr(&output_data[u32_idx + 1u], alpha);
            }
        }
        case 2u: {
            // RGB_U8: 3 bytes per pixel - use atomic OR
            let byte_offset = y * params.output_stride + x * 3u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;

            let r = u32(clamp(color.r * 255.0, 0.0, 255.0));
            let g = u32(clamp(color.g * 255.0, 0.0, 255.0));
            let b = u32(clamp(color.b * 255.0, 0.0, 255.0));

            if byte_in_u32 == 0u {
                atomicOr(&output_data[u32_idx], r | (g << 8u) | (b << 16u));
            } else if byte_in_u32 == 1u {
                atomicOr(&output_data[u32_idx], (r << 8u) | (g << 16u) | (b << 24u));
            } else if byte_in_u32 == 2u {
                atomicOr(&output_data[u32_idx], (r << 16u) | (g << 24u));
                atomicOr(&output_data[u32_idx + 1u], b);
            } else {
                atomicOr(&output_data[u32_idx], r << 24u);
                atomicOr(&output_data[u32_idx + 1u], g | (b << 8u));
            }
        }
        case 3u: {
            // RGBA_U8 - word-aligned, can use direct store
            let stride_pixels = params.output_stride / 4u;
            let idx = y * stride_pixels + x;
            let r = u32(clamp(color.r * 255.0, 0.0, 255.0));
            let g = u32(clamp(color.g * 255.0, 0.0, 255.0));
            let b = u32(clamp(color.b * 255.0, 0.0, 255.0));
            let a = u32(clamp(color.a * 255.0, 0.0, 255.0));
            atomicStore(&output_data[idx], r | (g << 8u) | (b << 16u) | (a << 24u));
        }
        case 4u: {
            // GRAY_F32 - word-aligned
            let stride_floats = params.output_stride / 4u;
            let idx = y * stride_floats + x;
            atomicStore(&output_data[idx], bitcast<u32>(color.r));
        }
        case 5u: {
            // GRAY_ALPHA_F32 - word-aligned
            let stride_floats = params.output_stride / 4u;
            let idx = y * stride_floats + x * 2u;
            atomicStore(&output_data[idx], bitcast<u32>(color.r));
            atomicStore(&output_data[idx + 1u], bitcast<u32>(color.a));
        }
        case 6u: {
            // RGB_F32 - word-aligned
            let stride_floats = params.output_stride / 4u;
            let idx = y * stride_floats + x * 3u;
            atomicStore(&output_data[idx], bitcast<u32>(color.r));
            atomicStore(&output_data[idx + 1u], bitcast<u32>(color.g));
            atomicStore(&output_data[idx + 2u], bitcast<u32>(color.b));
        }
        case 7u: {
            // RGBA_F32 - word-aligned
            let stride_floats = params.output_stride / 4u;
            let idx = y * stride_floats + x * 4u;
            atomicStore(&output_data[idx], bitcast<u32>(color.r));
            atomicStore(&output_data[idx + 1u], bitcast<u32>(color.g));
            atomicStore(&output_data[idx + 2u], bitcast<u32>(color.b));
            atomicStore(&output_data[idx + 3u], bitcast<u32>(color.a));
        }
        case 8u: {
            // GRAY_U16: 2 bytes per pixel - use atomic OR
            let byte_offset = y * params.output_stride + x * 2u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;
            let gray = u32(clamp(color.r * 65535.0, 0.0, 65535.0));

            if byte_in_u32 == 0u {
                atomicOr(&output_data[u32_idx], gray);
            } else {
                atomicOr(&output_data[u32_idx], gray << 16u);
            }
        }
        case 9u: {
            // GRAY_ALPHA_U16: 4 bytes per pixel (word-aligned)
            let stride_u32 = params.output_stride / 4u;
            let idx = y * stride_u32 + x;
            let gray = u32(clamp(color.r * 65535.0, 0.0, 65535.0));
            let alpha = u32(clamp(color.a * 65535.0, 0.0, 65535.0));
            atomicStore(&output_data[idx], gray | (alpha << 16u));
        }
        case 10u: {
            // RGB_U16: 6 bytes per pixel - use atomic OR
            let byte_offset = y * params.output_stride + x * 6u;
            let u32_idx = byte_offset / 4u;
            let byte_in_u32 = byte_offset % 4u;

            let r = u32(clamp(color.r * 65535.0, 0.0, 65535.0));
            let g = u32(clamp(color.g * 65535.0, 0.0, 65535.0));
            let b = u32(clamp(color.b * 65535.0, 0.0, 65535.0));

            if byte_in_u32 == 0u {
                // R at bytes 0-1, G at bytes 2-3, B at bytes 4-5
                atomicOr(&output_data[u32_idx], r | (g << 16u));
                atomicOr(&output_data[u32_idx + 1u], b);
            } else {
                // byte_in_u32 == 2: R at bytes 2-3, G at bytes 4-5, B at bytes 6-7
                atomicOr(&output_data[u32_idx], r << 16u);
                atomicOr(&output_data[u32_idx + 1u], g | (b << 16u));
            }
        }
        case 11u: {
            // RGBA_U16: 8 bytes per pixel (2 u32s per pixel, word-aligned)
            let stride_u32 = params.output_stride / 4u;
            let idx = y * stride_u32 + x * 2u;
            let r = u32(clamp(color.r * 65535.0, 0.0, 65535.0));
            let g = u32(clamp(color.g * 65535.0, 0.0, 65535.0));
            let b = u32(clamp(color.b * 65535.0, 0.0, 65535.0));
            let a = u32(clamp(color.a * 65535.0, 0.0, 65535.0));
            atomicStore(&output_data[idx], r | (g << 16u));
            atomicStore(&output_data[idx + 1u], b | (a << 16u));
        }
        default: {}
    }
}

fn sample_nearest(pos: vec2<f32>) -> vec4<f32> {
    let x = i32(round(pos.x));
    let y = i32(round(pos.y));
    return read_pixel(x, y);
}

fn sample_bilinear(pos: vec2<f32>) -> vec4<f32> {
    let x0 = i32(floor(pos.x));
    let y0 = i32(floor(pos.y));
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let fx = pos.x - f32(x0);
    let fy = pos.y - f32(y0);

    let c00 = read_pixel(x0, y0);
    let c10 = read_pixel(x1, y0);
    let c01 = read_pixel(x0, y1);
    let c11 = read_pixel(x1, y1);

    let c0 = mix(c00, c10, fx);
    let c1 = mix(c01, c11, fx);

    return mix(c0, c1, fy);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let out_x = global_id.x;
    let out_y = global_id.y;

    if out_x >= params.output_size.x || out_y >= params.output_size.y {
        return;
    }

    // Output pixel center in output coordinates
    let out_pos = vec2<f32>(f32(out_x) + 0.5, f32(out_y) + 0.5);

    // Apply inverse transform to get source position
    let src_pos = params.inv_matrix * out_pos + params.inv_translation;

    // Sample based on filter mode
    var color: vec4<f32>;
    if params.filter_mode == 0u {
        color = sample_nearest(src_pos - vec2<f32>(0.5, 0.5));
    } else {
        color = sample_bilinear(src_pos - vec2<f32>(0.5, 0.5));
    }

    write_pixel(out_x, out_y, color);
}
