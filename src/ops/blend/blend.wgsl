// Blend two tightly-packed buffers, one invocation per `u32` word.
//
// Blending is per-channel: out.c = blend(src.c, dst.c)·alpha + dst.c·(1-alpha),
// with the alpha channel (if any) passed through the blend as Normal. So output
// element E depends only on element E of src and dst — one invocation owns one
// output word, no row stride, no shared-word read-modify-write, no races.

struct Params {
    mode: u32,           // 0=Normal,1=Add,2=Subtract,3=Multiply,4=Screen,5=Overlay
    alpha: f32,
    _pad0: u32,
    _pad1: u32,
    total_bytes: u32,    // packed image size = width * height * bytes_per_pixel
    elem_size: u32,      // bytes per channel value: 1 (u8), 2 (u16), 4 (f32)
    channels: u32,       // channels per pixel
    alpha_channel: u32,  // channel index passed through (Normal), or NO_ALPHA
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
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
) {
    let w = linear_index(gid, groups);
    if w * 4u >= params.total_bytes {
        return;
    }

    let elem = params.elem_size;
    let scale = full_scale(elem);
    var out_word: u32 = 0u;
    for (var byte: u32 = 0u; byte < 4u; byte += elem) {
        let offset = w * 4u + byte;
        if offset >= params.total_bytes {
            break;
        }
        let shift = byte * 8u;
        let s = unpack(src[w], shift, elem) / scale;
        let d = unpack(dst[w], shift, elem) / scale;
        let is_alpha = (offset / elem) % params.channels == params.alpha_channel;
        let blended = clamp(blend_elem(s, d, is_alpha) * scale, 0.0, scale);
        out_word = out_word | pack(blended, shift, elem);
    }
    output[w] = out_word;
}
