#include "./common.wgsl"

@group(0) @binding(0) var accumulation: texture_2d<f32>;
@group(0) @binding(1) var<uniform> background: vec4<f32>;
@group(0) @binding(2) var<uniform> scene: SceneUniform;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let dimensions = vec2<i32>(textureDimensions(accumulation));
    let scale = scene.viewport.xy / vec2<f32>(dimensions);
    var accum: vec4<f32>;
    if all(abs(scale - vec2<f32>(1.0)) < vec2<f32>(1e-6)) {
        accum = textureLoad(accumulation, vec2<i32>(position.xy), 0);
    } else {
        let source = position.xy * scale - vec2<f32>(0.5);
        let base = vec2<i32>(floor(source));
        let weight = fract(source);
        let maximum = dimensions - vec2<i32>(1);
        let p00 = clamp(base, vec2<i32>(0), maximum);
        let p10 = clamp(base + vec2<i32>(1, 0), vec2<i32>(0), maximum);
        let p01 = clamp(base + vec2<i32>(0, 1), vec2<i32>(0), maximum);
        let p11 = clamp(base + vec2<i32>(1, 1), vec2<i32>(0), maximum);
        let row0 = mix(textureLoad(accumulation, p00, 0), textureLoad(accumulation, p10, 0), weight.x);
        let row1 = mix(textureLoad(accumulation, p01, 0), textureLoad(accumulation, p11, 0), weight.x);
        accum = mix(row0, row1, weight.y);
    }
    var display_rgb = accum.rgb + accum.a * background.rgb;
    if (scene.flags.w & SCENE_FLAG_LINEAR_TO_SRGB) != 0u {
        let low = 12.92 * display_rgb;
        let high = 1.055 * pow(max(display_rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
        display_rgb = select(high, low, display_rgb <= vec3<f32>(0.0031308));
    }
    return vec4<f32>(clamp(display_rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
