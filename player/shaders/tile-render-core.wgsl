#include "./common.wgsl"

@group(0) @binding(0) var<uniform> scene: SceneUniform;
@group(0) @binding(1) var<storage, read> screens: array<ScreenGaussian>;
@group(0) @binding(2) var<storage, read> sorted_ids: array<u32>;
// WGSL atomics always require read_write storage access, even when this stage
// only performs atomicLoad. Dawn enforces this more strictly than Naga.
@group(0) @binding(3) var<storage, read_write> tile_counts: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> tile_rank_masks_0: array<atomic<u32>>;
@group(0) @binding(5) var<storage, read_write> tile_rank_masks_1: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read_write> tile_rank_masks_2: array<atomic<u32>>;
@group(0) @binding(7) var<storage, read_write> counters: FrameCounters;
// Binding 8 is intentionally supplied by the output-specific entry point.
@group(0) @binding(9) var<storage, read_write> persistent_flags: array<atomic<u32>>;

var<workgroup> telemetry_tests: array<u32, 256>;
var<workgroup> telemetry_early: array<u32, 256>;
var<workgroup> telemetry_budget: array<u32, 256>;
var<workgroup> telemetry_max_tests: array<u32, 256>;
var<workgroup> telemetry_budget_remaining: array<u32, 256>;

struct TilePixel {
    color: vec3<f32>,
    transmittance: f32,
    valid: u32,
};

fn load_rank_mask(shard: u32, index: u32) -> u32 {
    if shard == 0u { return atomicLoad(&tile_rank_masks_0[index]); }
    if shard == 1u { return atomicLoad(&tile_rank_masks_1[index]); }
    return atomicLoad(&tile_rank_masks_2[index]);
}

fn composite(rank: u32, pixel: vec2<f32>, accum: ptr<function, vec3<f32>>,
             transmittance: ptr<function, f32>) -> bool {
    let gaussian = screens[sorted_ids[rank]];
    let delta = pixel - gaussian.center_radii.xy;
    if any(abs(delta) > gaussian.center_radii.zw) { return false; }
    let conic = gaussian.conic_opacity.xyz;
    let exponent = -0.5 * (conic.x * delta.x * delta.x +
        2.0 * conic.y * delta.x * delta.y + conic.z * delta.y * delta.y);
    if exponent > 0.0 { return false; }
    let alpha = min(scene.raster_policy.x, gaussian.conic_opacity.w * exp(exponent));
    if alpha < scene.raster_policy.y || !(alpha == alpha) { return false; }
    let next_transmittance = *transmittance * (1.0 - alpha);
    let explicit_policy =
        (scene.flags.w & SCENE_FLAG_EXPLICIT_RASTER_POLICY) != 0u;
    if explicit_policy && next_transmittance <= scene.raster_policy.z {
        return true;
    }
    *accum += *transmittance * gaussian.color_depth.xyz * alpha;
    *transmittance = next_transmittance;
    return !explicit_policy && *transmittance < scene.raster_policy.z;
}

// This is the single compositing implementation used by both the native
// rgba16float accumulation target and the browser direct-presentation target.
// Rank masks are exact (one global sorted rank per bit), and every set bit is
// visited least-significant-first at all three hierarchy levels. The resulting
// composite order is therefore exactly sorted_ids[0..visible], front to back.
fn render_tile_pixel(local_index: u32, local_id: vec3<u32>,
                     tile_id: vec3<u32>) -> TilePixel {
    let viewport = vec2<u32>(scene.viewport.xy);
    let tile_columns = (viewport.x + TILE_SIZE - 1u) / TILE_SIZE;
    let tile = tile_id.y * tile_columns + tile_id.x;
    let raw_count = atomicLoad(&tile_counts[tile]);
    let pixel_id = tile_id.xy * TILE_SIZE + local_id.xy;
    let valid_pixel = all(pixel_id < viewport);
    if raw_count == 0u {
        return TilePixel(vec3<f32>(0.0), 1.0, u32(valid_pixel));
    }
    let telemetry_enabled = (scene.flags.w & SCENE_FLAG_TELEMETRY) != 0u;
    let interactive = (scene.flags.w & SCENE_FLAG_INTERACTIVE) != 0u;
    if telemetry_enabled && local_index == 0u {
        atomicMax(&counters.max_tile_load, raw_count);
    }

    let pixel = vec2<f32>(pixel_id) + vec2<f32>(0.5);
    var color = vec3<f32>(0.0);
    var transmittance = 1.0;
    var tests = 0u;
    var done = !valid_pixel;
    var early_terminated = false;
    var budget_limited = false;

    // Exact rank bits are visited in increasing global depth rank. Unlike atomic
    // append + bitonic sort this has no capacity, overflow fallback, or
    // power-of-two workload discontinuity.
    let visible = atomicLoad(&counters.visible_count);
    let rank_words_per_tile = (scene.flags.x + TILE_MASK_RANKS_PER_BIT * 32u - 1u) /
        (TILE_MASK_RANKS_PER_BIT * 32u);
    let level_one_words_per_tile = (rank_words_per_tile + 31u) / 32u;
    let level_two_words_per_tile = (level_one_words_per_tile + 31u) / 32u;
    let words_per_tile = rank_words_per_tile + level_one_words_per_tile +
        level_two_words_per_tile;
    let tile_rows = (viewport.y + TILE_SIZE - 1u) / TILE_SIZE;
    let tile_count = tile_columns * tile_rows;
    let tiles_per_shard = (tile_count + TILE_MASK_SHARDS - 1u) / TILE_MASK_SHARDS;
    let shard = tile / tiles_per_shard;
    let mask_base = (tile - shard * tiles_per_shard) * words_per_tile;
    let level_one_base = mask_base + rank_words_per_tile;
    let level_two_base = level_one_base + level_one_words_per_tile;
    for (var level_two_word = 0u;
         level_two_word < level_two_words_per_tile && !done;
         level_two_word += 1u) {
        var level_two_mask = load_rank_mask(shard, level_two_base + level_two_word);
        while level_two_mask != 0u && !done {
            let level_two_bit = firstTrailingBit(level_two_mask);
            let level_one_word = level_two_word * 32u + level_two_bit;
            if level_one_word < level_one_words_per_tile {
                var level_one_mask = load_rank_mask(shard, level_one_base + level_one_word);
                while level_one_mask != 0u && !done {
                    let level_one_bit = firstTrailingBit(level_one_mask);
                    let rank_word = level_one_word * 32u + level_one_bit;
                    if rank_word < rank_words_per_tile {
                        var rank_mask = load_rank_mask(shard, mask_base + rank_word);
                        while rank_mask != 0u && !done {
                            let rank_bit = firstTrailingBit(rank_mask);
                            let rank_base = (rank_word * 32u + rank_bit) *
                                TILE_MASK_RANKS_PER_BIT;
                            for (var offset = 0u;
                                 offset < TILE_MASK_RANKS_PER_BIT && !done;
                                 offset += 1u) {
                                let rank = rank_base + offset;
                                if rank < visible {
                                    if interactive && tests >= INTERACTIVE_MAX_PIXEL_TESTS {
                                        budget_limited = true;
                                        done = true;
                                    } else {
                                        tests += 1u;
                                        if composite(rank, pixel, &color, &transmittance) {
                                            early_terminated = true;
                                            done = true;
                                        }
                                    }
                                }
                            }
                            rank_mask &= rank_mask - 1u;
                        }
                    }
                    level_one_mask &= level_one_mask - 1u;
                }
            }
            level_two_mask &= level_two_mask - 1u;
        }
    }

    let early = u32(valid_pixel && early_terminated);
    let limited = u32(valid_pixel && budget_limited);
    telemetry_tests[local_index] = select(0u, tests, telemetry_enabled);
    telemetry_early[local_index] = select(0u, early, telemetry_enabled);
    // Sticky interaction truncation is a correctness/audit signal, not a
    // sampled performance counter. Preserve it on every frame even when the
    // heavier telemetry atomics are disabled between profiler samples.
    telemetry_budget[local_index] = limited;
    telemetry_max_tests[local_index] = select(0u, tests, telemetry_enabled);
    telemetry_budget_remaining[local_index] = select(
        0u, bitcast<u32>(transmittance), telemetry_enabled && budget_limited);
    workgroupBarrier();
    var stride = 128u;
    loop {
        if local_index < stride {
            telemetry_tests[local_index] += telemetry_tests[local_index + stride];
            telemetry_early[local_index] += telemetry_early[local_index + stride];
            telemetry_budget[local_index] += telemetry_budget[local_index + stride];
            telemetry_max_tests[local_index] = max(
                telemetry_max_tests[local_index], telemetry_max_tests[local_index + stride]);
            telemetry_budget_remaining[local_index] = max(
                telemetry_budget_remaining[local_index], telemetry_budget_remaining[local_index + stride]);
        }
        workgroupBarrier();
        if stride == 1u { break; }
        stride >>= 1u;
    }
    if local_index == 0u {
        if telemetry_enabled {
            atomicAdd(&counters.pixel_splat_tests, telemetry_tests[0]);
            atomicAdd(&counters.early_terminated_pixels, telemetry_early[0]);
            atomicAdd(&counters.budget_limited_pixels, telemetry_budget[0]);
            atomicMax(&counters.max_pixel_splat_tests, telemetry_max_tests[0]);
            atomicMax(&counters.max_budget_remaining_bits, telemetry_budget_remaining[0]);
        }
        if telemetry_budget[0] != 0u {
            atomicOr(&persistent_flags[0], PERSISTENT_INTERACTION_BUDGET_HIT);
        }
    }
    return TilePixel(color, transmittance, u32(valid_pixel));
}
