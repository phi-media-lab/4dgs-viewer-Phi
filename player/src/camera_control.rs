use serde::{Deserialize, Serialize};

use crate::{
    asset::FixedCamera,
    renderer::{self, CameraUniform},
};

pub(crate) const RECEIVER_TELEMETRY_SCHEMA: u32 = 7;
pub(crate) const RECEIVER_PROGRESS_SCHEMA: u32 = 1;
const ORBIT_FOLLOW_RATE_PER_SECOND: f32 = 36.0;
const ZOOM_FOLLOW_RATE_PER_SECOND: f32 = 36.0;

#[derive(Debug, Clone)]
pub struct CameraController {
    initial_fixed: FixedCamera,
    fixed: bool,
    target: [f32; 3],
    yaw: f32,
    target_yaw: f32,
    pitch: f32,
    target_pitch: f32,
    distance: f32,
    target_distance: f32,
    roll: f32,
    near: f32,
    far: f32,
    pub time: f32,
    pub playing: bool,
    input_epoch: Option<u32>,
    input_sequence: u64,
    input_orbit_x: f32,
    input_orbit_y: f32,
    input_zoom: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, tag = "type", rename_all = "kebab-case")]
pub enum ControlMessage {
    Orbit {
        dx: f32,
        dy: f32,
    },
    Zoom {
        delta: f32,
    },
    CameraState {
        epoch: u32,
        sequence: u64,
        client_time_ms: f64,
        orbit_x: f32,
        orbit_y: f32,
        zoom: f32,
    },
    Reset,
    SetTime {
        value: f32,
    },
    SetPlaying {
        value: bool,
    },
    KeyframeRequest {
        connection_generation: u64,
        #[serde(default)]
        active_ssrc: u32,
        request_id: u64,
        last_frames_received: u64,
        client_time_ms: f64,
        reason: String,
    },
    ReceiverStats {
        #[serde(flatten)]
        stats: Box<ReceiverStats>,
    },
    ReceiverProgress {
        #[serde(flatten)]
        progress: ReceiverProgress,
    },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReceiverProgress {
    pub progress_schema: u32,
    pub sample_time_ms: f64,
    pub connection_generation: u64,
    pub active_ssrc: u32,
    pub frames_received: u64,
    pub packets_received: u64,
    pub pli_count: u64,
    pub fir_count: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReceiverStats {
    pub telemetry_schema: u32,
    pub stats_sample_time_ms: f64,
    pub preview_owner_id: String,
    pub frames_received: u64,
    pub frames_decoded: u64,
    pub frames_dropped_supported: bool,
    pub frames_dropped: Option<u64>,
    pub key_frames_decoded: u64,
    pub packets_received: u64,
    pub packets_lost: i64,
    pub bytes_received: u64,
    pub retransmitted_packets_received: u64,
    pub retransmitted_bytes_received: u64,
    pub jitter_buffer_delay_ms: f64,
    pub jitter_buffer_target_delay_ms: f64,
    pub jitter_buffer_minimum_delay_ms: f64,
    pub jitter_buffer_delay_interval_ms: f64,
    pub jitter_buffer_target_delay_interval_ms: f64,
    pub jitter_buffer_minimum_delay_interval_ms: f64,
    pub jitter_buffer_emitted_count: u64,
    pub jitter_buffer_delay_total_ms: f64,
    pub jitter_buffer_target_delay_total_ms: f64,
    pub jitter_buffer_minimum_delay_total_ms: f64,
    pub receiver_jitter_buffer_target_mode: String,
    pub receiver_jitter_buffer_target_ms: Option<f64>,
    pub receiver_jitter_buffer_target_api: String,
    pub receiver_jitter_buffer_target_readback_ms: Option<f64>,
    pub receiver_playout_delay_hint_readback_ms: Option<f64>,
    pub rtt_ms: f64,
    pub rtt_max_ms: f64,
    pub selected_candidate_pair_id: String,
    pub selected_candidate_pair_available_incoming_bitrate_bps: f64,
    pub selected_candidate_pair_available_outgoing_bitrate_bps: f64,
    pub selected_candidate_pair_bytes_sent: u64,
    pub selected_candidate_pair_bytes_received: u64,
    pub local_candidate_type: String,
    pub local_candidate_protocol: String,
    pub local_candidate_address: String,
    pub local_candidate_port: u32,
    pub remote_candidate_type: String,
    pub remote_candidate_protocol: String,
    pub remote_candidate_address: String,
    pub remote_candidate_port: u32,
    pub nack_count: u64,
    pub pli_count: u64,
    pub fir_count: u64,
    pub freeze_count: u64,
    pub total_freezes_duration_ms: f64,
    pub total_decode_time_ms: f64,
    pub frames_rendered_supported: bool,
    pub frames_rendered: Option<u64>,
    pub packets_discarded: u64,
    pub total_processing_delay_ms: f64,
    pub total_inter_frame_delay_ms: f64,
    pub decoder_implementation_supported: bool,
    pub decoder_implementation: Option<String>,
    pub power_efficient_decoder_supported: bool,
    pub power_efficient_decoder: Option<bool>,
    pub presented_frames: u64,
    pub presentation_probe_enabled: bool,
    pub presentation_fps: f64,
    pub presentation_p99_ms: f64,
    pub presentation_max_ms: f64,
    pub presentation_gaps_over_50ms: u64,
    pub presentation_timing_samples: u64,
    pub presentation_timing_total_samples: u64,
    pub presentation_timing_censored_frames: u64,
    pub video_frame_callbacks: u64,
    pub video_frame_callback_missed: u64,
    pub video_frame_callback_lead_p01_ms: f64,
    pub video_frame_callback_lead_min_ms: f64,
    pub capture_to_display_samples: u64,
    pub capture_to_display_p50_ms: f64,
    pub capture_to_display_p99_ms: f64,
    pub capture_to_display_max_ms: f64,
    pub capture_to_receive_p99_ms: f64,
    pub receive_to_display_p99_ms: f64,
    pub frame_processing_p99_ms: f64,
    pub video_playback_total_frames: u64,
    pub video_playback_dropped_frames: u64,
    pub control_messages_sent: u64,
    pub control_input_messages_sent: u64,
    pub control_drag_input_messages_sent: u64,
    pub control_wheel_input_messages_sent: u64,
    pub control_input_to_send_p99_ms: f64,
    pub control_input_to_send_max_ms: f64,
    pub control_buffered_amount_max_bytes: u64,
    pub control_backpressure_skip_count: u64,
    pub animation_probe_enabled: bool,
    pub animation_frame_fps: f64,
    pub animation_frame_p99_ms: f64,
    pub animation_frame_max_ms: f64,
    pub long_task_probe_enabled: bool,
    pub long_task_count: u64,
    pub long_task_total_ms: f64,
    pub long_task_max_ms: f64,
    pub page_visible: bool,
    pub page_focused: bool,
    pub connection_generation: u64,
    pub page_visibility_transition_count: u64,
    pub page_focus_transition_count: u64,
    pub active_ssrc: u32,
    pub width: u32,
    pub height: u32,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub device_pixel_ratio: f64,
    pub video_client_width: u32,
    pub video_client_height: u32,
    pub video_paused: bool,
    pub video_ready_state: u32,
    pub video_current_time_s: f64,
    pub video_live_track_count: u32,
    pub video_track_ready_state: String,
    pub video_track_muted: bool,
    pub media_attach_count: u64,
    pub media_detach_count: u64,
    pub media_recovery_state: String,
    pub media_stall_age_ms: f64,
    pub media_first_frame_wait_ms: f64,
    pub media_keyframe_requests_sent: u64,
    pub media_keyframe_request_id: u64,
    pub media_keyframe_request_age_ms: f64,
    pub reconnect_count: u64,
    pub reconnect_failure_streak: u32,
    pub last_reconnect_reason: String,
    pub last_reconnect_generation: u64,
    pub last_reconnect_at_unix_ms: f64,
    pub last_retry_delay_ms: f64,
}

impl From<&ReceiverStats> for ReceiverProgress {
    fn from(stats: &ReceiverStats) -> Self {
        Self {
            progress_schema: RECEIVER_PROGRESS_SCHEMA,
            sample_time_ms: stats.stats_sample_time_ms,
            connection_generation: stats.connection_generation,
            active_ssrc: stats.active_ssrc,
            frames_received: stats.frames_received,
            packets_received: stats.packets_received,
            pli_count: stats.pli_count,
            fir_count: stats.fir_count,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct ControlApplyResult {
    pub accepted: bool,
    pub camera_input: bool,
    pub orbit_input: bool,
    pub zoom_input: bool,
    pub sequence_gap: u64,
}

/// RFC 1982-style serial comparison for the wrapping u32 camera epoch.
/// A value in the forward half of the sequence space is newer; a delayed
/// packet from an older epoch must never reset the camera after a reconnect.
pub(crate) fn epoch_is_newer(candidate: u32, current: u32) -> bool {
    let forward = candidate.wrapping_sub(current);
    forward != 0 && forward < (1_u32 << 31)
}

impl CameraController {
    pub fn new(fixed: FixedCamera, time: f32) -> Self {
        Self {
            near: fixed.near,
            far: fixed.far,
            initial_fixed: fixed,
            fixed: true,
            target: [0.0, 0.0, 3.0],
            yaw: 0.0,
            target_yaw: 0.0,
            pitch: 0.0,
            target_pitch: 0.0,
            distance: 5.5,
            target_distance: 5.5,
            roll: 0.0,
            time,
            playing: true,
            input_epoch: None,
            input_sequence: 0,
            input_orbit_x: 0.0,
            input_orbit_y: 0.0,
            input_zoom: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.reset_pose();
        self.input_epoch = None;
        self.input_sequence = 0;
        self.input_orbit_x = 0.0;
        self.input_orbit_y = 0.0;
        self.input_zoom = 0.0;
    }

    fn reset_pose(&mut self) {
        self.fixed = true;
        self.target = [0.0, 0.0, 3.0];
        self.yaw = 0.0;
        self.target_yaw = 0.0;
        self.pitch = 0.0;
        self.target_pitch = 0.0;
        self.distance = 5.5;
        self.target_distance = 5.5;
        self.roll = 0.0;
        self.near = self.initial_fixed.near;
        self.far = self.initial_fixed.far;
    }

    pub fn apply(
        &mut self,
        message: ControlMessage,
        width: u32,
        height: u32,
    ) -> ControlApplyResult {
        match message {
            ControlMessage::Orbit { dx, dy } => {
                if !dx.is_finite() || !dy.is_finite() {
                    return ControlApplyResult::default();
                }
                let (orbit_input, zoom_input) = self.apply_delta(dx, dy, 0.0, width, height);
                ControlApplyResult {
                    accepted: true,
                    camera_input: orbit_input || zoom_input,
                    orbit_input,
                    zoom_input,
                    ..Default::default()
                }
            }
            ControlMessage::Zoom { delta } => {
                if !delta.is_finite() {
                    return ControlApplyResult::default();
                }
                let (orbit_input, zoom_input) = self.apply_delta(0.0, 0.0, delta, width, height);
                ControlApplyResult {
                    accepted: true,
                    camera_input: orbit_input || zoom_input,
                    orbit_input,
                    zoom_input,
                    ..Default::default()
                }
            }
            ControlMessage::CameraState {
                epoch,
                sequence,
                client_time_ms,
                orbit_x,
                orbit_y,
                zoom,
            } => {
                if !client_time_ms.is_finite()
                    || !orbit_x.is_finite()
                    || !orbit_y.is_finite()
                    || !zoom.is_finite()
                    || orbit_x.abs() > 10_000_000.0
                    || orbit_y.abs() > 10_000_000.0
                    || zoom.abs() > 10_000_000.0
                {
                    return ControlApplyResult::default();
                }
                match self.input_epoch {
                    Some(current_epoch) if epoch == current_epoch => {
                        if sequence <= self.input_sequence {
                            return ControlApplyResult::default();
                        }
                    }
                    Some(current_epoch) if !epoch_is_newer(epoch, current_epoch) => {
                        return ControlApplyResult::default();
                    }
                    _ => {
                        self.reset_pose();
                        self.input_epoch = Some(epoch);
                        self.input_sequence = 0;
                        self.input_orbit_x = 0.0;
                        self.input_orbit_y = 0.0;
                        self.input_zoom = 0.0;
                    }
                }
                let sequence_gap = sequence.saturating_sub(self.input_sequence + 1);
                let dx = orbit_x - self.input_orbit_x;
                let dy = orbit_y - self.input_orbit_y;
                let zoom_delta = zoom - self.input_zoom;
                self.input_sequence = sequence;
                self.input_orbit_x = orbit_x;
                self.input_orbit_y = orbit_y;
                self.input_zoom = zoom;
                let (orbit_input, zoom_input) = if dx != 0.0 || dy != 0.0 || zoom_delta != 0.0 {
                    self.apply_delta(dx, dy, zoom_delta, width, height)
                } else {
                    (false, false)
                };
                let camera_input = orbit_input || zoom_input;
                ControlApplyResult {
                    accepted: true,
                    camera_input,
                    orbit_input,
                    zoom_input,
                    sequence_gap,
                }
            }
            ControlMessage::Reset => {
                self.reset();
                ControlApplyResult {
                    accepted: true,
                    ..Default::default()
                }
            }
            ControlMessage::SetTime { value } => {
                self.time = value.clamp(0.0, 1.0);
                self.playing = false;
                ControlApplyResult {
                    accepted: true,
                    ..Default::default()
                }
            }
            ControlMessage::SetPlaying { value } => {
                self.playing = value;
                ControlApplyResult {
                    accepted: true,
                    ..Default::default()
                }
            }
            ControlMessage::KeyframeRequest { .. }
            | ControlMessage::ReceiverStats { .. }
            | ControlMessage::ReceiverProgress { .. } => ControlApplyResult::default(),
        }
    }

    fn apply_delta(
        &mut self,
        dx: f32,
        dy: f32,
        zoom: f32,
        width: u32,
        height: u32,
    ) -> (bool, bool) {
        self.release_fixed(width, height);
        let previous_target_yaw = self.target_yaw;
        let previous_target_pitch = self.target_pitch;
        let previous_target_distance = self.target_distance;
        self.target_yaw = shortest_angle(self.target_yaw - dx * 0.006);
        self.target_pitch = (self.target_pitch + dy * 0.006).clamp(-1.45, 1.45);
        self.target_distance = (self.target_distance * (zoom * 0.001).exp()).clamp(0.2, 100.0);
        (
            self.target_yaw != previous_target_yaw || self.target_pitch != previous_target_pitch,
            self.target_distance != previous_target_distance,
        )
    }

    pub fn tick(&mut self, seconds: f32) {
        // Input events arrive in bursts, but frames are produced at a stable cadence.
        // Following a target in render time prevents sparse wheel packets from becoming
        // visible camera steps. This exponential form is frame-rate independent.
        // A delayed render frame must not turn into a single large camera jump on recovery.
        let camera_seconds = seconds.clamp(0.0, 0.05);
        let orbit_blend = 1.0 - (-ORBIT_FOLLOW_RATE_PER_SECOND * camera_seconds).exp();
        let zoom_blend = 1.0 - (-ZOOM_FOLLOW_RATE_PER_SECOND * camera_seconds).exp();
        self.yaw =
            shortest_angle(self.yaw + shortest_angle(self.target_yaw - self.yaw) * orbit_blend);
        self.pitch += (self.target_pitch - self.pitch) * orbit_blend;
        self.distance += (self.target_distance - self.distance) * zoom_blend;
        if self.orbit_error() < 1e-4 {
            self.yaw = self.target_yaw;
            self.pitch = self.target_pitch;
        }
        if self.zoom_relative_error() < 1e-4 {
            self.distance = self.target_distance;
        }
        if self.playing {
            self.time = (self.time + seconds * 0.14) % 1.0;
        }
    }

    pub fn is_settling(&self) -> bool {
        self.zoom_relative_error() >= 1e-4 || self.orbit_error() >= 1e-4
    }

    pub fn distance(&self) -> f32 {
        self.distance
    }

    pub fn target_distance(&self) -> f32 {
        self.target_distance
    }

    pub fn uniform(&self, width: u32, height: u32) -> CameraUniform {
        if self.fixed {
            return renderer::fixed_camera(&self.initial_fixed, width, height);
        }
        let cos_pitch = self.pitch.cos();
        let eye = [
            self.target[0] + self.distance * self.yaw.sin() * cos_pitch,
            self.target[1] + self.distance * self.pitch.sin(),
            self.target[2] - self.distance * self.yaw.cos() * cos_pitch,
        ];
        let forward = normalize(sub(self.target, eye));
        let base_right = normalize(cross([0.0, 1.0, 0.0], forward));
        let base_down = cross(forward, base_right);
        let right = add(
            scale(base_right, self.roll.cos()),
            scale(base_down, self.roll.sin()),
        );
        let down = add(
            scale(base_down, self.roll.cos()),
            scale(base_right, -self.roll.sin()),
        );
        let world_to_camera = [
            right[0],
            down[0],
            forward[0],
            0.0,
            right[1],
            down[1],
            forward[1],
            0.0,
            right[2],
            down[2],
            forward[2],
            0.0,
            -dot(right, eye),
            -dot(down, eye),
            -dot(forward, eye),
            1.0,
        ];
        // Orbiting changes only the pose. Keep the asset's calibrated lens,
        // aspect-fit scale and principal point so releasing the fixed camera
        // cannot introduce a projection step.
        let intrinsics = renderer::fixed_camera(&self.initial_fixed, width, height).intrinsics;
        CameraUniform {
            world_to_camera,
            intrinsics,
            near: self.near,
            far: self.far,
            eye,
        }
    }

    fn release_fixed(&mut self, width: u32, height: u32) {
        if !self.fixed {
            return;
        }
        let uniform = renderer::fixed_camera(&self.initial_fixed, width, height);
        let rows = self.initial_fixed.world_to_camera_row_major;
        let forward = normalize([rows[2][0], rows[2][1], rows[2][2]]);
        let fixed_right = normalize([rows[0][0], rows[0][1], rows[0][2]]);
        let base_right = normalize(cross([0.0, 1.0, 0.0], forward));
        let base_down = cross(forward, base_right);
        self.target = add(uniform.eye, scale(forward, self.distance));
        self.pitch = (-forward[1]).clamp(-1.0, 1.0).asin();
        self.target_pitch = self.pitch;
        self.yaw = (-forward[0]).atan2(forward[2]);
        self.target_yaw = self.yaw;
        self.roll = dot(fixed_right, base_down).atan2(dot(fixed_right, base_right));
        self.target_distance = self.distance;
        self.fixed = false;
    }

    fn zoom_relative_error(&self) -> f32 {
        (self.target_distance - self.distance).abs() / self.target_distance.max(1e-6)
    }

    fn orbit_error(&self) -> f32 {
        shortest_angle(self.target_yaw - self.yaw)
            .abs()
            .max((self.target_pitch - self.pitch).abs())
    }
}

fn shortest_angle(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(v: [f32; 3], f: f32) -> [f32; 3] {
    [v[0] * f, v[1] * f, v[2] * f]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt().max(1e-12);
    scale(v, 1.0 / length)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> FixedCamera {
        FixedCamera {
            world_to_camera_row_major: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            intrinsics: [500.0, 500.0, 320.0, 180.0],
            source_size: [640, 360],
            near: 0.01,
            far: 100.0,
        }
    }

    fn calibrated_fixed() -> FixedCamera {
        let diagonal = std::f32::consts::FRAC_1_SQRT_2;
        let roll_sin = 0.5_f32;
        let roll_cos = 0.5_f32 * 3.0_f32.sqrt();
        let forward = [-diagonal, 0.0, diagonal];
        let base_right = [diagonal, 0.0, diagonal];
        let base_down = [0.0, 1.0, 0.0];
        let right = add(scale(base_right, roll_cos), scale(base_down, roll_sin));
        let down = add(scale(base_down, roll_cos), scale(base_right, -roll_sin));
        let eye = [2.0, -1.0, -3.0];
        FixedCamera {
            world_to_camera_row_major: [
                [right[0], right[1], right[2], -dot(right, eye)],
                [down[0], down[1], down[2], -dot(down, eye)],
                [forward[0], forward[1], forward[2], -dot(forward, eye)],
                [0.0, 0.0, 0.0, 1.0],
            ],
            intrinsics: [731.25, 704.5, 301.75, 207.125],
            source_size: [731, 487],
            near: 0.025,
            far: 250.0,
        }
    }

    fn assert_uniform_close(before: &CameraUniform, after: &CameraUniform) {
        for (before, after) in before
            .world_to_camera
            .iter()
            .chain(before.intrinsics.iter())
            .chain(before.eye.iter())
            .chain([&before.near, &before.far])
            .zip(
                after
                    .world_to_camera
                    .iter()
                    .chain(after.intrinsics.iter())
                    .chain(after.eye.iter())
                    .chain([&after.near, &after.far]),
            )
        {
            assert!(
                (before - after).abs() < 1e-5,
                "camera jumped: {before} != {after}"
            );
        }
    }

    #[test]
    fn releasing_fixed_camera_has_no_jump() {
        let mut camera = CameraController::new(calibrated_fixed(), 0.5);
        let before = camera.uniform(960, 540);
        camera.apply(ControlMessage::Orbit { dx: 0.0, dy: 0.0 }, 960, 540);
        let after = camera.uniform(960, 540);
        assert_uniform_close(&before, &after);
    }

    #[test]
    fn first_orbit_input_changes_only_the_smoothed_target() {
        let mut camera = CameraController::new(calibrated_fixed(), 0.5);
        let before = camera.uniform(960, 540);
        let result = camera.apply(ControlMessage::Orbit { dx: 18.0, dy: -7.0 }, 960, 540);
        let before_tick = camera.uniform(960, 540);
        assert!(result.camera_input && result.orbit_input);
        assert!(camera.is_settling());
        assert_uniform_close(&before, &before_tick);

        camera.tick(1.0 / 60.0);
        let after_tick = camera.uniform(960, 540);
        assert!(
            before
                .world_to_camera
                .iter()
                .zip(after_tick.world_to_camera)
                .any(|(before, after)| (before - after).abs() > 1e-5)
        );
        assert!(
            after_tick
                .world_to_camera
                .iter()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn released_camera_preserves_calibration_after_viewport_change() {
        let fixed = calibrated_fixed();
        let mut camera = CameraController::new(fixed.clone(), 0.5);
        camera.apply(ControlMessage::Orbit { dx: 0.0, dy: 0.0 }, 960, 540);

        let expected = renderer::fixed_camera(&fixed, 800, 800).intrinsics;
        let actual = camera.uniform(800, 800).intrinsics;
        for (expected, actual) in expected.into_iter().zip(actual) {
            assert!((expected - actual).abs() < 1e-5);
        }
    }

    #[test]
    fn grab_drag_direction_and_pitch_clamp_are_stable() {
        let mut camera = CameraController::new(fixed(), 0.5);
        camera.apply(
            ControlMessage::Orbit {
                dx: 100.0,
                dy: 100.0,
            },
            640,
            360,
        );
        assert!(
            camera.target_yaw < camera.yaw,
            "right drag should orbit the camera left"
        );
        assert!(
            camera.target_pitch > camera.pitch,
            "down drag should move the camera upward"
        );
        camera.apply(
            ControlMessage::Orbit {
                dx: 0.0,
                dy: 100_000.0,
            },
            640,
            360,
        );
        assert_eq!(camera.target_pitch, 1.45);
        camera.apply(
            ControlMessage::Orbit {
                dx: 0.0,
                dy: -100_000.0,
            },
            640,
            360,
        );
        assert_eq!(camera.target_pitch, -1.45);
    }

    #[test]
    fn zoom_follows_a_target_without_an_input_step_or_overshoot() {
        let mut camera = CameraController::new(fixed(), 0.5);
        camera.apply(ControlMessage::Zoom { delta: -500.0 }, 640, 360);
        let start = camera.distance;
        let target = camera.target_distance;
        assert!(target < start);
        assert_eq!(camera.distance, start, "input must only update the target");
        assert!(camera.is_settling());

        let mut previous = start;
        for _ in 0..120 {
            camera.tick(1.0 / 60.0);
            assert!(camera.distance <= previous);
            assert!(camera.distance >= target);
            previous = camera.distance;
        }
        assert!(!camera.is_settling());
        assert!((camera.distance - target).abs() < 1e-5);
    }

    #[test]
    fn zoom_reaches_the_interaction_envelope_within_three_30_fps_frames() {
        let mut camera = CameraController::new(fixed(), 0.5);
        camera.apply(ControlMessage::Zoom { delta: -500.0 }, 640, 360);
        let initial_error = (camera.distance - camera.target_distance).abs();
        for _ in 0..3 {
            camera.tick(1.0 / 30.0);
        }
        let remaining_error = (camera.distance - camera.target_distance).abs();
        assert!(remaining_error <= initial_error * 0.03);
    }

    #[test]
    fn reset_cancels_pending_zoom() {
        let mut camera = CameraController::new(fixed(), 0.5);
        camera.apply(ControlMessage::Zoom { delta: 500.0 }, 640, 360);
        assert!(camera.is_settling());
        camera.reset();
        assert!(!camera.is_settling());
        assert_eq!(camera.distance, 5.5);
    }

    #[test]
    fn cumulative_camera_state_recovers_lost_packets_and_rejects_old_ones() {
        let mut camera = CameraController::new(fixed(), 0.5);
        camera.apply(
            ControlMessage::CameraState {
                epoch: 7,
                sequence: 1,
                client_time_ms: 10.0,
                orbit_x: 100.0,
                orbit_y: 20.0,
                zoom: -40.0,
            },
            640,
            360,
        );
        let recovered = camera.apply(
            ControlMessage::CameraState {
                epoch: 7,
                sequence: 3,
                client_time_ms: 30.0,
                orbit_x: 300.0,
                orbit_y: 60.0,
                zoom: -120.0,
            },
            640,
            360,
        );
        assert!(recovered.accepted && recovered.camera_input);
        assert!(recovered.orbit_input);
        assert!(recovered.zoom_input);
        assert_eq!(recovered.sequence_gap, 1);
        let target_yaw = camera.target_yaw;
        let target_pitch = camera.target_pitch;
        let target_distance = camera.target_distance;

        let stale = camera.apply(
            ControlMessage::CameraState {
                epoch: 7,
                sequence: 2,
                client_time_ms: 20.0,
                orbit_x: 200.0,
                orbit_y: 40.0,
                zoom: -80.0,
            },
            640,
            360,
        );
        assert!(!stale.accepted);
        assert_eq!(camera.target_yaw, target_yaw);
        assert_eq!(camera.target_pitch, target_pitch);
        assert_eq!(camera.target_distance, target_distance);
    }

    #[test]
    fn apply_result_attributes_orbit_and_zoom_inputs_independently() {
        let mut camera = CameraController::new(fixed(), 0.5);
        let legacy_orbit = camera.apply(ControlMessage::Orbit { dx: 1.0, dy: 0.0 }, 640, 360);
        assert!(legacy_orbit.accepted && legacy_orbit.camera_input);
        assert!(legacy_orbit.orbit_input);
        assert!(!legacy_orbit.zoom_input);
        let legacy_zoom = camera.apply(ControlMessage::Zoom { delta: 1.0 }, 640, 360);
        assert!(legacy_zoom.accepted && legacy_zoom.camera_input);
        assert!(!legacy_zoom.orbit_input);
        assert!(legacy_zoom.zoom_input);

        let initial = camera.apply(
            ControlMessage::CameraState {
                epoch: 9,
                sequence: 1,
                client_time_ms: 1.0,
                orbit_x: 0.0,
                orbit_y: 0.0,
                zoom: 0.0,
            },
            640,
            360,
        );
        assert!(initial.accepted);
        assert!(!initial.camera_input && !initial.orbit_input && !initial.zoom_input);
        let orbit = camera.apply(
            ControlMessage::CameraState {
                epoch: 9,
                sequence: 2,
                client_time_ms: 2.0,
                orbit_x: 4.0,
                orbit_y: -2.0,
                zoom: 0.0,
            },
            640,
            360,
        );
        assert!(orbit.camera_input && orbit.orbit_input && !orbit.zoom_input);
        let zoom = camera.apply(
            ControlMessage::CameraState {
                epoch: 9,
                sequence: 3,
                client_time_ms: 3.0,
                orbit_x: 4.0,
                orbit_y: -2.0,
                zoom: 5.0,
            },
            640,
            360,
        );
        assert!(zoom.camera_input && !zoom.orbit_input && zoom.zoom_input);
        let heartbeat = camera.apply(
            ControlMessage::CameraState {
                epoch: 9,
                sequence: 4,
                client_time_ms: 4.0,
                orbit_x: 4.0,
                orbit_y: -2.0,
                zoom: 5.0,
            },
            640,
            360,
        );
        assert!(heartbeat.accepted);
        assert!(!heartbeat.camera_input && !heartbeat.orbit_input && !heartbeat.zoom_input);
    }

    #[test]
    fn apply_result_counts_only_actual_target_pose_changes() {
        let mut camera = CameraController::new(fixed(), 0.5);

        let zero_orbit = camera.apply(ControlMessage::Orbit { dx: 0.0, dy: 0.0 }, 640, 360);
        assert!(zero_orbit.accepted);
        assert!(!zero_orbit.camera_input && !zero_orbit.orbit_input && !zero_orbit.zoom_input);
        let zero_zoom = camera.apply(ControlMessage::Zoom { delta: 0.0 }, 640, 360);
        assert!(zero_zoom.accepted);
        assert!(!zero_zoom.camera_input && !zero_zoom.orbit_input && !zero_zoom.zoom_input);

        let pitch_to_limit = camera.apply(
            ControlMessage::Orbit {
                dx: 0.0,
                dy: 100_000.0,
            },
            640,
            360,
        );
        assert!(pitch_to_limit.camera_input && pitch_to_limit.orbit_input);
        let pitch_against_limit = camera.apply(
            ControlMessage::Orbit {
                dx: 0.0,
                dy: 100_000.0,
            },
            640,
            360,
        );
        assert!(pitch_against_limit.accepted);
        assert!(
            !pitch_against_limit.camera_input
                && !pitch_against_limit.orbit_input
                && !pitch_against_limit.zoom_input
        );

        let zoom_to_limit = camera.apply(ControlMessage::Zoom { delta: 100_000.0 }, 640, 360);
        assert!(zoom_to_limit.camera_input && zoom_to_limit.zoom_input);
        let zoom_against_limit = camera.apply(ControlMessage::Zoom { delta: 100_000.0 }, 640, 360);
        assert!(zoom_against_limit.accepted);
        assert!(
            !zoom_against_limit.camera_input
                && !zoom_against_limit.orbit_input
                && !zoom_against_limit.zoom_input
        );
    }

    #[test]
    fn cumulative_state_reports_no_applied_update_when_clamps_hold_the_target() {
        let mut camera = CameraController::new(fixed(), 0.5);
        let to_limits = camera.apply(
            ControlMessage::CameraState {
                epoch: 13,
                sequence: 1,
                client_time_ms: 1.0,
                orbit_x: 0.0,
                orbit_y: 1_000_000.0,
                zoom: 1_000_000.0,
            },
            640,
            360,
        );
        assert!(to_limits.camera_input && to_limits.orbit_input && to_limits.zoom_input);

        let held_at_limits = camera.apply(
            ControlMessage::CameraState {
                epoch: 13,
                sequence: 2,
                client_time_ms: 2.0,
                orbit_x: 0.0,
                orbit_y: 2_000_000.0,
                zoom: 2_000_000.0,
            },
            640,
            360,
        );
        assert!(held_at_limits.accepted);
        assert!(
            !held_at_limits.camera_input
                && !held_at_limits.orbit_input
                && !held_at_limits.zoom_input
        );
    }

    #[test]
    fn a_new_camera_epoch_resets_pose_before_applying_state() {
        let mut camera = CameraController::new(fixed(), 0.5);
        camera.apply(ControlMessage::Zoom { delta: 500.0 }, 640, 360);
        assert!(camera.target_distance > 5.5);
        let result = camera.apply(
            ControlMessage::CameraState {
                epoch: 8,
                sequence: 1,
                client_time_ms: 40.0,
                orbit_x: 0.0,
                orbit_y: 0.0,
                zoom: 0.0,
            },
            640,
            360,
        );
        assert!(result.accepted);
        assert_eq!(camera.distance, 5.5);
        assert_eq!(camera.target_distance, 5.5);
        assert!(!camera.is_settling());
    }

    #[test]
    fn a_delayed_old_epoch_cannot_roll_back_a_new_camera_session() {
        let mut camera = CameraController::new(fixed(), 0.5);
        for (epoch, orbit_x) in [(7, 100.0), (8, 240.0)] {
            let applied = camera.apply(
                ControlMessage::CameraState {
                    epoch,
                    sequence: 1,
                    client_time_ms: 10.0,
                    orbit_x,
                    orbit_y: 0.0,
                    zoom: 0.0,
                },
                640,
                360,
            );
            assert!(applied.accepted);
        }
        let target_yaw = camera.target_yaw;
        let stale = camera.apply(
            ControlMessage::CameraState {
                epoch: 7,
                sequence: 99,
                client_time_ms: 30.0,
                orbit_x: 10_000.0,
                orbit_y: 0.0,
                zoom: 0.0,
            },
            640,
            360,
        );
        assert!(!stale.accepted);
        assert_eq!(camera.input_epoch, Some(8));
        assert_eq!(camera.target_yaw, target_yaw);

        assert!(epoch_is_newer(1, 0xffff_fffe));
        assert!(!epoch_is_newer(0xffff_fffe, 1));
    }

    #[test]
    fn receiver_progress_deserializes_the_minimal_recovery_contract() {
        let message: ControlMessage = serde_json::from_value(serde_json::json!({
            "type": "receiver-progress",
            "progress_schema": 1,
            "sample_time_ms": 42.5,
            "connection_generation": 17,
            "active_ssrc": 99,
            "frames_received": 3_010,
            "packets_received": 63_663,
            "pli_count": 2,
            "fir_count": 1,
        }))
        .unwrap();
        let ControlMessage::ReceiverProgress { progress } = message else {
            panic!("expected receiver progress");
        };
        assert_eq!(
            progress,
            ReceiverProgress {
                progress_schema: RECEIVER_PROGRESS_SCHEMA,
                sample_time_ms: 42.5,
                connection_generation: 17,
                active_ssrc: 99,
                frames_received: 3_010,
                packets_received: 63_663,
                pli_count: 2,
                fir_count: 1,
            }
        );

        let full = ReceiverStats {
            stats_sample_time_ms: progress.sample_time_ms,
            connection_generation: progress.connection_generation,
            active_ssrc: progress.active_ssrc,
            frames_received: progress.frames_received,
            packets_received: progress.packets_received,
            pli_count: progress.pli_count,
            fir_count: progress.fir_count,
            ..Default::default()
        };
        assert_eq!(ReceiverProgress::from(&full), progress);

        assert!(
            serde_json::from_value::<ControlMessage>(serde_json::json!({
                "type": "receiver-progress",
                "progress_schema": 1,
                "sample_time_ms": 43.0,
            }))
            .is_err(),
            "the compact recovery contract must not silently default missing counters"
        );
    }

    #[test]
    fn control_protocol_rejects_unknown_fields() {
        for message in [
            r#"{"type":"camera-state","epoch":1,"sequence":1,"client_time_ms":42.0,"orbit_x":0.0,"orbit_y":0.0,"zoom":0.0,"unexpected":true}"#,
            r#"{"type":"receiver-progress","progress_schema":1,"sample_time_ms":42.0,"connection_generation":1,"active_ssrc":2,"frames_received":3,"packets_received":4,"pli_count":0,"fir_count":0,"unexpected":true}"#,
            r#"{"type":"receiver-stats","telemetry_schema":7,"unexpected":true}"#,
        ] {
            assert!(
                serde_json::from_str::<ControlMessage>(message).is_err(),
                "unknown protocol field was accepted: {message}"
            );
        }
    }

    #[test]
    fn receiver_schema_seven_deserializes_nullable_capability_telemetry() {
        let message: ControlMessage = serde_json::from_value(serde_json::json!({
            "type": "receiver-stats",
            "telemetry_schema": 7,
            "stats_sample_time_ms": 42.5,
            "packets_lost": -3,
            "decoder_implementation_supported": true,
            "decoder_implementation": "VideoToolbox",
            "power_efficient_decoder_supported": true,
            "power_efficient_decoder": true,
            "selected_candidate_pair_id": "CPwF7nYp",
            "selected_candidate_pair_available_incoming_bitrate_bps": 100_000_000.0,
            "selected_candidate_pair_available_outgoing_bitrate_bps": 80_000_000.0,
            "selected_candidate_pair_bytes_sent": 4_294_967_297_u64,
            "selected_candidate_pair_bytes_received": 8_589_934_594_u64,
            "local_candidate_type": "host",
            "local_candidate_protocol": "udp",
            "local_candidate_address": "192.0.2.7",
            "local_candidate_port": 51_234,
            "remote_candidate_type": "host",
            "remote_candidate_protocol": "udp",
            "remote_candidate_address": "198.51.100.128",
            "remote_candidate_port": 41_900,
            "control_buffered_amount_max_bytes": 2_048,
            "control_backpressure_skip_count": 3,
            "receiver_jitter_buffer_target_readback_ms": 35.0,
            "receiver_playout_delay_hint_readback_ms": 35.0,
            "connection_generation": 17,
            "page_visibility_transition_count": 2,
            "page_focus_transition_count": 4,
            "active_ssrc": 4_294_967_295_u32,
            "viewport_width": 1_280,
            "viewport_height": 720,
            "device_pixel_ratio": 2.0,
            "video_client_width": 1_280,
            "video_client_height": 720,
            "retransmitted_packets_received": 9,
            "retransmitted_bytes_received": 12_345,
            "media_recovery_state": "healthy",
            "media_keyframe_requests_sent": 2,
            "reconnect_count": 1,
            "last_reconnect_reason": "media-stall-timeout",
        }))
        .unwrap();
        let ControlMessage::ReceiverStats { stats } = message else {
            panic!("expected receiver stats");
        };
        assert_eq!(stats.telemetry_schema, RECEIVER_TELEMETRY_SCHEMA);
        assert_eq!(stats.stats_sample_time_ms, 42.5);
        assert_eq!(stats.packets_lost, -3);
        assert!(stats.decoder_implementation_supported);
        assert_eq!(
            stats.decoder_implementation.as_deref(),
            Some("VideoToolbox")
        );
        assert!(stats.power_efficient_decoder_supported);
        assert_eq!(stats.power_efficient_decoder, Some(true));
        assert_eq!(stats.selected_candidate_pair_id, "CPwF7nYp");
        assert_eq!(
            stats.selected_candidate_pair_available_incoming_bitrate_bps,
            100_000_000.0
        );
        assert_eq!(
            stats.selected_candidate_pair_available_outgoing_bitrate_bps,
            80_000_000.0
        );
        assert_eq!(stats.selected_candidate_pair_bytes_sent, 4_294_967_297);
        assert_eq!(stats.selected_candidate_pair_bytes_received, 8_589_934_594);
        assert_eq!(stats.local_candidate_type, "host");
        assert_eq!(stats.local_candidate_protocol, "udp");
        assert_eq!(stats.local_candidate_address, "192.0.2.7");
        assert_eq!(stats.local_candidate_port, 51_234);
        assert_eq!(stats.remote_candidate_type, "host");
        assert_eq!(stats.remote_candidate_protocol, "udp");
        assert_eq!(stats.remote_candidate_address, "198.51.100.128");
        assert_eq!(stats.remote_candidate_port, 41_900);
        assert_eq!(stats.control_buffered_amount_max_bytes, 2_048);
        assert_eq!(stats.control_backpressure_skip_count, 3);
        assert_eq!(stats.receiver_jitter_buffer_target_readback_ms, Some(35.0));
        assert_eq!(stats.receiver_playout_delay_hint_readback_ms, Some(35.0));
        assert_eq!(stats.connection_generation, 17);
        assert_eq!(stats.page_visibility_transition_count, 2);
        assert_eq!(stats.page_focus_transition_count, 4);
        assert_eq!(stats.active_ssrc, u32::MAX);
        assert_eq!(stats.viewport_width, 1_280);
        assert_eq!(stats.viewport_height, 720);
        assert_eq!(stats.device_pixel_ratio, 2.0);
        assert_eq!(stats.video_client_width, 1_280);
        assert_eq!(stats.video_client_height, 720);
        assert_eq!(stats.retransmitted_packets_received, 9);
        assert_eq!(stats.retransmitted_bytes_received, 12_345);
        assert_eq!(stats.media_recovery_state, "healthy");
        assert_eq!(stats.media_keyframe_requests_sent, 2);
        assert_eq!(stats.reconnect_count, 1);
        assert_eq!(stats.last_reconnect_reason, "media-stall-timeout");
    }

    #[test]
    fn receiver_schema_seven_preserves_nulls_support_flags_and_interval_jitter() {
        let message: ControlMessage = serde_json::from_value(serde_json::json!({
            "type": "receiver-stats",
            "telemetry_schema": 7,
            "preview_owner_id": "preview-owner_123",
            "frames_dropped_supported": false,
            "frames_dropped": null,
            "frames_rendered_supported": true,
            "frames_rendered": 38,
            "decoder_implementation_supported": false,
            "decoder_implementation": null,
            "power_efficient_decoder_supported": true,
            "power_efficient_decoder": false,
            "jitter_buffer_delay_interval_ms": 12.5,
            "jitter_buffer_target_delay_interval_ms": 35.0,
            "jitter_buffer_minimum_delay_interval_ms": 4.25,
            "receiver_jitter_buffer_target_mode": "browser",
            "receiver_jitter_buffer_target_ms": null,
            "receiver_jitter_buffer_target_api": "browser-default",
            "control_drag_input_messages_sent": 12,
            "control_wheel_input_messages_sent": 7,
        }))
        .unwrap();
        let ControlMessage::ReceiverStats { stats } = message else {
            panic!("expected receiver stats");
        };
        assert_eq!(stats.telemetry_schema, RECEIVER_TELEMETRY_SCHEMA);
        assert_eq!(stats.preview_owner_id, "preview-owner_123");
        assert!(!stats.frames_dropped_supported);
        assert_eq!(stats.frames_dropped, None);
        assert!(stats.frames_rendered_supported);
        assert_eq!(stats.frames_rendered, Some(38));
        assert!(!stats.decoder_implementation_supported);
        assert_eq!(stats.decoder_implementation, None);
        assert!(stats.power_efficient_decoder_supported);
        assert_eq!(stats.power_efficient_decoder, Some(false));
        assert_eq!(stats.jitter_buffer_delay_interval_ms, 12.5);
        assert_eq!(stats.jitter_buffer_target_delay_interval_ms, 35.0);
        assert_eq!(stats.jitter_buffer_minimum_delay_interval_ms, 4.25);
        assert_eq!(stats.receiver_jitter_buffer_target_mode, "browser");
        assert_eq!(stats.receiver_jitter_buffer_target_ms, None);
        assert_eq!(stats.receiver_jitter_buffer_target_api, "browser-default");
        assert_eq!(stats.control_drag_input_messages_sent, 12);
        assert_eq!(stats.control_wheel_input_messages_sent, 7);
    }

    #[test]
    fn keyframe_request_deserializes_as_priority_control() {
        let message: ControlMessage = serde_json::from_str(
            r#"{"type":"keyframe-request","connection_generation":7,"active_ssrc":99,"request_id":3,"last_frames_received":3010,"client_time_ms":42.0,"reason":"frame-stall"}"#,
        )
        .unwrap();
        let ControlMessage::KeyframeRequest {
            connection_generation,
            active_ssrc,
            request_id,
            last_frames_received,
            reason,
            ..
        } = message
        else {
            panic!("expected keyframe request");
        };
        assert_eq!(connection_generation, 7);
        assert_eq!(active_ssrc, 99);
        assert_eq!(request_id, 3);
        assert_eq!(last_frames_received, 3010);
        assert_eq!(reason, "frame-stall");
    }

    #[test]
    fn receiver_schema_four_defaults_missing_attribution_telemetry() {
        let message: ControlMessage = serde_json::from_str(
            r#"{"type":"receiver-stats","telemetry_schema":4,"packets_lost":-1}"#,
        )
        .unwrap();
        let ControlMessage::ReceiverStats { stats } = message else {
            panic!("expected receiver stats");
        };
        assert_eq!(stats.telemetry_schema, 4);
        assert_eq!(stats.packets_lost, -1);
        assert!(stats.preview_owner_id.is_empty());
        assert!(stats.selected_candidate_pair_id.is_empty());
        assert_eq!(
            stats.selected_candidate_pair_available_incoming_bitrate_bps,
            0.0
        );
        assert_eq!(
            stats.selected_candidate_pair_available_outgoing_bitrate_bps,
            0.0
        );
        assert_eq!(stats.selected_candidate_pair_bytes_sent, 0);
        assert_eq!(stats.selected_candidate_pair_bytes_received, 0);
        assert!(stats.local_candidate_type.is_empty());
        assert!(stats.local_candidate_protocol.is_empty());
        assert!(stats.local_candidate_address.is_empty());
        assert_eq!(stats.local_candidate_port, 0);
        assert!(stats.remote_candidate_type.is_empty());
        assert!(stats.remote_candidate_protocol.is_empty());
        assert!(stats.remote_candidate_address.is_empty());
        assert_eq!(stats.remote_candidate_port, 0);
        assert_eq!(stats.control_buffered_amount_max_bytes, 0);
        assert_eq!(stats.control_backpressure_skip_count, 0);
        assert_eq!(stats.control_drag_input_messages_sent, 0);
        assert_eq!(stats.control_wheel_input_messages_sent, 0);
        assert!(!stats.frames_dropped_supported);
        assert_eq!(stats.frames_dropped, None);
        assert!(!stats.frames_rendered_supported);
        assert_eq!(stats.frames_rendered, None);
        assert!(!stats.decoder_implementation_supported);
        assert_eq!(stats.decoder_implementation, None);
        assert!(!stats.power_efficient_decoder_supported);
        assert_eq!(stats.power_efficient_decoder, None);
        assert!(stats.receiver_jitter_buffer_target_mode.is_empty());
        assert_eq!(stats.receiver_jitter_buffer_target_ms, None);
        assert_eq!(stats.receiver_jitter_buffer_target_readback_ms, None);
        assert_eq!(stats.receiver_playout_delay_hint_readback_ms, None);
        assert_eq!(stats.connection_generation, 0);
        assert_eq!(stats.page_visibility_transition_count, 0);
        assert_eq!(stats.page_focus_transition_count, 0);
        assert_eq!(stats.viewport_width, 0);
        assert_eq!(stats.viewport_height, 0);
        assert_eq!(stats.device_pixel_ratio, 0.0);
        assert_eq!(stats.video_client_width, 0);
        assert_eq!(stats.video_client_height, 0);
    }
}
