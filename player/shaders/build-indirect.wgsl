#include "./common.wgsl"

const WORKGROUP_SIZE: u32 = 256u;
// The native path keeps the 256-workgroup default. Browser clients can tune
// this scheduling limit per adapter without changing radix keys, passes, or
// stable ordering. This matters because radix-scatter currently scans every
// workgroup histogram from every workgroup, making that portion of the pass
// quadratic in the dispatched workgroup count.
override MAX_RADIX_WORKGROUPS: u32 = 256u;

struct DispatchIndirectArgs { x: u32, y: u32, z: u32 };
struct DrawIndirectArgs { vertex_count: u32, instance_count: u32, first_vertex: u32, first_instance: u32 };
struct RadixSortParams { num_elements: u32, shift: u32, num_workgroups: u32, num_blocks_per_workgroup: u32 };

@group(0) @binding(0) var<storage, read_write> counters: FrameCounters;
@group(0) @binding(1) var<storage, read_write> dispatch_args: DispatchIndirectArgs;
@group(0) @binding(2) var<storage, read_write> radix_params: array<RadixSortParams, 4>;
@group(0) @binding(3) var<storage, read_write> draw_args: DrawIndirectArgs;

@compute @workgroup_size(1, 1, 1)
fn main() {
    let count = atomicLoad(&counters.visible_count);
    let total_blocks = (count + WORKGROUP_SIZE - 1u) / WORKGROUP_SIZE;
    let workgroups = min(total_blocks, MAX_RADIX_WORKGROUPS);
    var blocks_per_workgroup = 0u;
    if workgroups > 0u { blocks_per_workgroup = (total_blocks + workgroups - 1u) / workgroups; }
    dispatch_args = DispatchIndirectArgs(workgroups, 1u, 1u);
    for (var pass_number = 0u; pass_number < 4u; pass_number += 1u) {
        radix_params[pass_number] = RadixSortParams(count, pass_number * 8u, workgroups, blocks_per_workgroup);
    }
    draw_args = DrawIndirectArgs(6u, count, 0u, 0u);
}
