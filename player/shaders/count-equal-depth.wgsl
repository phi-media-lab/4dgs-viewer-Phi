#include "./common.wgsl"

const WORKGROUP_SIZE: u32 = 256u;
struct RadixSortParams { num_elements: u32, shift: u32, num_workgroups: u32, num_blocks_per_workgroup: u32 };
@group(0) @binding(0) var<storage, read> params: array<RadixSortParams, 4>;
@group(0) @binding(1) var<storage, read> sorted_keys: array<u32>;
@group(0) @binding(2) var<storage, read_write> counters: FrameCounters;

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(local_invocation_id) local_id: vec3<u32>, @builtin(workgroup_id) workgroup_id: vec3<u32>) {
    let p = params[0];
    for (var block = 0u; block < p.num_blocks_per_workgroup; block += 1u) {
        let element = workgroup_id.x * p.num_blocks_per_workgroup * WORKGROUP_SIZE + block * WORKGROUP_SIZE + local_id.x;
        if (element > 0u && element < p.num_elements && sorted_keys[element] == sorted_keys[element - 1u]) {
            atomicAdd(&counters.equal_depth, 1u);
        }
    }
}
