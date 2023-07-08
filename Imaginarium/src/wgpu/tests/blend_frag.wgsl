struct VertexOutput {
    @location(0) tex1_coord: vec2<f32>,
    @location(1) tex2_coord: vec2<f32>,
    @builtin(position) position: vec4<f32>,
};

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
fn fs_mult_blend(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let color1 = textureSample(tex_1, the_sampler, vertex.tex1_coord);
    let color2 = textureSample(tex_2, the_sampler, vertex.tex2_coord);

    // RGBr = RGBa * A1 + RGB2 * A2 * (1 - A1)
    // Ar = A1 + A2 * (1 - A1)

    return vec4<f32>(color1 * color2);
}
