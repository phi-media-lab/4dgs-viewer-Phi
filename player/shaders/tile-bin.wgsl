#include "./common.wgsl"

@group(0) @binding(0) var<uniform> scene: SceneUniform;
@group(0) @binding(1) var<storage, read> screens: array<ScreenGaussian>;
@group(0) @binding(2) var<storage, read> sorted_ids: array<u32>;
@group(0) @binding(3) var<storage, read_write> tile_counts: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> tile_rank_masks_0: array<atomic<u32>>;
@group(0) @binding(5) var<storage, read_write> tile_rank_masks_1: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read_write> tile_rank_masks_2: array<atomic<u32>>;
@group(0) @binding(7) var<storage, read_write> counters: FrameCounters;
@group(0) @binding(8) var<storage, read_write> persistent_flags: array<atomic<u32>>;

var<workgroup> overlap_sums: array<u32, 256>;

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>,
        @builtin(local_invocation_index) local_index: u32) {
    let rank = gid.x;
    let visible = atomicLoad(&counters.visible_count);
    var overlap_count = 0u;

    if rank < visible {
        let gaussian = screens[sorted_ids[rank]];
        let viewport = scene.viewport.xy;
        let tile_columns = (u32(viewport.x) + TILE_SIZE - 1u) / TILE_SIZE;
        let tile_min = vec2<u32>(floor(max(gaussian.center_radii.xy - gaussian.center_radii.zw,
                                              vec2<f32>(0.0)) / f32(TILE_SIZE)));
        let tile_max = vec2<u32>(floor(min(gaussian.center_radii.xy + gaussian.center_radii.zw,
                                              viewport - vec2<f32>(1.0)) / f32(TILE_SIZE)));
        let tile_extent = tile_max - tile_min + vec2<u32>(1u);
        overlap_count = tile_extent.x * tile_extent.y;

        let rank_run = rank / TILE_MASK_RANKS_PER_BIT;
        let rank_word = rank_run / 32u;
        let rank_bit = 1u << (rank_run & 31u);
        let rank_words_per_tile = (scene.flags.x + TILE_MASK_RANKS_PER_BIT * 32u - 1u) /
            (TILE_MASK_RANKS_PER_BIT * 32u);
        let level_one_words_per_tile = (rank_words_per_tile + 31u) / 32u;
        let level_two_words_per_tile = (level_one_words_per_tile + 31u) / 32u;
        let words_per_tile = rank_words_per_tile + level_one_words_per_tile +
            level_two_words_per_tile;
        let tile_rows = (u32(viewport.y) + TILE_SIZE - 1u) / TILE_SIZE;
        let tile_count = tile_columns * tile_rows;
        let tiles_per_shard = (tile_count + TILE_MASK_SHARDS - 1u) / TILE_MASK_SHARDS;
        for (var tile_y = tile_min.y; tile_y <= tile_max.y; tile_y += 1u) {
            for (var tile_x = tile_min.x; tile_x <= tile_max.x; tile_x += 1u) {
                let tile = tile_y * tile_columns + tile_x;
                let shard = tile / tiles_per_shard;
                let tile_mask_base = (tile - shard * tiles_per_shard) * words_per_tile;
                let rank_mask_index = tile_mask_base + rank_word;
                let level_one_word = rank_word / 32u;
                let level_one_index = tile_mask_base + rank_words_per_tile + level_one_word;
                let level_one_bit = 1u << (rank_word & 31u);
                let level_two_index = tile_mask_base + rank_words_per_tile +
                    level_one_words_per_tile + level_one_word / 32u;
                let level_two_bit = 1u << (level_one_word & 31u);
                var address_valid = false;
                if shard == 0u && level_two_index < arrayLength(&tile_rank_masks_0) {
                    atomicOr(&tile_rank_masks_0[rank_mask_index], rank_bit);
                    atomicOr(&tile_rank_masks_0[level_one_index], level_one_bit);
                    atomicOr(&tile_rank_masks_0[level_two_index], level_two_bit);
                    address_valid = true;
                } else if shard == 1u && level_two_index < arrayLength(&tile_rank_masks_1) {
                    atomicOr(&tile_rank_masks_1[rank_mask_index], rank_bit);
                    atomicOr(&tile_rank_masks_1[level_one_index], level_one_bit);
                    atomicOr(&tile_rank_masks_1[level_two_index], level_two_bit);
                    address_valid = true;
                } else if shard == 2u && level_two_index < arrayLength(&tile_rank_masks_2) {
                    atomicOr(&tile_rank_masks_2[rank_mask_index], rank_bit);
                    atomicOr(&tile_rank_masks_2[level_one_index], level_one_bit);
                    atomicOr(&tile_rank_masks_2[level_two_index], level_two_bit);
                    address_valid = true;
                }
                if address_valid {
                    atomicAdd(&tile_counts[tile], 1u);
                } else {
                    // This is structurally impossible when the Rust allocation and
                    // shared constants agree. Keep a sticky GPU-side proof instead
                    // of silently writing past the workload representation.
                    atomicOr(&persistent_flags[0], PERSISTENT_MASK_ADDRESS_OVERFLOW);
                    atomicMax(&counters.tile_overflow, 1u);
                }
            }
        }
    }

    // Reduce hundreds of thousands of contended global telemetry atomics to one
    // add per preprocess workgroup. Tile-local count atomics remain distributed.
    overlap_sums[local_index] = overlap_count;
    workgroupBarrier();
    var stride = 128u;
    loop {
        if local_index < stride {
            overlap_sums[local_index] += overlap_sums[local_index + stride];
        }
        workgroupBarrier();
        if stride == 1u { break; }
        stride >>= 1u;
    }
    if local_index == 0u && (scene.flags.w & SCENE_FLAG_TELEMETRY) != 0u {
        atomicAdd(&counters.tile_overlaps, overlap_sums[0]);
    }
}
