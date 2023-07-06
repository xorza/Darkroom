struct VertexOutput {
    @location(0) tex1_coord: vec2<f32>,
    @location(1) tex2_coord: vec2<f32>,
    @builtin(position) position: vec4<f32>,
};

struct PushConstants {
    tex1_size: vec2<f32>,
    tex2_size: vec2<f32>,
};
var<push_constant> pc: PushConstants;

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) tex_coord: vec2<f32>
) -> VertexOutput {
    var result: VertexOutput;
    result.position = vec4<f32>(position * 2.0 - 1.0, 0.0, 1.0);
    result.tex1_coord = tex_coord * pc.tex1_size;
    result.tex2_coord = tex_coord * pc.tex2_size;
    return result;
}

@group(0)
@binding(0)
var the_sampler: sampler;
@group(0)
@binding(1)
var tex_1: texture_2d<f32>;
@group(0)
@binding(2)
var tex_2: texture_2d<f32>;

@fragment
fn fs_main(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let color1 = textureLoad(tex_1, vec2<i32>(vertex.tex1_coord), 0);
    let color2 = textureLoad(tex_2, vec2<i32>(vertex.tex2_coord), 0);
    return vec4<f32>(color1 * color2);
}
