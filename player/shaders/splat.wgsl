#include "./common.wgsl"

@group(0) @binding(0) var<uniform> scene: SceneUniform;
@group(0) @binding(1) var<storage, read> screens: array<ScreenGaussian>;
@group(0) @binding(2) var<storage, read> sorted_ids: array<u32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) center: vec2<f32>,
    @location(1) conic: vec3<f32>,
    @location(2) color_opacity: vec4<f32>,
};

fn corner(index: u32) -> vec2<f32> {
    return array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0)
    )[index];
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    let gaussian = screens[sorted_ids[instance_index]];
    let pixel = gaussian.center_radii.xy + corner(vertex_index) * gaussian.center_radii.zw;
    let viewport = scene.viewport.xy;
    var output: VertexOutput;
    output.position = vec4<f32>(pixel.x / viewport.x * 2.0 - 1.0, 1.0 - pixel.y / viewport.y * 2.0, 0.0, 1.0);
    output.center = gaussian.center_radii.xy;
    output.conic = gaussian.conic_opacity.xyz;
    output.color_opacity = vec4<f32>(gaussian.color_depth.xyz, gaussian.conic_opacity.w);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let delta = input.position.xy - input.center;
    let exponent = -0.5 * (input.conic.x * delta.x * delta.x +
        2.0 * input.conic.y * delta.x * delta.y + input.conic.z * delta.y * delta.y);
    if exponent > 0.0 { discard; }
    let alpha = min(scene.raster_policy.x, input.color_opacity.w * exp(exponent));
    if alpha < scene.raster_policy.y || !(alpha == alpha) { discard; }
    return vec4<f32>(input.color_opacity.xyz * alpha, alpha);
}
