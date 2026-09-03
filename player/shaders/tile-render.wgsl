#include "./tile-render-core.wgsl"

@group(0) @binding(8) var accumulation: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(local_invocation_index) local_index: u32,
        @builtin(local_invocation_id) local_id: vec3<u32>,
        @builtin(workgroup_id) tile_id: vec3<u32>) {
    let result = render_tile_pixel(local_index, local_id, tile_id);
    let pixel_id = tile_id.xy * TILE_SIZE + local_id.xy;
    if result.valid != 0u {
        textureStore(accumulation, vec2<i32>(pixel_id),
            vec4<f32>(result.color, result.transmittance));
    }
}
