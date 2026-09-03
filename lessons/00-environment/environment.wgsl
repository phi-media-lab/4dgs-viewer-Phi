struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>( 0.0,  0.72),
        vec2<f32>(-0.72, -0.58),
        vec2<f32>( 0.72, -0.58),
    );
    let colors = array<vec3<f32>, 3>(
        vec3<f32>(0.95, 0.22, 0.18),
        vec3<f32>(0.16, 0.78, 0.36),
        vec3<f32>(0.18, 0.42, 0.96),
    );

    var output: VertexOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    output.color = colors[vertex_index];
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 1.0);
}
