// Affine resampling of a tightly-packed image, one invocation per output pixel.
//
// Channel values interpolate in their own units, as on the CPU, and narrow back
// through `pack`. Sub-word elements are ORed into the output, which the host
// clears first; a pixel's elements may share a word with its neighbours'.

struct Params {
    // Inverse transform: output pixel centre → input position.
    inv_matrix: mat2x2<f32>,
    inv_translation: vec2<f32>,
    input_size: vec2<u32>,
    output_size: vec2<u32>,
    channels: u32,
    elem_size: u32,
    // 0 = Nearest, 1 = Bilinear
    filter_mode: u32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> input_data: array<u32>;
@group(0) @binding(2) var<storage, read_write> output_data: array<atomic<u32>>;

fn read_elem(index: u32) -> f32 {
    let byte = index * params.elem_size;
    return unpack(input_data[byte / 4u], (byte % 4u) * 8u, params.elem_size);
}

fn write_elem(index: u32, value: f32) {
    let byte = index * params.elem_size;
    let bits = pack(value, (byte % 4u) * 8u, params.elem_size);
    if params.elem_size == 4u {
        atomicStore(&output_data[byte / 4u], bits);
    } else {
        atomicOr(&output_data[byte / 4u], bits);
    }
}

// The pixel's channels in the low lanes; the rest, and any pixel outside the
// input, read as zero.
fn read_pixel(x: i32, y: i32) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    if x < 0 || x >= i32(params.input_size.x) || y < 0 || y >= i32(params.input_size.y) {
        return color;
    }
    let base = (u32(y) * params.input_size.x + u32(x)) * params.channels;
    for (var k: u32 = 0u; k < params.channels; k++) {
        color[k] = read_elem(base + k);
    }
    return color;
}

fn sample_nearest(pos: vec2<f32>) -> vec4<f32> {
    return read_pixel(i32(round(pos.x)), i32(round(pos.y)));
}

fn sample_bilinear(pos: vec2<f32>) -> vec4<f32> {
    let x0 = i32(floor(pos.x));
    let y0 = i32(floor(pos.y));
    let fx = pos.x - f32(x0);
    let fy = pos.y - f32(y0);

    let c0 = mix(read_pixel(x0, y0), read_pixel(x0 + 1, y0), fx);
    let c1 = mix(read_pixel(x0, y0 + 1), read_pixel(x0 + 1, y0 + 1), fx);
    return mix(c0, c1, fy);
}

@compute @workgroup_size(256)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
) {
    let pixel = linear_index(gid, groups);
    if pixel >= params.output_size.x * params.output_size.y {
        return;
    }
    let out_x = pixel % params.output_size.x;
    let out_y = pixel / params.output_size.x;

    let out_pos = vec2<f32>(f32(out_x) + 0.5, f32(out_y) + 0.5);
    let src_pos = params.inv_matrix * out_pos + params.inv_translation - vec2<f32>(0.5, 0.5);

    var color: vec4<f32>;
    if params.filter_mode == 0u {
        color = sample_nearest(src_pos);
    } else {
        color = sample_bilinear(src_pos);
    }

    let base = pixel * params.channels;
    for (var k: u32 = 0u; k < params.channels; k++) {
        write_elem(base + k, color[k]);
    }
}
