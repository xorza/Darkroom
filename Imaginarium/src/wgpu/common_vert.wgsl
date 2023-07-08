struct VertexOutput {
    @location(0) tex1_coord: vec2<f32>,
    @location(1) tex2_coord: vec2<f32>,
    @builtin(position) position: vec4<f32>,
};

struct PushConstants {
    tex1_transform: mat3x3<f32>,
    tex2_transform: mat3x3<f32>,
};
var<push_constant> pc: PushConstants;

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) tex_coord: vec2<f32>
) -> VertexOutput {
    var result: VertexOutput;
    result.position = vec4<f32>(position, 0.0, 1.0);
    var tex_coord = vec3<f32>(tex_coord, 1.0);

    result.tex1_coord = (pc.tex1_transform * tex_coord).xy;
    result.tex2_coord = (pc.tex2_transform * tex_coord).xy;

    return result;
}
