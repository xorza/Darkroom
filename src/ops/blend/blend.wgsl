// Blend two tightly-packed buffers, dispatched per `u32` word.
//
// Blending is per-channel: out.c = blend(src.c, dst.c)·alpha + dst.c·(1-alpha),
// with the alpha channel (if any) passed through the blend as Normal. So output
// element E depends only on element E of src and dst — one invocation owns one
// output word, no row stride, no shared-word read-modify-write, no races.

const NO_ALPHA: u32 = 0xffffffffu;

struct Params {
    mode: u32,           // 0=Normal,1=Add,2=Subtract,3=Multiply,4=Screen,5=Overlay
    alpha: f32,
    total_bytes: u32,    // packed image size = width * height * bytes_per_pixel
    elem_size: u32,      // bytes per channel value: 1 (u8), 2 (u16), 4 (f32)
    channels: u32,       // channels per pixel (1..4)
    alpha_channel: u32,  // channel index passed through (Normal), or NO_ALPHA
    is_float: u32,       // 1 => f32 channels
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read> dst: array<u32>;
@group(0) @binding(3) var<storage, read_write> output: array<u32>;

fn blend_channel(s: f32, d: f32, mode: u32) -> f32 {
    switch mode {
        case 1u: { return min(s + d, 1.0); }            // Add
        case 2u: { return max(d - s, 0.0); }            // Subtract
        case 3u: { return s * d; }                      // Multiply
        case 4u: { return 1.0 - (1.0 - s) * (1.0 - d); } // Screen
        case 5u: {                                      // Overlay
            if d < 0.5 { return 2.0 * s * d; }
            return 1.0 - 2.0 * (1.0 - s) * (1.0 - d);
        }
        default: { return s; }                          // Normal
    }
}

// Normalized blend of one channel element. Alpha channels blend as Normal.
fn blend_elem(s: f32, d: f32, is_alpha: bool) -> f32 {
    var blended: f32;
    if is_alpha {
        blended = s;
    } else {
        blended = blend_channel(s, d, params.mode);
    }
    return blended * params.alpha + d * (1.0 - params.alpha);
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = gid.x;
    let total_words = (params.total_bytes + 3u) / 4u;
    if w >= total_words {
        return;
    }

    let s_word = src[w];
    let d_word = dst[w];

    // f32: one channel value per word.
    if params.is_float == 1u {
        let channel = w % params.channels;
        let is_alpha = channel == params.alpha_channel;
        let res = blend_elem(bitcast<f32>(s_word), bitcast<f32>(d_word), is_alpha);
        output[w] = bitcast<u32>(res);
        return;
    }

    var out_word: u32 = 0u;
    if params.elem_size == 2u {
        // u16: two channel values per word.
        for (var k: u32 = 0u; k < 2u; k++) {
            if w * 4u + k * 2u >= params.total_bytes {
                continue;
            }
            let s = f32((s_word >> (k * 16u)) & 0xFFFFu) / 65535.0;
            let d = f32((d_word >> (k * 16u)) & 0xFFFFu) / 65535.0;
            let is_alpha = ((w * 2u + k) % params.channels) == params.alpha_channel;
            let out = u32(clamp(blend_elem(s, d, is_alpha) * 65535.0, 0.0, 65535.0));
            out_word = out_word | (out << (k * 16u));
        }
    } else {
        // u8: four channel values per word.
        for (var k: u32 = 0u; k < 4u; k++) {
            if w * 4u + k >= params.total_bytes {
                continue;
            }
            let s = f32((s_word >> (k * 8u)) & 0xFFu) / 255.0;
            let d = f32((d_word >> (k * 8u)) & 0xFFu) / 255.0;
            let is_alpha = ((w * 4u + k) % params.channels) == params.alpha_channel;
            let out = u32(clamp(blend_elem(s, d, is_alpha) * 255.0, 0.0, 255.0));
            out_word = out_word | (out << (k * 8u));
        }
    }
    output[w] = out_word;
}
