const WORKGROUP_SIZE: u32 = 256u;
struct RadixSortParams { num_elements: u32, shift: u32, num_workgroups: u32, num_blocks_per_workgroup: u32 };
struct PassIndex { value: u32 };
@group(0) @binding(0) var<uniform> pass_index: PassIndex;
@group(0) @binding(1) var<storage, read> params: array<RadixSortParams, 4>;
@group(0) @binding(2) var<storage, read> keys_in: array<u32>;
@group(0) @binding(3) var<storage, read_write> histograms: array<u32>;
var<workgroup> histogram: array<atomic<u32>, 256>;

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(local_invocation_id) local_id: vec3<u32>, @builtin(workgroup_id) workgroup_id: vec3<u32>) {
    let lid = local_id.x;
    let wid = workgroup_id.x;
    let p = params[pass_index.value];
    atomicStore(&histogram[lid], 0u);
    workgroupBarrier();
    for (var block = 0u; block < p.num_blocks_per_workgroup; block += 1u) {
        let element = wid * p.num_blocks_per_workgroup * WORKGROUP_SIZE + block * WORKGROUP_SIZE + lid;
        if element < p.num_elements {
            atomicAdd(&histogram[(keys_in[element] >> p.shift) & 255u], 1u);
        }
    }
    workgroupBarrier();
    histograms[lid * p.num_workgroups + wid] = atomicLoad(&histogram[lid]);
}
