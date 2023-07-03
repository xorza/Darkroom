struct VertexOutput {
    @location(0) tex_coord: vec2<f32>,
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) tex_coord: vec2<f32>
) -> VertexOutput {
    var result: VertexOutput;
    result.position = vec4<f32>(position * 2.0 - 1.0, 0.0, 1.0);
    result.tex_coord = tex_coord * 255.0;
    return result;
}

@group(0)
@binding(1)
var tex_a: texture_2d<f32>;

@fragment
fn fs_main(vertex: VertexOutput) -> @location(0) vec4<f32> {
    let color_a = textureLoad(tex_a, vec2<i32>(vertex.tex_coord), 0);
    return vec4<f32>(color_a);
}
