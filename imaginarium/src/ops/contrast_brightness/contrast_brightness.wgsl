// Contrast/brightness over a tightly-packed buffer, one invocation per `u32` word.
//
// Each invocation owns exactly one output word and builds it from the matching
// input word — pointwise, so element E of the output depends only on element E
// of the input (same word position). No row stride, no shared-word
// read-modify-write, no data races. Element layout follows the packed pixel
// bytes; `alpha_channel` (or NO_ALPHA) marks the channel copied through.
//
// Formula: out = clamp(in * scale + offset, 0, max) in the storage type's own
// units — the CPU's `ChannelAffine`, whose values the uniform carries.

struct Params {
    scale: f32,
    offset: f32,
    max: f32,
    _pad: u32,
    total_bytes: u32,    // packed image size = width * height * bytes_per_pixel
    elem_size: u32,      // bytes per channel value: 1 (u8), 2 (u16), 4 (f32)
    channels: u32,       // channels per pixel
    alpha_channel: u32,  // channel index copied through unchanged, or NO_ALPHA
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> input: array<u32>;
@group(0) @binding(2) var<storage, read_write> output: array<u32>;

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
    var out_word: u32 = 0u;
    for (var byte: u32 = 0u; byte < 4u; byte += elem) {
        let offset = w * 4u + byte;
        if offset >= params.total_bytes {
            break;
        }
        let shift = byte * 8u;
        let value = unpack(input[w], shift, elem);
        var adjusted = value;
        if (offset / elem) % params.channels != params.alpha_channel {
            adjusted = clamp(value * params.scale + params.offset, 0.0, params.max);
        }
        out_word = out_word | pack(adjusted, shift, elem);
    }
    output[w] = out_word;
}
