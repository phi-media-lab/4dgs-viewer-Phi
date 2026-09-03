struct Record {
    center_opacity: vec4<f32>,
    velocity_time: vec4<f32>,
    scale_sigma: vec4<f32>,
    color: vec4<f32>,
};

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

struct SortStage {
    k: u32,
    j: u32,
    padding: vec2<u32>,
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

@group(0) @binding(0) var<storage, read> records: array<Record>;
@group(0) @binding(1) var<storage, read_write> projected: array<Projected>;
@group(0) @binding(2) var<storage, read_write> sort_entries: array<SortEntry>;
@group(0) @binding(3) var<uniform> params: Params;
@group(0) @binding(4) var<uniform> sort_stage: SortStage;
@group(0) @binding(5) var<storage, read_write> counters: Counters;
@group(0) @binding(6) var<storage, read_write> draw_args: DrawArgs;

@compute @workgroup_size(64)
fn reset_main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if (index == 0u) {
        counters.total = arrayLength(&records);
        atomicStore(&counters.active_count, 0u);
        atomicStore(&counters.visible_count, 0u);
        draw_args.vertex_count = 6u;
        atomicStore(&draw_args.instance_count, 0u);
        draw_args.first_vertex = 0u;
        draw_args.first_instance = 0u;
    }
    if (index < arrayLength(&sort_entries)) {
        sort_entries[index] = SortEntry(-3.402823e38, index, vec2<u32>(0u));
    }
}

@compute @workgroup_size(64)
fn project_main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let source_index = invocation.x;
    if (source_index >= arrayLength(&records)) {
        return;
    }
    let record = records[source_index];
    let delta_time = params.time - record.velocity_time.w;
    let center = record.center_opacity.xyz + record.velocity_time.xyz * delta_time;
    let normalized_time = delta_time / record.scale_sigma.w;
    let opacity = record.center_opacity.w * exp(-0.5 * normalized_time * normalized_time);
    let depth = center.z;
    let temporally_and_depth_valid = opacity >= params.alpha_min
        && depth > params.near
        && depth < params.far;

    if (!temporally_and_depth_valid) {
        projected[source_index] = Projected(
            vec4<f32>(0.0),
            vec4<f32>(1.0, 0.0, 1.0, 0.0),
            vec4<f32>(record.color.rgb, depth),
            vec4<u32>(source_index, 0u, 0u, 0u),
        );
        return;
    }
    atomicAdd(&counters.active_count, 1u);

    let inverse_depth = 1.0 / depth;
    let fx = params.focal_y / params.aspect;
    let fy = params.focal_y;
    let mean = vec2<f32>(fx * center.x * inverse_depth, fy * center.y * inverse_depth);
    let jx = fx * inverse_depth;
    let jy = fy * inverse_depth;
    let jxz = -fx * center.x * inverse_depth * inverse_depth;
    let jyz = -fy * center.y * inverse_depth * inverse_depth;
    let variance = record.scale_sigma.xyz * record.scale_sigma.xyz;
    let minimum_variance = params.min_sigma_ndc * params.min_sigma_ndc;
    let cxx = jx * jx * variance.x + jxz * jxz * variance.z + minimum_variance;
    let cyy = jy * jy * variance.y + jyz * jyz * variance.z + minimum_variance;
    let cxy = jxz * jyz * variance.z;
    let determinant = cxx * cyy - cxy * cxy;

    if (determinant <= 0.0) {
        projected[source_index] = Projected(
            vec4<f32>(0.0),
            vec4<f32>(1.0, 0.0, 1.0, 0.0),
            vec4<f32>(record.color.rgb, depth),
            vec4<u32>(source_index, 0u, 0u, 0u),
        );
        return;
    }

    let extent = 3.0 * sqrt(vec2<f32>(cxx, cyy));
    let conic = vec3<f32>(cyy, -cxy, cxx) / determinant;
    let visible = abs(mean.x) <= 1.0 + extent.x && abs(mean.y) <= 1.0 + extent.y;
    let valid = select(0u, 1u, visible);
    let visible_opacity = select(0.0, opacity, visible);
    projected[source_index] = Projected(
        vec4<f32>(mean, extent),
        vec4<f32>(conic, visible_opacity),
        vec4<f32>(record.color.rgb, depth),
        vec4<u32>(source_index, valid, 0u, 0u),
    );
    if (visible) {
        let compact_index = atomicAdd(&counters.visible_count, 1u);
        sort_entries[compact_index] = SortEntry(depth, source_index, vec2<u32>(0u));
        atomicAdd(&draw_args.instance_count, 1u);
    }
}

fn comes_before(left: SortEntry, right: SortEntry) -> bool {
    if (left.key > right.key) {
        return true;
    }
    if (left.key < right.key) {
        return false;
    }
    return left.source_index < right.source_index;
}

@compute @workgroup_size(64)
fn sort_main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let left_index = invocation.x;
    let right_index = left_index ^ sort_stage.j;
    if (left_index >= arrayLength(&sort_entries)
        || right_index >= arrayLength(&sort_entries)
        || right_index <= left_index) {
        return;
    }
    let left = sort_entries[left_index];
    let right = sort_entries[right_index];
    let descending = (left_index & sort_stage.k) == 0u;
    let left_before_right = comes_before(left, right);
    let should_swap = select(left_before_right, !left_before_right, descending);
    if (should_swap) {
        sort_entries[left_index] = right;
        sort_entries[right_index] = left;
    }
}
