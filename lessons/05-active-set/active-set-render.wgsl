struct Record {
    center_sigma: vec4<f32>,
    time_gate: vec4<f32>,
    color: vec4<f32>,
};

struct Params {
    time: f32,
    alpha_min: f32,
    padding: vec2<u32>,
};

@group(0) @binding(0) var<storage, read> records: array<Record>;
@group(0) @binding(3) var<storage, read> visible_indices: array<u32>;
@group(0) @binding(5) var<uniform> params: Params;

fn alpha_at(record: Record) -> f32 {
    let normalized = (params.time - record.time_gate.x) / record.time_gate.y;
    return record.time_gate.z * exp(-0.5 * normalized * normalized);
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) color_alpha: vec4<f32>,
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
    let record = records[visible_indices[instance_index]];
    let local = corners[vertex_index];
    let extent = 3.0 * record.center_sigma.zw;

    var output: VertexOutput;
    output.position = vec4<f32>(record.center_sigma.xy + local * extent, 0.0, 1.0);
    output.local = local * 3.0;
    output.color_alpha = vec4<f32>(record.color.rgb, alpha_at(record));
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let exponent = -0.5 * dot(input.local, input.local);
    let alpha = input.color_alpha.a * exp(exponent);
    if (alpha < params.alpha_min * 0.1) {
        discard;
    }
    // Atomic compaction does not define instance order. Emit premultiplied RGB
    // for the pipeline's commutative additive diagnostic blend and preserve the
    // opaque target alpha with a zero source alpha.
    return vec4<f32>(input.color_alpha.rgb * alpha, 0.0);
}
