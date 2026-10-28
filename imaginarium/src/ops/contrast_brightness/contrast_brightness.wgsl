// Contrast/brightness over a tightly-packed buffer, one invocation per `u32` word.
//
// Each invocation owns exactly one output word and builds it from the matching
// input word — pointwise, so element E of the output depends only on element E
// of the input (same word position). No row stride, no shared-word
// read-modify-write, no data races. Element layout follows the packed pixel
// bytes; `alpha_channel` (or NO_ALPHA) marks the channel copied through.
//
// Formula: out = in * scale + offset in the storage type's own units — the CPU's
// `ChannelAffine`, whose values the uniform carries — then `hold`, which `pack`
// saturates for an integer type and an f32 keeps.

struct Params {
    scale: f32,
    offset: f32,
    max: f32,
    clamp: u32,
    byte_len: u32,         // packed image size = width * height * bytes_per_pixel
    sample_byte_len: u32,  // bytes per channel value: 1 (u8), 2 (u16), 4 (f32)
    channel_count: u32,    // channels per pixel
    alpha_channel: u32,    // channel index copied through unchanged, or NO_ALPHA
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> data: array<u32>;

@compute @workgroup_size(256)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
) {
    let w = linear_index(gid, groups);
    if w * 4u >= params.byte_len {
        return;
    }

    let sample_byte_len = params.sample_byte_len;
    var out_word: u32 = 0u;
    for (var byte: u32 = 0u; byte < 4u; byte += sample_byte_len) {
        let offset = w * 4u + byte;
        if offset >= params.byte_len {
            break;
        }
        let shift = byte * 8u;
        let value = unpack(data[w], shift, sample_byte_len);
        var adjusted = value;
        if (offset / sample_byte_len) % params.channel_count != params.alpha_channel {
            adjusted = hold(value * params.scale + params.offset, params.clamp, params.max);
        }
        out_word = out_word | pack(adjusted, shift, sample_byte_len);
    }
    data[w] = out_word;
}
