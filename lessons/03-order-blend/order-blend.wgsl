struct Gaussian {
    mean: vec2<f32>,
    sigma: vec2<f32>,
    color: vec4<f32>,
    depth: f32,
    opacity: f32,
    _padding: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) color: vec3<f32>,
    @location(2) @interpolate(flat) opacity: f32,
};

@group(0) @binding(0) var<storage, read> gaussians: array<Gaussian>;
@group(0) @binding(1) var<storage, read> draw_order: array<u32>;

const QUAD = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>( 1.0,  1.0),
);

@vertex
fn vs_gaussian(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let record = gaussians[draw_order[instance_index]];
    let corner = QUAD[vertex_index];

    var output: VertexOutput;
    output.position = vec4<f32>(record.mean + 3.0 * record.sigma * corner, 0.0, 1.0);
    output.local = 3.0 * corner;
    output.color = record.color.rgb;
    output.opacity = record.opacity;
    return output;
}

@fragment
fn fs_gaussian(input: VertexOutput) -> @location(0) vec4<f32> {
    let exponent = -0.5 * dot(input.local, input.local);
    let alpha = clamp(input.opacity * exp(exponent), 0.0, 0.999);
    if alpha < (1.0 / 255.0) {
        discard;
    }
    return vec4<f32>(input.color * alpha, alpha);
}

@vertex
fn vs_background(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fs_background() -> @location(0) vec4<f32> {
    return vec4<f32>(0.025, 0.025, 0.03, 1.0);
}
