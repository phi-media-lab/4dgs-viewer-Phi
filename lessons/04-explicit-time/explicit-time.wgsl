struct Primitive4D {
    mean: vec2<f32>,
    velocity: vec2<f32>,
    color: vec4<f32>,
    time_center: f32,
    duration: f32,
    opacity: f32,
    moving: f32,
    scale: vec2<f32>,
    depth: f32,
    _padding: f32,
};

struct TimeState {
    time: f32,
    _padding: vec3<f32>,
};

struct Evaluation {
    mean: vec2<f32>,
    gate: f32,
    opacity: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) color: vec3<f32>,
    @location(2) @interpolate(flat) opacity: f32,
};

@group(0) @binding(0) var<storage, read> primitives: array<Primitive4D>;
@group(0) @binding(1) var<uniform> time_state: TimeState;
@group(0) @binding(2) var<storage, read> validation_times: array<f32>;
@group(0) @binding(3) var<storage, read_write> validation_output: array<vec4<f32>>;

const QUAD = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>( 1.0,  1.0),
);

fn evaluate_primitive(primitive: Primitive4D, time: f32) -> Evaluation {
    let is_moving = primitive.moving >= 0.5;
    let delta_time = time - primitive.time_center;
    let safe_duration = max(primitive.duration, 0.0001);
    let normalized_time = delta_time / safe_duration;
    let gate = select(1.0, exp(-0.5 * normalized_time * normalized_time), is_moving);
    let displacement = select(vec2<f32>(0.0), primitive.velocity * delta_time, is_moving);

    var result: Evaluation;
    result.mean = primitive.mean + displacement;
    result.gate = gate;
    result.opacity = clamp(primitive.opacity * gate, 0.0, 0.999);
    return result;
}

@vertex
fn vs_primitive(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let primitive = primitives[instance_index];
    let evaluation = evaluate_primitive(primitive, time_state.time);
    let corner = QUAD[vertex_index];

    var output: VertexOutput;
    output.position = vec4<f32>(evaluation.mean + 3.0 * primitive.scale * corner, 0.0, 1.0);
    output.local = 3.0 * corner;
    output.color = primitive.color.rgb;
    output.opacity = evaluation.opacity;
    return output;
}

@fragment
fn fs_primitive(input: VertexOutput) -> @location(0) vec4<f32> {
    let spatial_weight = exp(-0.5 * dot(input.local, input.local));
    let alpha = clamp(input.opacity * spatial_weight, 0.0, 0.999);
    if alpha < (1.0 / 255.0) {
        discard;
    }
    return vec4<f32>(input.color * alpha, alpha);
}

@compute @workgroup_size(64)
fn evaluate_for_validation(@builtin(global_invocation_id) id: vec3<u32>) {
    let primitive_count = arrayLength(&primitives);
    let time_count = arrayLength(&validation_times);
    let flat_index = id.x;
    if flat_index >= primitive_count * time_count {
        return;
    }
    let time_index = flat_index / primitive_count;
    let primitive_index = flat_index % primitive_count;
    let result = evaluate_primitive(primitives[primitive_index], validation_times[time_index]);
    validation_output[flat_index] = vec4<f32>(result.mean, result.gate, result.opacity);
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
