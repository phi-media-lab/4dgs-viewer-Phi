struct Projected {
    mean_extent: vec4<f32>,
    conic_alpha: vec4<f32>,
    color_depth: vec4<f32>,
    source_valid: vec4<u32>,
};

struct SortEntry {
    key: f32,
    source_index: u32,
    padding: vec2<u32>,
};

struct Params {
    aspect: f32,
    time: f32,
    alpha_min: f32,
    focal_y: f32,
    near: f32,
    far: f32,
    min_sigma_ndc: f32,
    padding: f32,
};

@group(0) @binding(1) var<storage, read> projected: array<Projected>;
@group(0) @binding(2) var<storage, read> sort_entries: array<SortEntry>;
@group(0) @binding(3) var<uniform> params: Params;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) delta_ndc: vec2<f32>,
    @location(1) conic_alpha: vec4<f32>,
    @location(2) color: vec3<f32>,
    @location(3) @interpolate(flat) valid: u32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
    );
    let source_index = sort_entries[instance_index].source_index;
    let item = projected[source_index];
    let corner = corners[vertex_index];
    let delta = corner * item.mean_extent.zw;

    var output: VertexOutput;
    output.position = vec4<f32>(item.mean_extent.xy + delta, 0.0, 1.0);
    output.delta_ndc = delta;
    output.conic_alpha = item.conic_alpha;
    output.color = item.color_depth.rgb;
    output.valid = item.source_valid.y;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if (input.valid == 0u) {
        discard;
    }
    let conic = input.conic_alpha.xyz;
    let delta = input.delta_ndc;
    let exponent = conic.x * delta.x * delta.x
        + 2.0 * conic.y * delta.x * delta.y
        + conic.z * delta.y * delta.y;
    if (exponent > 9.0) {
        discard;
    }
    let alpha = input.conic_alpha.w * exp(-0.5 * exponent);
    if (alpha < params.alpha_min * 0.1) {
        discard;
    }
    return vec4<f32>(input.color, alpha);
}
