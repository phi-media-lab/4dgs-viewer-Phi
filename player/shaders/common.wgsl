const SH_C0: f32 = 0.28209479177387814;
const SH_C1: f32 = 0.4886025119029199;
const SH_C2_0: f32 = 1.0925484305920792;
const SH_C2_1: f32 = -1.0925484305920792;
const SH_C2_2: f32 = 0.31539156525252005;
const SH_C2_3: f32 = -1.0925484305920792;
const SH_C2_4: f32 = 0.5462742152960396;
const SH_C3_0: f32 = -0.5900435899266435;
const SH_C3_1: f32 = 2.890611442640554;
const SH_C3_2: f32 = -0.4570457994644658;
const SH_C3_3: f32 = 0.3731763325901154;
const SH_C3_4: f32 = -0.4570457994644658;
const SH_C3_5: f32 = 1.445305721320277;
const SH_C3_6: f32 = -0.5900435899266435;
const MAX_RADIUS: f32 = 2048.0;
const TILE_SIZE: u32 = 16u;
// One bit represents a short, consecutive run of global depth ranks. Tile masks
// are conservative: the renderer tests every rank in a present run, so false
// positives only cost work and never change compositing order or image quality.
const TILE_MASK_RANKS_PER_BIT: u32 = 1u;
const TILE_MASK_SHARDS: u32 = 3u;
const INTERACTIVE_MAX_PIXEL_TESTS: u32 = 2048u;
const SCENE_FLAG_TELEMETRY: u32 = 1u;
const SCENE_FLAG_INTERACTIVE: u32 = 2u;
const PERSISTENT_MASK_ADDRESS_OVERFLOW: u32 = 1u;
const PERSISTENT_INTERACTION_BUDGET_HIT: u32 = 2u;

struct Gaussian4D {
    mean_time: vec4<f32>,
    log_scale_opacity: vec4<f32>,
    rotation_xyzw: vec4<f32>,
    velocity_gate: vec4<f32>,
    sh0_duration: vec4<f32>,
};

struct ScreenGaussian {
    center_radii: vec4<f32>,
    conic_opacity: vec4<f32>,
    color_depth: vec4<f32>,
};

struct SceneUniform {
    world_to_camera: mat4x4<f32>,
    intrinsics: vec4<f32>,
    viewport: vec4<f32>,
    time_policy: vec4<f32>,
    background: vec4<f32>,
    camera_position_sh: vec4<f32>,
    flags: vec4<u32>,
};

struct FrameCounters {
    active_count: atomic<u32>,
    visible_count: atomic<u32>,
    invalid_count: atomic<u32>,
    culled_temporal: atomic<u32>,
    culled_frustum: atomic<u32>,
    culled_footprint: atomic<u32>,
    equal_depth: atomic<u32>,
    tile_overlaps: atomic<u32>,
    tile_overflow: atomic<u32>,
    max_tile_load: atomic<u32>,
    early_terminated_pixels: atomic<u32>,
    pixel_splat_tests: atomic<u32>,
    budget_limited_pixels: atomic<u32>,
    max_pixel_splat_tests: atomic<u32>,
    max_budget_remaining_bits: atomic<u32>,
    reserved: atomic<u32>,
};

fn sigmoid(x: f32) -> f32 {
    return 1.0 / (1.0 + exp(-x));
}

fn finite_scalar(x: f32) -> bool {
    return x == x && abs(x) < 1e30;
}

fn finite3(x: vec3<f32>) -> bool {
    return all(x == x) && all(abs(x) < vec3<f32>(1e30));
}

fn quat_to_mat3_xyzw(raw: vec4<f32>) -> mat3x3<f32> {
    let q = normalize(raw);
    let x = q.x;
    let y = q.y;
    let z = q.z;
    let w = q.w;
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let wx = w * x;
    let wy = w * y;
    let wz = w * z;
    return mat3x3<f32>(
        vec3<f32>(1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (xz - wy)),
        vec3<f32>(2.0 * (xy - wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + wx)),
        vec3<f32>(2.0 * (xz + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (xx + yy)),
    );
}

fn covariance3d(log_scale: vec3<f32>, rotation: vec4<f32>) -> mat3x3<f32> {
    let scale2 = exp(2.0 * log_scale);
    let r = quat_to_mat3_xyzw(rotation);
    return r * mat3x3<f32>(
        vec3<f32>(scale2.x, 0.0, 0.0),
        vec3<f32>(0.0, scale2.y, 0.0),
        vec3<f32>(0.0, 0.0, scale2.z),
    ) * transpose(r);
}
