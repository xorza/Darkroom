// Contrast/brightness over a tightly-packed buffer, dispatched per `u32` word.
//
// Each invocation owns exactly one output word and builds it from the matching
// input word — pointwise, so element E of the output depends only on element E
// of the input (same word position). No row stride, no shared-word
// read-modify-write, no data races. Element layout follows the packed pixel
// bytes; `alpha_channel` (or NO_ALPHA) marks the channel copied through.
//
// Formula: out = (in - mid) * contrast + mid + brightness, clamped per type.

const NO_ALPHA: u32 = 0xffffffffu;

struct Params {
    contrast: f32,
    brightness: f32,
    total_bytes: u32,    // packed image size = width * height * bytes_per_pixel
    elem_size: u32,      // bytes per channel value: 1 (u8), 2 (u16), 4 (f32)
    channels: u32,       // channels per pixel (1..4)
    alpha_channel: u32,  // channel index copied through unchanged, or NO_ALPHA
    is_float: u32,       // 1 => f32 channels (normalized 0..1)
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> input: array<u32>;
@group(0) @binding(2) var<storage, read_write> output: array<u32>;

fn adjust_norm(v: f32) -> f32 {
    return (v - 0.5) * params.contrast + 0.5 + params.brightness;
}
fn adjust_u8(v: f32) -> f32 {
    let offset = 127.5 * (1.0 - params.contrast) + params.brightness * 255.0;
    return clamp(v * params.contrast + offset, 0.0, 255.0);
}
fn adjust_u16(v: f32) -> f32 {
    let offset = 32767.5 * (1.0 - params.contrast) + params.brightness * 65535.0;
    return clamp(v * params.contrast + offset, 0.0, 65535.0);
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = gid.x;
    let total_words = (params.total_bytes + 3u) / 4u;
    if w >= total_words {
        return;
    }

    let in_word = input[w];

    // f32: one channel value per word.
    if params.is_float == 1u {
        let channel = w % params.channels;
        if channel == params.alpha_channel {
            output[w] = in_word;
        } else {
            output[w] = bitcast<u32>(adjust_norm(bitcast<f32>(in_word)));
        }
        return;
    }

    var out_word: u32 = 0u;
    if params.elem_size == 2u {
        // u16: two channel values per word.
        for (var k: u32 = 0u; k < 2u; k++) {
            if w * 4u + k * 2u >= params.total_bytes {
                continue;
            }
            let raw = (in_word >> (k * 16u)) & 0xFFFFu;
            let channel = (w * 2u + k) % params.channels;
            var out: u32;
            if channel == params.alpha_channel {
                out = raw;
            } else {
                out = u32(adjust_u16(f32(raw)));
            }
            out_word = out_word | (out << (k * 16u));
        }
    } else {
        // u8: four channel values per word.
        for (var k: u32 = 0u; k < 4u; k++) {
            if w * 4u + k >= params.total_bytes {
                continue;
            }
            let raw = (in_word >> (k * 8u)) & 0xFFu;
            let channel = (w * 4u + k) % params.channels;
            var out: u32;
            if channel == params.alpha_channel {
                out = raw;
            } else {
                out = u32(adjust_u8(f32(raw)));
            }
            out_word = out_word | (out << (k * 8u));
        }
    }
    output[w] = out_word;
}
