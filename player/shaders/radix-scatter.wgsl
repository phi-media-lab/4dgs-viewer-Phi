const WORKGROUP_SIZE: u32 = 256u;
const FLAG_WORDS: u32 = 8u;
struct RadixSortParams { num_elements: u32, shift: u32, num_workgroups: u32, num_blocks_per_workgroup: u32 };
struct PassIndex { value: u32 };
struct BinFlags { words: array<atomic<u32>, 8> };
@group(0) @binding(0) var<uniform> pass_index: PassIndex;
@group(0) @binding(1) var<storage, read> params: array<RadixSortParams, 4>;
@group(0) @binding(2) var<storage, read> keys_in: array<u32>;
@group(0) @binding(3) var<storage, read_write> keys_out: array<u32>;
@group(0) @binding(4) var<storage, read> values_in: array<u32>;
@group(0) @binding(5) var<storage, read_write> values_out: array<u32>;
@group(0) @binding(6) var<storage, read> histograms: array<u32>;
var<workgroup> global_offsets: array<u32, 256>;
var<workgroup> bin_scan: array<u32, 256>;
var<workgroup> bin_flags: array<BinFlags, 256>;

fn count_bits_before(bin: u32, word_id: u32, bit_mask: u32) -> u32 {
    var prefix = 0u;
    for (var word = 0u; word < FLAG_WORDS; word += 1u) {
        let bits = atomicLoad(&bin_flags[bin].words[word]);
        if word < word_id { prefix += countOneBits(bits); }
        if word == word_id { prefix += countOneBits(bits & (bit_mask - 1u)); }
    }
    return prefix;
}

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(local_invocation_id) local_id: vec3<u32>, @builtin(workgroup_id) workgroup_id: vec3<u32>) {
    let lid = local_id.x;
    let wid = workgroup_id.x;
    let p = params[pass_index.value];
    var total_count = 0u;
    var local_offset = 0u;
    let histogram_base = lid * p.num_workgroups;
    for (var group = 0u; group < p.num_workgroups; group += 1u) {
        if group == wid { local_offset = total_count; }
        total_count += histograms[histogram_base + group];
    }
    bin_scan[lid] = total_count;
    workgroupBarrier();

    var offset = 1u;
    loop {
        if offset >= 256u { break; }
        let index = (lid + 1u) * (offset << 1u) - 1u;
        if index < 256u { bin_scan[index] += bin_scan[index - offset]; }
        workgroupBarrier();
        offset <<= 1u;
    }
    if lid == 255u { bin_scan[255] = 0u; }
    workgroupBarrier();
    offset = 128u;
    loop {
        let index = (lid + 1u) * (offset << 1u) - 1u;
        if index < 256u {
            let temporary = bin_scan[index - offset];
            bin_scan[index - offset] = bin_scan[index];
            bin_scan[index] += temporary;
        }
        workgroupBarrier();
        if offset == 1u { break; }
        offset >>= 1u;
    }
    global_offsets[lid] = bin_scan[lid] + local_offset;
    workgroupBarrier();

    for (var block = 0u; block < p.num_blocks_per_workgroup; block += 1u) {
        for (var word = 0u; word < FLAG_WORDS; word += 1u) { atomicStore(&bin_flags[lid].words[word], 0u); }
        workgroupBarrier();
        let element = wid * p.num_blocks_per_workgroup * WORKGROUP_SIZE + block * WORKGROUP_SIZE + lid;
        let valid = element < p.num_elements;
        var key = 0u;
        var value = 0u;
        var bin = 0u;
        if valid {
            key = keys_in[element];
            value = values_in[element];
            bin = (key >> p.shift) & 255u;
            atomicOr(&bin_flags[bin].words[lid / 32u], 1u << (lid & 31u));
        }
        workgroupBarrier();
        var local_prefix = 0u;
        var block_count = 0u;
        if valid {
            local_prefix = count_bits_before(bin, lid / 32u, 1u << (lid & 31u));
            for (var word = 0u; word < FLAG_WORDS; word += 1u) {
                block_count += countOneBits(atomicLoad(&bin_flags[bin].words[word]));
            }
            let destination = global_offsets[bin] + local_prefix;
            keys_out[destination] = key;
            values_out[destination] = value;
        }
        workgroupBarrier();
        if valid && local_prefix == block_count - 1u { global_offsets[bin] += block_count; }
        workgroupBarrier();
    }
}
