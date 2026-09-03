struct Record {
    center_sigma: vec4<f32>,
    time_gate: vec4<f32>,
    color: vec4<f32>,
};

struct Counters {
    total: u32,
    active_count: atomic<u32>,
    visible_count: atomic<u32>,
    padding: u32,
};

struct DrawArgs {
    vertex_count: u32,
    instance_count: atomic<u32>,
    first_vertex: u32,
    first_instance: u32,
};

struct Params {
    time: f32,
    alpha_min: f32,
    padding: vec2<u32>,
};

@group(0) @binding(0) var<storage, read> records: array<Record>;
@group(0) @binding(1) var<storage, read_write> counters: Counters;
@group(0) @binding(2) var<storage, read_write> active_indices: array<u32>;
@group(0) @binding(3) var<storage, read_write> visible_indices: array<u32>;
@group(0) @binding(4) var<storage, read_write> draw_args: DrawArgs;
@group(0) @binding(5) var<uniform> params: Params;

fn alpha_at(record: Record) -> f32 {
    let normalized = (params.time - record.time_gate.x) / record.time_gate.y;
    return record.time_gate.z * exp(-0.5 * normalized * normalized);
}

fn overlaps_view(record: Record) -> bool {
    let extent = 3.0 * record.center_sigma.zw;
    return abs(record.center_sigma.x) <= 1.0 + extent.x
        && abs(record.center_sigma.y) <= 1.0 + extent.y;
}

@compute @workgroup_size(1)
fn reset_main() {
    counters.total = arrayLength(&records);
    atomicStore(&counters.active_count, 0u);
    atomicStore(&counters.visible_count, 0u);
    draw_args.vertex_count = 6u;
    atomicStore(&draw_args.instance_count, 0u);
    draw_args.first_vertex = 0u;
    draw_args.first_instance = 0u;
}

@compute @workgroup_size(64)
fn active_main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let source_index = invocation.x;
    if (source_index >= arrayLength(&records)) {
        return;
    }
    if (alpha_at(records[source_index]) < params.alpha_min) {
        return;
    }
    let output_index = atomicAdd(&counters.active_count, 1u);
    active_indices[output_index] = source_index;
}

@compute @workgroup_size(64)
fn visible_main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let active_index = invocation.x;
    if (active_index >= atomicLoad(&counters.active_count)) {
        return;
    }
    let source_index = active_indices[active_index];
    if (!overlaps_view(records[source_index])) {
        return;
    }
    let output_index = atomicAdd(&counters.visible_count, 1u);
    visible_indices[output_index] = source_index;
    atomicAdd(&draw_args.instance_count, 1u);
}
