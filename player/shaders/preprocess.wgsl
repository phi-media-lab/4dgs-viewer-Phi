// SPDX-FileCopyrightText: Copyright 2025 the Regents of the University of California, Nerfstudio Team and contributors. All rights reserved.
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Modified from gsplat's perspective covariance projection at commit
// 90d7b4b349e379ccf9ee6a8cef76aa40f48bb32e. Changes include translation to
// WGSL, vectorized principal-point bounds, WebGPU storage layout, temporal 4D
// evaluation, SH evaluation, validation and telemetry. See THIRD_PARTY.md.

#include "./common.wgsl"

@group(0) @binding(0) var<uniform> scene: SceneUniform;
@group(0) @binding(1) var<storage, read> gaussians: array<Gaussian4D>;
@group(0) @binding(2) var<storage, read_write> screens: array<ScreenGaussian>;
@group(0) @binding(3) var<storage, read_write> depth_keys: array<u32>;
@group(0) @binding(4) var<storage, read_write> instance_ids: array<u32>;
@group(0) @binding(5) var<storage, read_write> counters: FrameCounters;
@group(0) @binding(6) var<storage, read> sh3_words: array<u32>;

fn read_sh3_scalar(record_base: u32, scalar_index: u32) -> f32 {
    let pair = unpack2x16float(sh3_words[record_base + scalar_index / 2u]);
    return select(pair.x, pair.y, (scalar_index & 1u) != 0u);
}

fn read_sh3(index: u32, coefficient: u32) -> vec3<f32> {
    let record_base = index * 23u;
    let scalar = coefficient * 3u;
    return vec3<f32>(
        read_sh3_scalar(record_base, scalar),
        read_sh3_scalar(record_base, scalar + 1u),
        read_sh3_scalar(record_base, scalar + 2u),
    );
}

fn evaluate_color(index: u32, sh0: vec3<f32>, direction: vec3<f32>) -> vec3<f32> {
    var result = SH_C0 * sh0;
    if scene.flags.z >= 1u {
        let x = direction.x;
        let y = direction.y;
        let z = direction.z;
        let xx = x * x;
        let yy = y * y;
        let zz = z * z;
        result += read_sh3(index, 0u) * (-SH_C1 * y);
        result += read_sh3(index, 1u) * (SH_C1 * z);
        result += read_sh3(index, 2u) * (-SH_C1 * x);
        result += read_sh3(index, 3u) * (SH_C2_0 * x * y);
        result += read_sh3(index, 4u) * (SH_C2_1 * y * z);
        result += read_sh3(index, 5u) * (SH_C2_2 * (2.0 * zz - xx - yy));
        result += read_sh3(index, 6u) * (SH_C2_3 * x * z);
        result += read_sh3(index, 7u) * (SH_C2_4 * (xx - yy));
        result += read_sh3(index, 8u) * (SH_C3_0 * y * (3.0 * xx - yy));
        result += read_sh3(index, 9u) * (SH_C3_1 * x * y * z);
        result += read_sh3(index, 10u) * (SH_C3_2 * y * (4.0 * zz - xx - yy));
        result += read_sh3(index, 11u) * (SH_C3_3 * z * (2.0 * zz - 3.0 * xx - 3.0 * yy));
        result += read_sh3(index, 12u) * (SH_C3_4 * x * (4.0 * zz - xx - yy));
        result += read_sh3(index, 13u) * (SH_C3_5 * z * (xx - yy));
        result += read_sh3(index, 14u) * (SH_C3_6 * x * (xx - 3.0 * yy));
    }
    return max(vec3<f32>(0.0), vec3<f32>(0.5) + result);
}

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    let count = scene.flags.x;
    if index >= count { return; }

    let g = gaussians[index];
    let quat_len2 = dot(g.rotation_xyzw, g.rotation_xyzw);
    if (!finite3(g.mean_time.xyz) || !finite3(g.log_scale_opacity.xyz) ||
       !finite3(g.velocity_gate.xyz) || !finite3(g.sh0_duration.xyz) ||
       !finite_scalar(g.mean_time.w) || !finite_scalar(g.log_scale_opacity.w) ||
       !finite_scalar(g.velocity_gate.w) || !finite_scalar(g.sh0_duration.w) ||
       !finite_scalar(quat_len2) || !(quat_len2 > 1e-12)) {
        atomicAdd(&counters.invalid_count, 1u);
        return;
    }

    let time = scene.time_policy.x;
    let max_duration = scene.time_policy.y;
    let alpha_min = scene.time_policy.z;
    let temporal_threshold = scene.time_policy.w;
    let dt = time - g.mean_time.w;
    let gate = sigmoid(20.0 * g.velocity_gate.w);
    let duration = max(1e-6, max_duration / 6.0 * sigmoid(g.sh0_duration.w));
    let temporal = gate + (1.0 - gate) * exp(-0.5 * (dt / duration) * (dt / duration));
    var opacity = sigmoid(g.log_scale_opacity.w) * temporal;

    if (scene.flags.y != 0u && opacity < temporal_threshold) {
        atomicAdd(&counters.culled_temporal, 1u);
        return;
    }
    atomicAdd(&counters.active_count, 1u);

    let position = g.mean_time.xyz + dt * g.velocity_gate.xyz;
    let camera_position4 = scene.world_to_camera * vec4<f32>(position, 1.0);
    let camera_position = camera_position4.xyz;
    let near = scene.viewport.z;
    let far = scene.viewport.w;
    if (!(camera_position.z > near && camera_position.z < far)) {
        atomicAdd(&counters.culled_frustum, 1u);
        return;
    }

    let fx = scene.intrinsics.x;
    let fy = scene.intrinsics.y;
    let center = vec2<f32>(
        fx * camera_position.x / camera_position.z + scene.intrinsics.z,
        fy * camera_position.y / camera_position.z + scene.intrinsics.w,
    );

    let cov_world = covariance3d(g.log_scale_opacity.xyz, g.rotation_xyzw);
    let world_to_camera_rotation = mat3x3<f32>(
        scene.world_to_camera[0].xyz,
        scene.world_to_camera[1].xyz,
        scene.world_to_camera[2].xyz,
    );
    let cov_camera = world_to_camera_rotation * cov_world * transpose(world_to_camera_rotation);
    let inv_z = 1.0 / camera_position.z;
    let inv_z2 = inv_z * inv_z;
    let viewport = scene.viewport.xy;
    // Match gsplat's perspective Jacobian at the view boundary. The projected
    // mean remains unclamped; only the covariance linearization point is
    // bounded so a large off-screen Gaussian cannot explode across the image.
    let tan_fov = 0.5 * viewport / vec2<f32>(fx, fy);
    let limit_positive = (viewport - scene.intrinsics.zw) / vec2<f32>(fx, fy) + 0.3 * tan_fov;
    let limit_negative = scene.intrinsics.zw / vec2<f32>(fx, fy) + 0.3 * tan_fov;
    let clamped_xy_over_z = clamp(camera_position.xy * inv_z, -limit_negative, limit_positive);
    let jacobian_xy = camera_position.z * clamped_xy_over_z;
    let jacobian = mat3x2<f32>(
        vec2<f32>(fx * inv_z, 0.0),
        vec2<f32>(0.0, fy * inv_z),
        vec2<f32>(-fx * jacobian_xy.x * inv_z2, -fy * jacobian_xy.y * inv_z2),
    );
    let cov2_original = jacobian * cov_camera * transpose(jacobian);
    let det_original = determinant(cov2_original);
    let low_pass = scene.camera_position_sh.w;
    let cov2 = cov2_original + mat2x2<f32>(vec2<f32>(low_pass, 0.0), vec2<f32>(0.0, low_pass));
    let det_filtered = determinant(cov2);
    let compensate_opacity = (scene.flags.w & SCENE_FLAG_OPACITY_COMPENSATION) != 0u;
    if (!(det_filtered > 0.0) || !finite_scalar(det_filtered) ||
        (compensate_opacity && !(det_original > 0.0))) {
        atomicAdd(&counters.invalid_count, 1u);
        return;
    }

    if compensate_opacity {
        opacity *= sqrt(det_original / det_filtered);
    }
    if (!(opacity > alpha_min)) {
        atomicAdd(&counters.culled_footprint, 1u);
        return;
    }
    let conic_xx = cov2[1][1] / det_filtered;
    let conic_xy = -cov2[0][1] / det_filtered;
    let conic_yy = cov2[0][0] / det_filtered;
    let k = 2.0 * log(opacity / alpha_min);
    let projected_radii = sqrt(k * vec2<f32>(cov2[0][0], cov2[1][1]));
    let explicit_policy =
        (scene.flags.w & SCENE_FLAG_EXPLICIT_RASTER_POLICY) != 0u;
    // Preserve the old Phi bound only for manifests that omit the explicit
    // raster ABI. Pixel4DGS/gsplat classic declares radius_clip == 0 and must
    // not acquire an unmanifested 2048 px support clamp during conversion.
    let radii = select(
        min(projected_radii, vec2<f32>(LEGACY_MAX_RADIUS)),
        projected_radii,
        explicit_policy,
    );
    if (!finite3(vec3<f32>(radii, opacity))) {
        atomicAdd(&counters.invalid_count, 1u);
        return;
    }
    if center.x + radii.x < 0.0 || center.y + radii.y < 0.0 ||
       center.x - radii.x >= viewport.x || center.y - radii.y >= viewport.y {
        atomicAdd(&counters.culled_frustum, 1u);
        return;
    }

    let slot = atomicAdd(&counters.visible_count, 1u);
    let view_direction = normalize(position - scene.camera_position_sh.xyz);
    let color = evaluate_color(index, g.sh0_duration.xyz, view_direction);
    screens[slot].center_radii = vec4<f32>(center, radii);
    screens[slot].conic_opacity = vec4<f32>(conic_xx, conic_xy, conic_yy, opacity);
    screens[slot].color_depth = vec4<f32>(color, camera_position.z);
    depth_keys[slot] = bitcast<u32>(camera_position.z);
    instance_ids[slot] = slot;
}
