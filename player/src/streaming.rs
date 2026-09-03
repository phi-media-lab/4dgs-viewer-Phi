use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    io::{Read, Write},
    net::{IpAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_allocators::{DmaBufAllocator, DmaBufAllocatorExtManual};
use gstreamer_app::AppSrc;
use gstreamer_sdp::SDPMessage;
use gstreamer_video::{
    ForceKeyUnitEvent, UpstreamForceKeyUnitEvent, VideoFormat, VideoFrameFlags, VideoMeta,
};
use gstreamer_webrtc::{
    WebRTCDataChannel, WebRTCICEConnectionState, WebRTCICEGatheringState,
    WebRTCPeerConnectionState, WebRTCPriorityType, WebRTCRTPTransceiver, WebRTCSDPType,
    WebRTCSessionDescription,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::camera_control::{ControlMessage, ReceiverProgress, ReceiverStats};
use crate::external_image::{DmabufLayout, ExternalImage};

const INDEX_HTML: &str = include_str!("../web/index.html");
const CLIENT_JS: &str = include_str!("../web/client.js");
const CLIENT_PROTOCOL: u32 = 2;
// These defaults stay below the stock Linux 212,992-byte UDP send-buffer burst
// limit. A higher bitrate remains available through the PHI_VIDEO_* overrides
// only after the operator has explicitly provisioned larger kernel buffers.
const DEFAULT_VIDEO_BITRATE_KBPS: u32 = 6_000;
// Vulkan exports DRM AR24. On this little-endian host that means B,G,R,A
// bytes, but the pixels are full-range RGB values. Keep the source and the
// video-domain conversion explicit so the VA postprocessor never infers
// BT.601 from resolution and the RTP colorspace extension can describe the
// actual result.
const SOURCE_COLORIMETRY: &str = "sRGB";
const VIDEO_COLORIMETRY: &str = "bt709";
// gstreamer-vaapi 1.24 fixates BGRA -> NV12 to centered chroma on radeonsi.
// This value must match the actual VASurface and the RTP colorspace extension.
const VIDEO_CHROMA_SITE: &str = "jpeg";
// `4` selects the balanced speed/quality point in the target VA-API profile.
const VIDEO_TARGET_USAGE: u32 = 4;
// This is the maximum steady-state GOP; RTCP/client recovery may request an
// earlier IDR. Treat the value as an experimental profile, not a universal
// network optimum.
const DEFAULT_KEYFRAME_INTERVAL_FRAMES: u32 = 600;
const DEFAULT_VIDEO_SLICES: u32 = 4;
// Leave socket priority to the WebRTC/network stack on the correctness path.
// Non-default DSCP values are an explicit, environment-specific LAN experiment
// selected through PHI_WEBRTC_VIDEO_PRIORITY and must be verified on the wire.
const DEFAULT_WEBRTC_VIDEO_PRIORITY: &str = "inherit";
const H264_RTP_PAYLOAD_TYPE: u32 = 108;
// The sleep-based nicesink pad probe is not a congestion controller. It remains
// available only as an explicit LAN experiment and is never installed by
// default. Its direct 100 Mbit/s default avoids a derived percentage hidden
// behind equal clamp bounds.
const MIN_NICE_PACER_BPS: u64 = 80_000_000;
const DEFAULT_NICE_PACER_BPS: u64 = 100_000_000;
const MAX_NICE_PACER_BPS: u64 = 100_000_000;
// Limit the unpaced burst independently of average bitrate so a whole encoded
// frame cannot enter the socket queue as one train.
const DEFAULT_NICE_PACER_BURST_BYTES: u32 = 8 * 1024;
const CONSERVATIVE_IP_UDP_OVERHEAD_BYTES: u64 = 48;
const RECOMMENDED_UDP_WMEM_BYTES: u64 = 4 * 1024 * 1024;
// Keep retransmission history bounded by both packet count and time. These are
// experimental single-peer defaults and must be measured for another network.
const RTX_HISTORY_PACKETS: u32 = 2_048;
const RTX_HISTORY_MS: u32 = 2_000;
const MAX_APPSRC_BUFFERS: u64 = 1;
const CLIENT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
// Explicit DataChannel close/failed paths remain immediate; the lease timeout
// allows a short receiver scheduling pause without rebuilding the pipeline.
const CLIENT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(8);
// ReceiverStats is the largest current payload. Bound every string before JSON
// parsing so an SCTP peer cannot force unbounded parser work on the application
// thread. The embedded browser's current diagnostics payload is far smaller.
const MAX_INBOUND_DATA_CHANNEL_STRING_BYTES: usize = 64 * 1024;
pub fn client_build() -> String {
    let mut digest = Sha256::new();
    digest.update(INDEX_HTML.as_bytes());
    digest.update([0]);
    digest.update(CLIENT_JS.as_bytes());
    format!("{:x}", digest.finalize())[..12].to_string()
}

#[derive(Debug)]
struct PendingFrame {
    pushed_at: Instant,
}

#[derive(Debug)]
pub struct Completion {
    pub encode_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyframeRecovery {
    requests: u64,
    feedback_coverage: u64,
    rtcp_feedback_requests: u64,
    feedback_fallback_requests: u64,
    other_manual_requests: u64,
    latency: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyframeRequestSource {
    RtcpFeedback,
    FeedbackFallback { coverage: u64 },
    OtherManual,
}

#[derive(Debug, Default)]
struct KeyframeRecoveryTracker {
    oldest_pending_request: Option<Instant>,
    pending_rtcp_feedback_requests: u64,
    pending_feedback_fallback_requests: u64,
    pending_feedback_fallback_coverage: u64,
    pending_other_manual_requests: u64,
}

impl KeyframeRecoveryTracker {
    fn note_request(&mut self, now: Instant, source: KeyframeRequestSource) -> u64 {
        self.oldest_pending_request.get_or_insert(now);
        match source {
            KeyframeRequestSource::RtcpFeedback => {
                self.pending_rtcp_feedback_requests =
                    self.pending_rtcp_feedback_requests.saturating_add(1);
            }
            KeyframeRequestSource::FeedbackFallback { coverage } => {
                self.pending_feedback_fallback_requests =
                    self.pending_feedback_fallback_requests.saturating_add(1);
                self.pending_feedback_fallback_coverage =
                    self.pending_feedback_fallback_coverage.max(coverage);
            }
            KeyframeRequestSource::OtherManual => {
                self.pending_other_manual_requests =
                    self.pending_other_manual_requests.saturating_add(1);
            }
        }
        self.pending_requests()
    }

    fn note_keyframe(&mut self, now: Instant) -> Option<KeyframeRecovery> {
        let requested_at = self.oldest_pending_request.take()?;
        let rtcp_feedback_requests = std::mem::take(&mut self.pending_rtcp_feedback_requests);
        let feedback_fallback_requests =
            std::mem::take(&mut self.pending_feedback_fallback_requests);
        let feedback_fallback_coverage =
            std::mem::take(&mut self.pending_feedback_fallback_coverage);
        let other_manual_requests = std::mem::take(&mut self.pending_other_manual_requests);
        let feedback_coverage = rtcp_feedback_requests.max(feedback_fallback_coverage);
        let requests = rtcp_feedback_requests
            .saturating_add(feedback_fallback_requests)
            .saturating_add(other_manual_requests);
        Some(KeyframeRecovery {
            requests,
            feedback_coverage,
            rtcp_feedback_requests,
            feedback_fallback_requests,
            other_manual_requests,
            // Multiple PLIs can be coalesced into one IDR. Measure from the
            // oldest outstanding request so retries cannot make recovery look
            // artificially faster.
            latency: now.saturating_duration_since(requested_at),
        })
    }

    fn pending_requests(&self) -> u64 {
        self.pending_rtcp_feedback_requests
            .saturating_add(self.pending_feedback_fallback_requests)
            .saturating_add(self.pending_other_manual_requests)
    }
}

fn encoded_access_unit_is_keyframe(flags: gst::BufferFlags) -> bool {
    // The pipeline fixes h264parse to AU alignment, so a non-delta output
    // buffer is one independently decodable H.264 access unit.
    !flags.contains(gst::BufferFlags::DELTA_UNIT)
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaConfig {
    pub bitrate_kbps: u32,
    pub keyframe_interval_frames: u32,
    pub video_slices: u32,
    pub cpb_size_kbits: u32,
    pub webrtc_video_priority: String,
    pub experimental_lan_pacer: bool,
    pub nice_max_bitrate_bps: u64,
    pub nice_pacer_burst_bytes: u32,
}

impl MediaConfig {
    fn from_env() -> Result<Self> {
        let bitrate_kbps = env_u32(
            "PHI_VIDEO_BITRATE_KBPS",
            DEFAULT_VIDEO_BITRATE_KBPS,
            1_000,
            50_000,
        )?;
        let keyframe_interval_frames = env_u32(
            "PHI_VIDEO_KEYFRAME_FRAMES",
            DEFAULT_KEYFRAME_INTERVAL_FRAMES,
            1,
            600,
        )?;
        let video_slices = env_u32("PHI_VIDEO_SLICES", DEFAULT_VIDEO_SLICES, 1, 200)?;
        // Keep a 0.5 s CPB floor. The public override stays in kbits while the
        // selected vaapih264enc property is derived in milliseconds.
        let minimum_cpb_size_kbits = minimum_cpb_size_kbits(bitrate_kbps);
        let cpb_default = minimum_cpb_size_kbits;
        let cpb_size_kbits = env_u32(
            "PHI_VIDEO_CPB_KBITS",
            cpb_default,
            minimum_cpb_size_kbits,
            100_000,
        )?;
        let encoder_cpb_length_ms = cpb_length_ms(cpb_size_kbits, bitrate_kbps);
        ensure!(
            (1..=10_000).contains(&encoder_cpb_length_ms),
            "PHI_VIDEO_CPB_KBITS maps to vaapih264enc cpb-length={encoder_cpb_length_ms} ms, expected 1..=10000"
        );
        let experimental_lan_pacer = env_flag("PHI_EXPERIMENTAL_LAN_PACER", false)?;
        let nice_max_bitrate_bps = u64::from(env_u32(
            "PHI_WEBRTC_PACER_BITRATE_BPS",
            DEFAULT_NICE_PACER_BPS as u32,
            MIN_NICE_PACER_BPS as u32,
            MAX_NICE_PACER_BPS as u32,
        )?);
        let nice_pacer_burst_bytes = env_u32(
            "PHI_WEBRTC_PACER_BURST_BYTES",
            DEFAULT_NICE_PACER_BURST_BYTES,
            1_500,
            131_072,
        )?;
        let webrtc_video_priority = env::var_os("PHI_WEBRTC_VIDEO_PRIORITY")
            .map(|value| {
                value
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("PHI_WEBRTC_VIDEO_PRIORITY is not valid UTF-8"))
            })
            .transpose()?
            .unwrap_or_else(|| DEFAULT_WEBRTC_VIDEO_PRIORITY.to_owned());
        parse_webrtc_video_priority(&webrtc_video_priority)?;
        Ok(Self {
            bitrate_kbps,
            keyframe_interval_frames,
            video_slices,
            cpb_size_kbits,
            webrtc_video_priority,
            experimental_lan_pacer,
            nice_max_bitrate_bps,
            nice_pacer_burst_bytes,
        })
    }
}

fn minimum_cpb_size_kbits(bitrate_kbps: u32) -> u32 {
    bitrate_kbps.div_ceil(2)
}

fn cpb_length_ms(cpb_size_kbits: u32, bitrate_kbps: u32) -> u32 {
    debug_assert!(bitrate_kbps > 0);
    cpb_size_kbits.saturating_mul(1_000).div_ceil(bitrate_kbps)
}

fn parse_webrtc_video_priority(value: &str) -> Result<Option<WebRTCPriorityType>> {
    match value {
        "inherit" => Ok(None),
        "very-low" => Ok(Some(WebRTCPriorityType::VeryLow)),
        "low" => Ok(Some(WebRTCPriorityType::Low)),
        "medium" => Ok(Some(WebRTCPriorityType::Medium)),
        "high" => Ok(Some(WebRTCPriorityType::High)),
        _ => bail!(
            "PHI_WEBRTC_VIDEO_PRIORITY must be one of inherit, very-low, low, medium, high; got {value:?}"
        ),
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TransportMetrics {
    pub configured_bitrate_kbps: u32,
    pub keyframe_interval_frames: u32,
    pub video_slices: u32,
    pub cpb_size_kbits: u32,
    pub webrtc_video_priority: String,
    pub system_udp_wmem_default_bytes: u64,
    pub system_udp_wmem_max_bytes: u64,
    pub recommended_udp_wmem_bytes: u64,
    pub gstreamer_bus_warnings: u64,
    pub gstreamer_bus_errors: u64,
    pub gstreamer_latency_messages: u64,
    pub gstreamer_latency_recalc_coalesced: u64,
    pub gstreamer_latency_recalc_schedule_failures: u64,
    pub gstreamer_latency_recalc_successes: u64,
    pub gstreamer_latency_recalc_failures: u64,
    pub gstreamer_latency_recalc_last_ms: f64,
    pub gstreamer_latency_recalc_max_ms: f64,
    pub experimental_lan_pacer: bool,
    pub nice_pacer_configured: bool,
    pub nice_max_bitrate_bps: u64,
    pub nice_pacer_burst_bytes: u32,
    pub nice_pacer_packets: u64,
    pub nice_pacer_buffer_lists: u64,
    pub nice_pacer_max_batch_packets: u64,
    pub nice_pacer_max_batch_wire_bytes: u64,
    pub nice_pacer_oversize_batches: u64,
    // Compatibility aliases for nice_pacer_requested_wait_{ms,max_ms}.
    // These historically measured the requested sleep, not elapsed wall time.
    pub nice_pacer_wait_ms: f64,
    pub nice_pacer_wait_max_ms: f64,
    pub nice_pacer_requested_wait_ms: f64,
    pub nice_pacer_requested_wait_max_ms: f64,
    pub nice_pacer_requested_wait_total_ms: f64,
    pub nice_pacer_actual_wait_ms: f64,
    pub nice_pacer_actual_wait_max_ms: f64,
    pub nice_pacer_actual_wait_total_ms: f64,
    pub nice_pacer_sleep_overshoot_ms: f64,
    pub nice_pacer_sleep_overshoot_max_ms: f64,
    pub nice_pacer_sleep_overshoot_total_ms: f64,
    pub nice_pacer_sleep_count: u64,
    pub free_slots: u64,
    pub pending_frames: u64,
    pub appsrc_queued_buffers: u64,
    pub appsrc_queued_bytes: u64,
    pub appsrc_queued_time_ns: u64,
    pub slot_wait_ms: f64,
    pub slot_wait_max_ms: f64,
    pub push_ms: f64,
    pub push_ms_max: f64,
    pub pushed_frames: u64,
    pub appsrc_dropped_frames: u64,
    pub encoded_frames: u64,
    pub encoded_keyframes: u64,
    pub encoded_au_bytes: u64,
    pub encoded_au_bytes_max: u64,
    pub last_keyframe_pts_ns: Option<u64>,
    pub primary_ssrc: u32,
    pub nack_retransmission_enabled: bool,
    pub negotiated_h264_generic_nack: bool,
    pub negotiated_h264_rtx: bool,
    pub negotiated_h264_rtx_payload_type: Option<u32>,
    pub negotiated_h264_primary_ssrc: Option<u32>,
    pub negotiated_h264_rtx_ssrc: Option<u32>,
    pub rtx_sender_present: bool,
    pub rtx_history_packets: u32,
    pub rtx_history_ms: u32,
    pub rtx_requests: u64,
    pub rtx_packets: u64,
    pub force_key_unit_requests: u64,
    pub active_force_key_unit_requests: u64,
    pub force_key_unit_requests_at_encoder: u64,
    pub force_key_unit_recoveries: u64,
    pub force_key_unit_requests_recovered: u64,
    pub feedback_force_key_unit_requests_recovered: u64,
    pub pending_force_key_unit_requests: u64,
    pub last_force_key_unit_request_running_time_ns: Option<u64>,
    pub last_force_key_unit_request_to_keyframe_ms: Option<f64>,
    pub max_force_key_unit_request_to_keyframe_ms: f64,
    pub push_errors: u64,
    pub last_pts_ns: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RecoveryMetrics {
    pub force_key_unit_requests: u64,
    pub feedback_force_key_unit_requests_recovered: u64,
    pub pending_force_key_unit_requests: u64,
}

#[derive(Default)]
struct DataChannels {
    control: Option<WebRTCDataChannel>,
    config: Option<WebRTCDataChannel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataChannelRole {
    Control,
    Config,
}

impl DataChannelRole {
    fn from_label(label: Option<&str>) -> Option<Self> {
        match label {
            Some("control") => Some(Self::Control),
            Some("config") => Some(Self::Config),
            _ => None,
        }
    }

    fn accepts(self, message: &ControlMessage) -> bool {
        match self {
            // The browser normally sends camera-state here. Progress/stats are
            // allowed as the existing fallback while the reliable channel is
            // not yet open. Orbit/zoom remain compatible with the v0.1 client.
            Self::Control => matches!(
                message,
                ControlMessage::Orbit { .. }
                    | ControlMessage::Zoom { .. }
                    | ControlMessage::CameraState { .. }
                    | ControlMessage::ReceiverStats { .. }
                    | ControlMessage::ReceiverProgress { .. }
            ),
            // Reliable camera tails, discrete configuration changes and
            // recovery/telemetry belong on the ordered channel.
            Self::Config => matches!(
                message,
                ControlMessage::CameraState { .. }
                    | ControlMessage::Reset
                    | ControlMessage::SetTime { .. }
                    | ControlMessage::SetPlaying { .. }
                    | ControlMessage::KeyframeRequest { .. }
                    | ControlMessage::ReceiverStats { .. }
                    | ControlMessage::ReceiverProgress { .. }
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncomingControlRoute {
    Oversized,
    Malformed,
    PriorityStored,
    WrongChannel,
    Queued,
    QueueFull,
    QueueDisconnected,
}

impl IncomingControlRoute {
    fn renews_lease(self) -> bool {
        // A full queue still proves that the recognized peer is active; the
        // message is counted as dropped below. Syntax/category failures and a
        // disconnected consumer cannot retain session ownership.
        matches!(self, Self::PriorityStored | Self::Queued | Self::QueueFull)
    }

    fn counts_as_drop(self) -> bool {
        matches!(
            self,
            Self::Oversized
                | Self::Malformed
                | Self::WrongChannel
                | Self::QueueFull
                | Self::QueueDisconnected
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LatencyRecalculationSchedule {
    Scheduled,
    Coalesced,
    Disconnected,
}

#[derive(Default)]
struct LatencyRecalculationCounters {
    messages: AtomicU64,
    coalesced: AtomicU64,
    schedule_failures: AtomicU64,
}

fn saturating_increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

fn schedule_latency_recalculation(sender: &mpsc::SyncSender<()>) -> LatencyRecalculationSchedule {
    match sender.try_send(()) {
        Ok(()) => LatencyRecalculationSchedule::Scheduled,
        Err(mpsc::TrySendError::Full(())) => LatencyRecalculationSchedule::Coalesced,
        Err(mpsc::TrySendError::Disconnected(())) => LatencyRecalculationSchedule::Disconnected,
    }
}

fn record_latency_message(
    counters: &LatencyRecalculationCounters,
    schedule: LatencyRecalculationSchedule,
) {
    saturating_increment(&counters.messages);
    match schedule {
        LatencyRecalculationSchedule::Scheduled => {}
        LatencyRecalculationSchedule::Coalesced => {
            saturating_increment(&counters.coalesced);
        }
        LatencyRecalculationSchedule::Disconnected => {
            saturating_increment(&counters.schedule_failures);
        }
    }
}

fn overlay_latency_message_counters(
    metrics: &mut TransportMetrics,
    counters: &LatencyRecalculationCounters,
) {
    metrics.gstreamer_latency_messages = counters.messages.load(Ordering::Relaxed);
    metrics.gstreamer_latency_recalc_coalesced = counters.coalesced.load(Ordering::Relaxed);
    metrics.gstreamer_latency_recalc_schedule_failures =
        counters.schedule_failures.load(Ordering::Relaxed);
}

fn record_latency_recalculation(
    metrics: &mut TransportMetrics,
    elapsed: Duration,
    succeeded: bool,
) {
    let elapsed_ms = elapsed.as_secs_f64() * 1_000.0;
    metrics.gstreamer_latency_recalc_last_ms = elapsed_ms;
    metrics.gstreamer_latency_recalc_max_ms =
        metrics.gstreamer_latency_recalc_max_ms.max(elapsed_ms);
    if succeeded {
        metrics.gstreamer_latency_recalc_successes =
            metrics.gstreamer_latency_recalc_successes.saturating_add(1);
    } else {
        metrics.gstreamer_latency_recalc_failures =
            metrics.gstreamer_latency_recalc_failures.saturating_add(1);
    }
}

fn recalculate_pipeline_latency(
    pipeline: &gst::Pipeline,
    metrics: &Arc<Mutex<TransportMetrics>>,
    serialization: &Mutex<()>,
    reason: &'static str,
) {
    // The initial application-thread recalculation can overlap a LATENCY
    // message emitted during PLAYING. Serialize the GStreamer calls without
    // holding the metrics mutex. A recalculation may itself post a message;
    // the sync handler only uses atomics/try_send, so this lock cannot cycle.
    let _serialization = serialization
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let started = Instant::now();
    let result = pipeline.recalculate_latency();
    let elapsed = started.elapsed();
    if let Ok(mut metrics) = metrics.lock() {
        record_latency_recalculation(&mut metrics, elapsed, result.is_ok());
    }
    if let Err(error) = result {
        eprintln!("GStreamer latency recalculation failed ({reason}): {error}");
    }
}

fn route_incoming_control(
    role: DataChannelRole,
    message: &str,
    control_tx: &mpsc::SyncSender<String>,
    latest_keyframe_request: &Arc<Mutex<Option<ControlMessage>>>,
) -> IncomingControlRoute {
    if message.len() > MAX_INBOUND_DATA_CHANNEL_STRING_BYTES {
        return IncomingControlRoute::Oversized;
    }
    let parsed = match serde_json::from_str::<ControlMessage>(message) {
        Ok(parsed) => parsed,
        Err(_) => return IncomingControlRoute::Malformed,
    };
    if !role.accepts(&parsed) {
        return IncomingControlRoute::WrongChannel;
    }
    if matches!(parsed, ControlMessage::KeyframeRequest { .. }) {
        // The reliable, ordered config DataChannel makes this a monotonic
        // latest-value slot. Recovery therefore cannot wait behind camera or
        // telemetry traffic in either bounded control queue.
        *latest_keyframe_request.lock().unwrap() = Some(parsed);
        return IncomingControlRoute::PriorityStored;
    }
    match control_tx.try_send(message.to_owned()) {
        Ok(()) => IncomingControlRoute::Queued,
        Err(mpsc::TrySendError::Full(_)) => IncomingControlRoute::QueueFull,
        Err(mpsc::TrySendError::Disconnected(_)) => IncomingControlRoute::QueueDisconnected,
    }
}

fn account_incoming_control_route(
    route: IncomingControlRoute,
    now: Instant,
    status: &Mutex<LiveStatus>,
    lifecycle: &Mutex<SessionLifecycle>,
) {
    if route.renews_lease() {
        lifecycle.lock().unwrap().record_activity(now);
    }
    if route == IncomingControlRoute::PriorityStored {
        status.lock().unwrap().controls += 1;
    } else if route.counts_as_drop() {
        status.lock().unwrap().controls_dropped += 1;
    }
    if route == IncomingControlRoute::QueueDisconnected {
        // Rebuild instead of leaving media alive with a permanently dead
        // camera/config consumer.
        lifecycle.lock().unwrap().request_restart();
    }
}

struct PacketPacer {
    next_send: Instant,
    bitrate_bps: u64,
    burst_duration: Duration,
}

impl PacketPacer {
    fn new(bitrate_bps: u64, burst_bytes: u32) -> Self {
        Self::new_at(Instant::now(), bitrate_bps, burst_bytes)
    }

    fn new_at(now: Instant, bitrate_bps: u64, burst_bytes: u32) -> Self {
        let burst_duration = duration_for_wire_bytes(u64::from(burst_bytes), bitrate_bps);
        Self {
            next_send: now.checked_sub(burst_duration).unwrap_or(now),
            bitrate_bps,
            burst_duration,
        }
    }

    fn reserve(&mut self, wire_bytes: u64) -> Duration {
        self.reserve_at(Instant::now(), wire_bytes)
    }

    fn reserve_at(&mut self, now: Instant, wire_bytes: u64) -> Duration {
        let credit_floor = now.checked_sub(self.burst_duration).unwrap_or(now);
        if self.next_send < credit_floor {
            self.next_send = credit_floor;
        }
        let wait = self.next_send.saturating_duration_since(now);
        self.next_send += duration_for_wire_bytes(wire_bytes, self.bitrate_bps);
        wait
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PacerWaitSample {
    requested_wait: Duration,
    actual_wait: Duration,
    sleep_overshoot: Duration,
    slept: bool,
}

fn classify_pacer_wait(requested_wait: Duration, measured_wait: Duration) -> PacerWaitSample {
    // A zero reservation does not call sleep. Report exactly zero rather than
    // accidentally turning probe overhead into scheduler delay.
    let (actual_wait, slept) = if requested_wait.is_zero() {
        (Duration::ZERO, false)
    } else {
        (measured_wait, true)
    };
    PacerWaitSample {
        requested_wait,
        actual_wait,
        sleep_overshoot: actual_wait.saturating_sub(requested_wait),
        slept,
    }
}

fn execute_pacer_wait(requested_wait: Duration) -> PacerWaitSample {
    if requested_wait.is_zero() {
        return classify_pacer_wait(requested_wait, Duration::ZERO);
    }
    let started = Instant::now();
    thread::sleep(requested_wait);
    classify_pacer_wait(requested_wait, started.elapsed())
}

fn record_pacer_wait(metrics: &mut TransportMetrics, sample: PacerWaitSample) {
    let requested_ms = sample.requested_wait.as_secs_f64() * 1_000.0;
    let actual_ms = sample.actual_wait.as_secs_f64() * 1_000.0;
    let overshoot_ms = sample.sleep_overshoot.as_secs_f64() * 1_000.0;

    metrics.nice_pacer_requested_wait_ms = requested_ms;
    metrics.nice_pacer_requested_wait_max_ms =
        metrics.nice_pacer_requested_wait_max_ms.max(requested_ms);
    metrics.nice_pacer_requested_wait_total_ms += requested_ms;
    metrics.nice_pacer_actual_wait_ms = actual_ms;
    metrics.nice_pacer_actual_wait_max_ms = metrics.nice_pacer_actual_wait_max_ms.max(actual_ms);
    metrics.nice_pacer_actual_wait_total_ms += actual_ms;
    metrics.nice_pacer_sleep_overshoot_ms = overshoot_ms;
    metrics.nice_pacer_sleep_overshoot_max_ms =
        metrics.nice_pacer_sleep_overshoot_max_ms.max(overshoot_ms);
    metrics.nice_pacer_sleep_overshoot_total_ms += overshoot_ms;
    metrics.nice_pacer_sleep_count = metrics
        .nice_pacer_sleep_count
        .saturating_add(u64::from(sample.slept));

    // Preserve the old JSON contract while making its requested-wait
    // semantics explicit through the new canonical names above.
    metrics.nice_pacer_wait_ms = requested_ms;
    metrics.nice_pacer_wait_max_ms = metrics.nice_pacer_wait_max_ms.max(requested_ms);
}

fn duration_for_wire_bytes(wire_bytes: u64, bitrate_bps: u64) -> Duration {
    debug_assert!(bitrate_bps > 0);
    let nanos = u128::from(wire_bytes)
        .saturating_mul(8)
        .saturating_mul(1_000_000_000)
        / u128::from(bitrate_bps);
    Duration::from_nanos(nanos.min(u128::from(u64::MAX)) as u64)
}

struct FrameReleaseContext {
    slot: usize,
    pts_ns: u64,
    released_tx: mpsc::Sender<usize>,
    queued: Arc<Mutex<HashMap<u64, PendingFrame>>>,
    metrics: Arc<Mutex<TransportMetrics>>,
}

#[derive(Debug, Clone, Copy)]
struct CadenceEpoch {
    slot: u64,
    pts_ns: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveStatus {
    pub state: String,
    pub frames: u64,
    pub fps: f64,
    pub render_ms: f64,
    pub preprocess_ms: f64,
    pub sort_ms: f64,
    pub tile_bin_ms: f64,
    pub tile_render_ms: f64,
    pub splat_ms: f64,
    pub resolve_ms: f64,
    pub gpu_ms: f64,
    pub render_scale: f64,
    pub lod_alpha_min: f64,
    pub interaction_active: bool,
    pub lod_overload_events: u64,
    pub lod_recovery_events: u64,
    pub encode_ms: f64,
    pub active: u32,
    pub visible: u32,
    pub tile_overlaps: u32,
    pub tile_overflow: u32,
    pub max_tile_load: u32,
    pub early_terminated_pixels: u32,
    pub pixel_splat_tests: u32,
    pub budget_limited_pixels: u32,
    pub max_pixel_splat_tests: u32,
    pub max_budget_remaining_transmittance: f32,
    pub persistent_workload_flags: u32,
    pub camera_distance: f32,
    pub camera_target_distance: f32,
    pub camera_orbit_updates_applied: u64,
    pub camera_zoom_updates_applied: u64,
    pub time: f32,
    pub dropped: u64,
    pub deadline_misses: u64,
    pub skipped_frames: u64,
    pub schedule_lateness_ms: f64,
    pub loop_ms: f64,
    pub slot_wait_ms: f64,
    pub push_ms: f64,
    pub input_sequence_gaps: u64,
    pub input_age_ms: f64,
    pub receiver_stall_keyframe_requests: u64,
    pub receiver_stall_keyframe_recoveries: u64,
    pub receiver_stall_keyframe_request_errors: u64,
    pub receiver_client_keyframe_requests_received: u64,
    pub receiver_client_keyframe_requests_forced: u64,
    pub receiver_client_keyframe_requests_coalesced: u64,
    pub receiver_client_keyframe_requests_rejected: u64,
    pub receiver_progress_fallback_requests_forced: u64,
    pub transport_metrics: TransportMetrics,
    pub resolution: [u32; 2],
    pub transport: String,
    pub peer: String,
    pub ice: String,
    pub control: String,
    pub controls: u64,
    pub controls_dropped: u64,
    pub receiver_progress: ReceiverProgress,
    pub browser: ReceiverStats,
    pub client_build: String,
}

pub struct StreamTransport {
    pipeline: gst::Pipeline,
    _latency_recalculation_worker: thread::JoinHandle<()>,
    latency_recalculation_counters: Arc<LatencyRecalculationCounters>,
    appsrc: AppSrc,
    allocator: DmaBufAllocator,
    layout: DmabufLayout,
    width: u32,
    height: u32,
    fps: u32,
    queued: Arc<Mutex<HashMap<u64, PendingFrame>>>,
    encoding: Arc<Mutex<VecDeque<PendingFrame>>>,
    completed_rx: mpsc::Receiver<Completion>,
    released_tx: mpsc::Sender<usize>,
    released_rx: mpsc::Receiver<usize>,
    free: VecDeque<usize>,
    encoder_src: gst::Pad,
    keyframe_recovery: Arc<Mutex<KeyframeRecoveryTracker>>,
    metrics: Arc<Mutex<TransportMetrics>>,
    rtx_sender: Arc<Mutex<Option<gst::Element>>>,
    cadence_epoch: Option<CadenceEpoch>,
    last_cadence_slot: Option<u64>,
    last_pts_ns: u64,
    signaler: Arc<WebRtcSignaler>,
}

pub struct StreamTransportConfig<'a> {
    pub layout: &'a DmabufLayout,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub slots: usize,
}

pub struct WebRtcSignaler {
    webrtc: gst::Element,
    primary_ssrc: u32,
    lifecycle: Arc<Mutex<SessionLifecycle>>,
    status: Arc<Mutex<LiveStatus>>,
    metrics: Arc<Mutex<TransportMetrics>>,
}

#[derive(Debug, Clone, Copy)]
enum SessionPhase {
    Idle,
    Negotiating { deadline: Instant },
    Live { lease_deadline: Instant },
    Restarting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfferDisposition {
    Claimed,
    Busy,
    Restarting,
}

#[derive(Debug)]
enum OfferOutcome {
    Answer(String),
    Busy,
    Restarting,
}

#[derive(Debug)]
struct SessionLifecycle {
    phase: SessionPhase,
}

impl Default for SessionLifecycle {
    fn default() -> Self {
        Self {
            phase: SessionPhase::Idle,
        }
    }
}

impl SessionLifecycle {
    fn expire(&mut self, now: Instant) {
        let expired = match self.phase {
            SessionPhase::Negotiating { deadline } => now >= deadline,
            SessionPhase::Live { lease_deadline } => now >= lease_deadline,
            SessionPhase::Idle | SessionPhase::Restarting => false,
        };
        if expired {
            self.phase = SessionPhase::Restarting;
        }
    }

    fn claim_offer(&mut self, now: Instant) -> OfferDisposition {
        self.expire(now);
        match self.phase {
            SessionPhase::Idle => {
                self.phase = SessionPhase::Negotiating {
                    deadline: now + CLIENT_HANDSHAKE_TIMEOUT,
                };
                OfferDisposition::Claimed
            }
            SessionPhase::Negotiating { .. } | SessionPhase::Live { .. } => OfferDisposition::Busy,
            SessionPhase::Restarting => OfferDisposition::Restarting,
        }
    }

    fn complete_answer(&mut self, now: Instant) -> bool {
        self.expire(now);
        matches!(self.phase, SessionPhase::Negotiating { .. })
    }

    fn open_control(&mut self, now: Instant) -> bool {
        self.expire(now);
        if matches!(
            self.phase,
            SessionPhase::Negotiating { .. } | SessionPhase::Live { .. }
        ) {
            self.phase = SessionPhase::Live {
                lease_deadline: now + CLIENT_HEARTBEAT_TIMEOUT,
            };
            true
        } else {
            false
        }
    }

    fn record_activity(&mut self, now: Instant) {
        self.expire(now);
        if matches!(self.phase, SessionPhase::Live { .. }) {
            self.phase = SessionPhase::Live {
                lease_deadline: now + CLIENT_HEARTBEAT_TIMEOUT,
            };
        }
    }

    fn request_restart(&mut self) {
        self.phase = SessionPhase::Restarting;
    }

    fn restart_requested(&mut self, now: Instant) -> bool {
        self.expire(now);
        matches!(self.phase, SessionPhase::Restarting)
    }

    fn stream_ready(&mut self, now: Instant) -> bool {
        self.expire(now);
        matches!(self.phase, SessionPhase::Live { .. })
    }
}

#[derive(Debug, Deserialize)]
struct Offer {
    sdp: String,
    #[serde(default)]
    client_build: Option<String>,
    #[serde(default)]
    client_protocol: Option<u32>,
}

#[derive(Debug, Serialize)]
struct Answer<'a> {
    #[serde(rename = "type")]
    type_: &'static str,
    sdp: &'a str,
}

fn env_u32(name: &str, default: u32, min: u32, max: u32) -> Result<u32> {
    let Some(value) = env::var_os(name) else {
        return Ok(default);
    };
    let value = value
        .to_str()
        .with_context(|| format!("{name} is not valid UTF-8"))?;
    parse_bounded_u32(name, value, min, max)
}

fn env_flag(name: &str, default: bool) -> Result<bool> {
    let Some(value) = env::var_os(name) else {
        return Ok(default);
    };
    let value = value
        .to_str()
        .with_context(|| format!("{name} is not valid UTF-8"))?;
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => bail!("{name} must be 0 or 1, got {value:?}"),
    }
}

fn parse_bounded_u32(name: &str, value: &str, min: u32, max: u32) -> Result<u32> {
    let parsed = value
        .parse::<u32>()
        .with_context(|| format!("{name} must be an integer, got {value:?}"))?;
    ensure!(
        (min..=max).contains(&parsed),
        "{name} must be in {min}..={max}, got {parsed}"
    );
    Ok(parsed)
}

fn read_kernel_u64(path: &str) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn configure_lan_nice_agent(webrtc: &gst::Element) -> Result<()> {
    // This renderer is intentionally LAN-only and advertises host candidates.
    // libnice otherwise enables UPnP and ICE-TCP by default, causing unrelated
    // router discovery/timeouts and extra candidate sockets long after the UDP
    // host pair is already usable.
    let ice_agent = webrtc.property::<gst::glib::Object>("ice-agent");
    ensure!(
        ice_agent.find_property("agent").is_some(),
        "GStreamer WebRTC ICE implementation does not expose its NiceAgent"
    );
    let nice_agent = ice_agent.property::<gst::glib::Object>("agent");
    for property in ["upnp", "ice-tcp"] {
        ensure!(
            nice_agent.find_property(property).is_some(),
            "NiceAgent has no {property} property"
        );
        nice_agent.set_property(property, false);
        ensure!(
            !nice_agent.property::<bool>(property),
            "NiceAgent rejected {property}=false"
        );
    }
    Ok(())
}

fn cadence_offset_ns(slot_delta: u64, fps: u32) -> u64 {
    debug_assert!(fps > 0);
    (u128::from(slot_delta) * 1_000_000_000_u128 / u128::from(fps)).min(u128::from(u64::MAX)) as u64
}

fn primary_ssrc_from_entropy(bytes: [u8; 4]) -> u32 {
    match u32::from_le_bytes(bytes) {
        0 => 1,
        u32::MAX => u32::MAX - 1,
        value => value,
    }
}

fn generate_primary_ssrc() -> Result<u32> {
    let mut entropy = [0_u8; 4];
    fs::File::open("/dev/urandom")
        .context("open OS entropy for RTP primary SSRC")?
        .read_exact(&mut entropy)
        .context("read OS entropy for RTP primary SSRC")?;
    Ok(primary_ssrc_from_entropy(entropy))
}

fn media_pipeline_description(config: &MediaConfig, primary_ssrc: u32) -> String {
    debug_assert!(primary_ssrc != 0 && primary_ssrc != u32::MAX);
    let encoder_cpb_length_ms = cpb_length_ms(config.cpb_size_kbits, config.bitrate_kbps);
    // Do not put chroma-site on the VASurface capsfilter. vaapipostproc fixes
    // the actual NV12 output to JPEG/centered, but forcing that field during
    // the old vaapih264enc reverse caps query makes its peer caps EMPTY on
    // GStreamer 1.24. The one-shot pixel gate proves that exact fixated path;
    // capssetter below carries the value to RTP without asserting an SPS VUI
    // field that this target encoder path does not provide.
    format!(
        "webrtcbin name=webrtc bundle-policy=max-bundle \
         appsrc name=source is-live=true do-timestamp=false block=false format=time emit-signals=false max-buffers={MAX_APPSRC_BUFFERS} max-bytes=0 max-time=0 leaky-type=downstream \
         ! vaapipostproc name=vpp \
         ! video/x-raw(memory:VASurface),format=NV12,colorimetry={VIDEO_COLORIMETRY} \
         ! vaapih264enc name=encoder bitrate={} cpb-length={} max-bframes=0 refs=1 keyframe-period={} num-slices={} rate-control=cbr quality-level={VIDEO_TARGET_USAGE} cabac=false dct8x8=false \
         ! h264parse config-interval=-1 \
         ! video/x-h264,profile=constrained-baseline,stream-format=byte-stream,alignment=au \
         ! capssetter replace=false caps=video/x-h264,colorimetry={VIDEO_COLORIMETRY},chroma-site={VIDEO_CHROMA_SITE} \
         ! identity name=encoded_done \
         ! rtph264pay name=payloader pt=108 config-interval=-1 aggregate-mode=zero-latency \
         ! application/x-rtp,media=video,encoding-name=H264,payload=108,clock-rate=90000,ssrc=(uint){primary_ssrc},packetization-mode=(string)1,profile=(string)constrained-baseline \
         ! webrtc.",
        config.bitrate_kbps,
        encoder_cpb_length_ms,
        config.keyframe_interval_frames,
        config.video_slices
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AnswerRecoveryCapabilities {
    generic_nack: bool,
    rtx_payload_type: Option<u32>,
    fid_primary_ssrc: Option<u32>,
    fid_rtx_ssrc: Option<u32>,
    primary_ssrc_advertised: bool,
    rtx_ssrc_advertised: bool,
}

impl AnswerRecoveryCapabilities {
    fn has_valid_ssrc_association(self, expected_primary_ssrc: u32) -> bool {
        matches!(
            (self.fid_primary_ssrc, self.fid_rtx_ssrc),
            (Some(primary), Some(rtx))
                if primary == expected_primary_ssrc
                    && primary != 0
                    && primary != u32::MAX
                    && rtx != 0
                    && rtx != primary
                    && self.primary_ssrc_advertised
                    && self.rtx_ssrc_advertised
        )
    }
}

fn inspect_answer_recovery_capabilities(answer: &str) -> AnswerRecoveryCapabilities {
    let mut in_video = false;
    let mut video_lines = Vec::new();
    for line in answer.lines().map(|line| line.trim_end_matches('\r')) {
        if line.starts_with("m=") {
            if !video_lines.is_empty() {
                break;
            }
            in_video = line.starts_with("m=video ");
        }
        if in_video {
            video_lines.push(line);
        }
    }
    let advertised_payload_types = video_lines
        .first()
        .into_iter()
        .flat_map(|line| line.split_ascii_whitespace().skip(3))
        .filter_map(|payload_type| payload_type.parse::<u32>().ok())
        .collect::<HashSet<_>>();
    let feedback_prefix = format!("a=rtcp-fb:{H264_RTP_PAYLOAD_TYPE}");
    let generic_nack = video_lines.iter().any(|line| {
        let mut fields = line.split_ascii_whitespace();
        fields.next() == Some(feedback_prefix.as_str())
            && fields.next() == Some("nack")
            && fields.next().is_none()
    });
    let rtx_payload_type = video_lines
        .iter()
        .filter_map(|line| {
            let value = line.strip_prefix("a=rtpmap:")?;
            let (payload_type, encoding) = value.split_once(' ')?;
            encoding
                .eq_ignore_ascii_case("rtx/90000")
                .then(|| payload_type.parse::<u32>().ok())
                .flatten()
        })
        .filter(|payload_type| advertised_payload_types.contains(payload_type))
        .find(|payload_type| {
            let prefix = format!("a=fmtp:{payload_type} ");
            video_lines.iter().any(|line| {
                line.strip_prefix(&prefix).is_some_and(|parameters| {
                    parameters
                        .split(';')
                        .any(|parameter| parameter.trim() == format!("apt={H264_RTP_PAYLOAD_TYPE}"))
                })
            })
        });
    let fid_ssrcs = video_lines.iter().find_map(|line| {
        let value = line.strip_prefix("a=ssrc-group:FID ")?;
        let mut fields = value.split_ascii_whitespace();
        let primary = fields.next()?.parse::<u32>().ok()?;
        let rtx = fields.next()?.parse::<u32>().ok()?;
        fields.next().is_none().then_some((primary, rtx))
    });
    let advertised_ssrcs = video_lines
        .iter()
        .filter_map(|line| line.strip_prefix("a=ssrc:"))
        .filter_map(|value| value.split_ascii_whitespace().next())
        .filter_map(|ssrc| ssrc.parse::<u32>().ok())
        .collect::<HashSet<_>>();
    let (fid_primary_ssrc, fid_rtx_ssrc) = fid_ssrcs
        .map(|(primary, rtx)| (Some(primary), Some(rtx)))
        .unwrap_or((None, None));
    AnswerRecoveryCapabilities {
        generic_nack,
        rtx_payload_type,
        fid_primary_ssrc,
        fid_rtx_ssrc,
        primary_ssrc_advertised: fid_primary_ssrc
            .is_some_and(|ssrc| advertised_ssrcs.contains(&ssrc)),
        rtx_ssrc_advertised: fid_rtx_ssrc.is_some_and(|ssrc| advertised_ssrcs.contains(&ssrc)),
    }
}

fn configure_sender(webrtc: &gst::Element, priority_name: &str) -> Result<bool> {
    let transceiver = webrtc
        .emit_by_name::<Option<WebRTCRTPTransceiver>>("get-transceiver", &[&0_i32])
        .context("webrtcbin sender transceiver 0")?;
    let sender = transceiver
        .sender()
        .context("webrtcbin sender transceiver has no RTP sender")?;
    if let Some(priority) = parse_webrtc_video_priority(priority_name)? {
        sender.set_priority(priority);
    }
    // The public setter returns void. In GStreamer 1.24 the generated
    // priority property getter is not ABI-safe for verification, and even a
    // successful property readback would not prove that libnice applied
    // IP_TOS. Validate the actual socket and wire packet in the live gate.
    ensure!(
        transceiver.find_property("do-nack").is_some(),
        "GStreamer WebRTC sender transceiver has no do-nack property"
    );
    transceiver.set_property("do-nack", true);
    let enabled = transceiver.property::<bool>("do-nack");
    ensure!(enabled, "GStreamer rejected sender do-nack=true");
    Ok(enabled)
}

unsafe extern "C" fn frame_buffer_finalized(
    data: *mut std::ffi::c_void,
    _mini_object: *mut gst::ffi::GstMiniObject,
) {
    let release = unsafe { Box::from_raw(data.cast::<FrameReleaseContext>()) };
    let dropped_before_dequeue = release
        .queued
        .lock()
        .unwrap()
        .remove(&release.pts_ns)
        .is_some();
    if dropped_before_dequeue {
        release.metrics.lock().unwrap().appsrc_dropped_frames += 1;
    }
    let _ = release.released_tx.send(release.slot);
}

impl Drop for StreamTransport {
    fn drop(&mut self) {
        // Construction starts the media pipeline before the HTTP listener is
        // bound. If a later initialization step fails (for example, the port
        // is already owned), transition every child to NULL before GObject
        // references are released. Dropping a PLAYING pipeline can otherwise
        // emit criticals and crash while the supervisor is trying to recover.
        // Removing the handler drops the worker's only sender. The worker
        // holds only a weak pipeline reference, and its JoinHandle is detached
        // on field drop, so teardown never waits on a GStreamer call and no
        // strong-reference cycle can keep the pipeline alive.
        if let Some(bus) = self.pipeline.bus() {
            bus.unset_sync_handler();
        }
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

impl StreamTransport {
    pub fn new(
        config: StreamTransportConfig<'_>,
        control_tx: mpsc::SyncSender<String>,
        latest_keyframe_request: Arc<Mutex<Option<ControlMessage>>>,
        status: Arc<Mutex<LiveStatus>>,
    ) -> Result<Self> {
        let StreamTransportConfig {
            layout,
            width,
            height,
            fps,
            slots,
        } = config;
        let media_config = MediaConfig::from_env()?;
        let system_udp_wmem_default_bytes =
            read_kernel_u64("/proc/sys/net/core/wmem_default").unwrap_or_default();
        let system_udp_wmem_max_bytes =
            read_kernel_u64("/proc/sys/net/core/wmem_max").unwrap_or_default();
        if system_udp_wmem_default_bytes < RECOMMENDED_UDP_WMEM_BYTES
            || system_udp_wmem_max_bytes < RECOMMENDED_UDP_WMEM_BYTES
        {
            eprintln!(
                "WebRTC UDP send buffer is below the {RECOMMENDED_UDP_WMEM_BYTES}-byte recommendation (default={system_udp_wmem_default_bytes}, max={system_udp_wmem_max_bytes}); the safe {} kbps encoder default remains active. Do not raise the encoder or pacer overrides until the host is provisioned and measured.",
                media_config.bitrate_kbps
            );
        }
        ensure!(
            layout.modifier == 0,
            "the validated VA import path requires linear DMA-BUF modifier 0; got {} and refuses tiled fallback",
            layout.modifier_hex
        );
        ensure!(
            layout.drm_fourcc_name == "AR24",
            "the validated VA import path requires DRM AR24, got {}",
            layout.drm_fourcc_name
        );
        eprintln!(
            "DMA-BUF modifier gate: candidates={:?}, selected={}, policy={}",
            layout.candidate_modifiers, layout.modifier_hex, layout.modifier_selection_policy
        );
        gst::init().context("initialize GStreamer")?;
        for element in [
            "appsrc",
            "vaapipostproc",
            "vaapih264enc",
            "h264parse",
            "capssetter",
            "identity",
            "rtph264pay",
            "rtphdrextcolorspace",
            "rtprtxsend",
            "webrtcbin",
        ] {
            ensure!(
                gst::ElementFactory::find(element).is_some(),
                "required GStreamer element {element} is unavailable"
            );
        }
        // rtph264pay's UINT_MAX default means "choose an SSRC lazily". With
        // RTX enabled, webrtcbin can serialize its FID group before the first
        // RTP packet causes that lazy choice, advertising primary SSRC 0 while
        // the payloader later sends a different random value. Pin the primary
        // SSRC before pipeline construction so SDP, RTP and RTX share one
        // identity from the beginning of negotiation.
        let primary_ssrc = generate_primary_ssrc()?;
        let pipeline_description = media_pipeline_description(&media_config, primary_ssrc);
        let pipeline = gst::parse::launch(&pipeline_description)
            .context("construct DMA-BUF WebRTC pipeline")?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow::anyhow!("GStreamer launch did not return a pipeline"))?;
        let payloader = pipeline
            .by_name("payloader")
            .context("H.264 RTP payloader")?;
        // Chrome offers the WebRTC color-space RTP header extension. With
        // auto-header-extension enabled, rtph264pay instantiates the factory
        // above for the negotiated extmap and serializes the BT.709
        // range/matrix/transfer/primaries/chroma contract from its H.264 sink
        // caps. This keeps the negotiated transport color contract explicit
        // when the target encoder path omits SPS VUI color-description fields.
        payloader.set_property("auto-header-extension", true);
        ensure!(
            payloader.property::<bool>("auto-header-extension"),
            "GStreamer rejected automatic RTP color-space header extensions"
        );
        payloader.set_property("ssrc", primary_ssrc);
        ensure!(
            payloader.property::<u32>("ssrc") == primary_ssrc,
            "GStreamer rejected the configured RTP primary SSRC"
        );
        let appsrc = pipeline
            .by_name("source")
            .context("pipeline appsrc")?
            .downcast::<AppSrc>()
            .map_err(|_| anyhow::anyhow!("source is not AppSrc"))?;
        let webrtc = pipeline.by_name("webrtc").context("pipeline webrtcbin")?;
        configure_lan_nice_agent(&webrtc).context("configure LAN-only ICE")?;
        let caps = gst::Caps::builder("video/x-raw")
            .features(["memory:DMABuf"])
            // Linear DRM AR24 is little-endian B,G,R,A, which is GStreamer's
            // legacy BGRA DMABuf contract. The modern DMA_DRM caps path on
            // this host only advertises the known-bad tiled modifier.
            .field("format", "BGRA")
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gst::Fraction::new(fps as i32, 1))
            .field("colorimetry", SOURCE_COLORIMETRY)
            .build();
        appsrc.set_caps(Some(&caps));

        let initial_metrics = TransportMetrics {
            configured_bitrate_kbps: media_config.bitrate_kbps,
            keyframe_interval_frames: media_config.keyframe_interval_frames,
            video_slices: media_config.video_slices,
            cpb_size_kbits: media_config.cpb_size_kbits,
            webrtc_video_priority: media_config.webrtc_video_priority.clone(),
            system_udp_wmem_default_bytes,
            system_udp_wmem_max_bytes,
            recommended_udp_wmem_bytes: RECOMMENDED_UDP_WMEM_BYTES,
            experimental_lan_pacer: media_config.experimental_lan_pacer,
            nice_max_bitrate_bps: media_config.nice_max_bitrate_bps,
            nice_pacer_burst_bytes: media_config.nice_pacer_burst_bytes,
            rtx_history_packets: RTX_HISTORY_PACKETS,
            rtx_history_ms: RTX_HISTORY_MS,
            primary_ssrc,
            free_slots: slots as u64,
            ..Default::default()
        };
        let metrics = Arc::new(Mutex::new(initial_metrics));
        let pipeline_weak = pipeline.downgrade();
        let latency_worker_metrics = Arc::clone(&metrics);
        let latency_recalculation_serialization = Arc::new(Mutex::new(()));
        let latency_worker_serialization = Arc::clone(&latency_recalculation_serialization);
        let latency_recalculation_counters = Arc::new(LatencyRecalculationCounters::default());
        let latency_bus_counters = Arc::clone(&latency_recalculation_counters);
        let (latency_tx, latency_rx) = mpsc::sync_channel(1);
        let latency_recalculation_worker = thread::Builder::new()
            .name("gstreamer-latency-recalc".into())
            .spawn(move || {
                while latency_rx.recv().is_ok() {
                    let Some(pipeline) = pipeline_weak.upgrade() else {
                        break;
                    };
                    recalculate_pipeline_latency(
                        &pipeline,
                        &latency_worker_metrics,
                        &latency_worker_serialization,
                        "latency-message",
                    );
                }
            })
            .context("start GStreamer latency recalculation worker")?;
        let bus_metrics = Arc::clone(&metrics);
        pipeline
            .bus()
            .context("pipeline bus")?
            .set_sync_handler(move |_, message| {
                match message.view() {
                    gst::MessageView::Latency(_) => {
                        // A sync handler runs in the message-posting thread,
                        // which may be a streaming thread. Only latch a bounded
                        // request here; the dedicated application thread calls
                        // gst_bin_recalculate_latency(). Capacity one gives us
                        // one in-flight plus one pending recalculation and
                        // coalesces a message storm without blocking media.
                        let schedule = schedule_latency_recalculation(&latency_tx);
                        record_latency_message(&latency_bus_counters, schedule);
                    }
                    gst::MessageView::Warning(warning) => {
                        let source = message
                            .src()
                            .map(|source| source.path_string())
                            .unwrap_or_else(|| "<unknown>".into());
                        eprintln!(
                            "GStreamer warning from {source}: {} ({})",
                            warning.error(),
                            warning.debug().unwrap_or_default()
                        );
                        if let Ok(mut metrics) = bus_metrics.lock() {
                            metrics.gstreamer_bus_warnings =
                                metrics.gstreamer_bus_warnings.saturating_add(1);
                        }
                    }
                    gst::MessageView::Error(error) => {
                        let source = message
                            .src()
                            .map(|source| source.path_string())
                            .unwrap_or_else(|| "<unknown>".into());
                        eprintln!(
                            "GStreamer error from {source}: {} ({})",
                            error.error(),
                            error.debug().unwrap_or_default()
                        );
                        if let Ok(mut metrics) = bus_metrics.lock() {
                            metrics.gstreamer_bus_errors =
                                metrics.gstreamer_bus_errors.saturating_add(1);
                        }
                    }
                    _ => {}
                }
                // This renderer has no asynchronous bus consumer. Dropping
                // after the synchronous observation prevents state and QoS
                // messages from accumulating forever in an unread queue.
                gst::BusSyncReply::Drop
            });
        let pacer_metrics = Arc::clone(&metrics);
        let rtx_sender = Arc::new(Mutex::new(None::<gst::Element>));
        let rtx_sender_added = Arc::clone(&rtx_sender);
        let nice_max_bitrate_bps = media_config.nice_max_bitrate_bps;
        let nice_pacer_burst_bytes = media_config.nice_pacer_burst_bytes;
        let experimental_lan_pacer = media_config.experimental_lan_pacer;
        pipeline.connect_deep_element_added(move |_, _, element| {
            let factory_name = element.factory().map(|factory| factory.name());
            if factory_name.as_deref() == Some("rtprtxsend") {
                element.set_property("max-size-packets", RTX_HISTORY_PACKETS);
                element.set_property("max-size-time", RTX_HISTORY_MS);
                let effective_packets = element.property::<u32>("max-size-packets");
                let effective_ms = element.property::<u32>("max-size-time");
                {
                    let mut rtx = rtx_sender_added.lock().unwrap();
                    *rtx = Some(element.clone());
                }
                let mut metrics = pacer_metrics.lock().unwrap();
                metrics.rtx_sender_present = true;
                metrics.rtx_history_packets = effective_packets;
                metrics.rtx_history_ms = effective_ms;
            }
            let is_nice_sink = element
                .factory()
                .is_some_and(|factory| factory.name() == "nicesink");
            if is_nice_sink && experimental_lan_pacer {
                element.set_property("max-bitrate", nice_max_bitrate_bps);
                let effective = element.property::<u64>("max-bitrate");
                let packet_pacer = Arc::new(Mutex::new(PacketPacer::new(
                    nice_max_bitrate_bps,
                    nice_pacer_burst_bytes,
                )));
                let packet_pacer_probe = Arc::clone(&packet_pacer);
                let probe_metrics = Arc::clone(&pacer_metrics);
                let probe_installed = element
                    .static_pad("sink")
                    .and_then(|pad| {
                        pad.add_probe(
                            gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
                            move |_, info| {
                                let (payload_bytes, packet_count, is_buffer_list) =
                                    match info.data.as_ref() {
                                        Some(gst::PadProbeData::Buffer(buffer)) => {
                                            (buffer.size(), 1_u64, false)
                                        }
                                        Some(gst::PadProbeData::BufferList(list)) => {
                                            (list.calculate_size(), list.len() as u64, true)
                                        }
                                        _ => (0, 0, false),
                                    };
                                if payload_bytes > 0 && packet_count > 0 {
                                    // DTLS/SRTP bytes are already in the buffer at this boundary.
                                    // Charge the larger IPv6+UDP overhead even on an IPv4 path
                                    // so an ICE address-family change cannot make the shaper
                                    // undercount the wire rate. GstBaseSink max-bitrate alone
                                    // does not guarantee packet spacing within a video-frame burst,
                                    // so pace the actual outgoing datagrams explicitly.
                                    let wire_bytes =
                                        (payload_bytes as u64)
                                            .saturating_add(packet_count.saturating_mul(
                                                CONSERVATIVE_IP_UDP_OVERHEAD_BYTES,
                                            ));
                                    let requested_wait =
                                        packet_pacer_probe.lock().unwrap().reserve(wire_bytes);
                                    let wait_sample = execute_pacer_wait(requested_wait);
                                    let mut metrics = probe_metrics.lock().unwrap();
                                    metrics.nice_pacer_packets =
                                        metrics.nice_pacer_packets.saturating_add(packet_count);
                                    metrics.nice_pacer_buffer_lists += u64::from(is_buffer_list);
                                    metrics.nice_pacer_max_batch_packets =
                                        metrics.nice_pacer_max_batch_packets.max(packet_count);
                                    metrics.nice_pacer_max_batch_wire_bytes =
                                        metrics.nice_pacer_max_batch_wire_bytes.max(wire_bytes);
                                    metrics.nice_pacer_oversize_batches +=
                                        u64::from(wire_bytes > u64::from(nice_pacer_burst_bytes));
                                    record_pacer_wait(&mut metrics, wait_sample);
                                }
                                gst::PadProbeReturn::Ok
                            },
                        )
                    })
                    .is_some();
                let mut metrics = pacer_metrics.lock().unwrap();
                metrics.nice_pacer_configured =
                    effective == nice_max_bitrate_bps && probe_installed;
                metrics.nice_max_bitrate_bps = effective;
                metrics.nice_pacer_burst_bytes = nice_pacer_burst_bytes;
            }
        });
        // Register the internal-element observer before configuring the
        // sender. An explicitly requested priority must be set before
        // PLAYING/SDP/ICE. Some GStreamer versions may also create rtprtxsend
        // synchronously while the transceiver property changes; enabling NACK
        // first would miss its history configuration and make the startup
        // recovery gate depend on implementation timing.
        let nack_retransmission_enabled =
            configure_sender(&webrtc, &media_config.webrtc_video_priority)
                .context("configure WebRTC sender priority and NACK/RTX recovery")?;
        metrics.lock().unwrap().nack_retransmission_enabled = nack_retransmission_enabled;

        let queued = Arc::new(Mutex::new(HashMap::<u64, PendingFrame>::new()));
        let encoding = Arc::new(Mutex::new(VecDeque::<PendingFrame>::new()));
        let keyframe_recovery = Arc::new(Mutex::new(KeyframeRecoveryTracker::default()));
        let (completed_tx, completed_rx) = mpsc::channel();
        let (released_tx, released_rx) = mpsc::channel();
        let queued_dequeue = Arc::clone(&queued);
        let encoding_dequeue = Arc::clone(&encoding);
        let appsrc_pad = appsrc.static_pad("src").context("appsrc source pad")?;
        appsrc_pad
            .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                if let Some(gst::PadProbeData::Buffer(buffer)) = &info.data
                    && let Some(pts) = buffer.pts()
                    && let Some(queued) = queued_dequeue.lock().unwrap().remove(&pts.nseconds())
                {
                    encoding_dequeue.lock().unwrap().push_back(queued);
                }
                gst::PadProbeReturn::Ok
            })
            .context("install appsrc dequeue probe")?;

        // RTCP PLI/FIR feedback is converted by GStreamer's RTP session into an
        // upstream GstForceKeyUnit event. Observe it both where it enters the
        // H.264 payloader and where it reaches GstVideoEncoder: unequal counts
        // identify an element that swallowed the request without changing the
        // event's normal propagation.
        let payloader_src = pipeline
            .by_name("payloader")
            .context("H.264 RTP payloader")?
            .static_pad("src")
            .context("H.264 RTP payloader src pad")?;
        let request_tracker = Arc::clone(&keyframe_recovery);
        let request_metrics = Arc::clone(&metrics);
        payloader_src
            .add_probe(gst::PadProbeType::EVENT_UPSTREAM, move |_, info| {
                if let Some(gst::PadProbeData::Event(event)) = info.data.as_ref()
                    && let Ok(ForceKeyUnitEvent::Upstream(request)) =
                        ForceKeyUnitEvent::parse(event.as_ref())
                {
                    // Keep tracker -> metrics as the single lock order used by
                    // both the request and keyframe probes.
                    let mut tracker = request_tracker.lock().unwrap();
                    let pending_requests =
                        tracker.note_request(Instant::now(), KeyframeRequestSource::RtcpFeedback);
                    let mut metrics = request_metrics.lock().unwrap();
                    metrics.force_key_unit_requests =
                        metrics.force_key_unit_requests.saturating_add(1);
                    metrics.pending_force_key_unit_requests = pending_requests;
                    metrics.last_force_key_unit_request_running_time_ns =
                        request.running_time.map(|value| value.nseconds());
                }
                gst::PadProbeReturn::Ok
            })
            .context("install RTP force-key-unit request probe")?;

        let encoder_src = pipeline
            .by_name("encoder")
            .context("H.264 encoder")?
            .static_pad("src")
            .context("H.264 encoder src pad")?;
        let encoder_request_metrics = Arc::clone(&metrics);
        encoder_src
            .add_probe(gst::PadProbeType::EVENT_UPSTREAM, move |_, info| {
                if let Some(gst::PadProbeData::Event(event)) = info.data.as_ref()
                    && matches!(
                        ForceKeyUnitEvent::parse(event.as_ref()),
                        Ok(ForceKeyUnitEvent::Upstream(_))
                    )
                {
                    let mut metrics = encoder_request_metrics.lock().unwrap();
                    metrics.force_key_unit_requests_at_encoder =
                        metrics.force_key_unit_requests_at_encoder.saturating_add(1);
                }
                gst::PadProbeReturn::Ok
            })
            .context("install encoder force-key-unit request probe")?;

        let encoding_probe = Arc::clone(&encoding);
        let recovery_probe = Arc::clone(&keyframe_recovery);
        let metrics_probe = Arc::clone(&metrics);
        let pad = pipeline
            .by_name("encoded_done")
            .context("encoded completion identity")?
            .static_pad("src")
            .context("encoded identity src pad")?;
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buffer)) = &info.data
                && let Some(pts) = buffer.pts()
            {
                let encoded_au_bytes = buffer.size() as u64;
                let is_keyframe = encoded_access_unit_is_keyframe(buffer.flags());
                // Force-key-unit traffic is rare. Holding this guard through
                // the metrics update makes tracker state and its exported
                // pending count one atomic observation without adding a lock
                // to the per-frame delta-unit path.
                let mut recovery_tracker = is_keyframe.then(|| recovery_probe.lock().unwrap());
                let recovery = recovery_tracker
                    .as_mut()
                    .and_then(|tracker| tracker.note_keyframe(Instant::now()));
                {
                    let mut metrics = metrics_probe.lock().unwrap();
                    metrics.encoded_frames = metrics.encoded_frames.saturating_add(1);
                    metrics.encoded_au_bytes = encoded_au_bytes;
                    metrics.encoded_au_bytes_max =
                        metrics.encoded_au_bytes_max.max(encoded_au_bytes);
                    if is_keyframe {
                        metrics.encoded_keyframes = metrics.encoded_keyframes.saturating_add(1);
                        metrics.last_keyframe_pts_ns = Some(pts.nseconds());
                        metrics.pending_force_key_unit_requests = recovery_tracker
                            .as_ref()
                            .map_or(0, |tracker| tracker.pending_requests());
                        if let Some(recovery) = recovery {
                            let latency_ms = recovery.latency.as_secs_f64() * 1_000.0;
                            metrics.force_key_unit_recoveries =
                                metrics.force_key_unit_recoveries.saturating_add(1);
                            metrics.force_key_unit_requests_recovered = metrics
                                .force_key_unit_requests_recovered
                                .saturating_add(recovery.requests);
                            metrics.feedback_force_key_unit_requests_recovered = metrics
                                .feedback_force_key_unit_requests_recovered
                                .saturating_add(recovery.feedback_coverage);
                            metrics.last_force_key_unit_request_to_keyframe_ms = Some(latency_ms);
                            metrics.max_force_key_unit_request_to_keyframe_ms = metrics
                                .max_force_key_unit_request_to_keyframe_ms
                                .max(latency_ms);
                        }
                    }
                }
                drop(recovery_tracker);
                if let Some(pending) = encoding_probe.lock().unwrap().pop_front() {
                    let encode_ms = pending.pushed_at.elapsed().as_secs_f64() * 1000.0;
                    let _ = completed_tx.send(Completion { encode_ms });
                }
            }
            gst::PadProbeReturn::Ok
        })
        .context("install encoder completion probe")?;

        let data_channels = Arc::new(Mutex::new(DataChannels::default()));
        let lifecycle = Arc::new(Mutex::new(SessionLifecycle::default()));
        connect_data_channel(
            &webrtc,
            Arc::clone(&data_channels),
            control_tx.clone(),
            latest_keyframe_request,
            Arc::clone(&status),
            Arc::clone(&lifecycle),
        )?;
        connect_peer_status(&webrtc, Arc::clone(&status), Arc::clone(&lifecycle));
        let signaler = Arc::new(WebRtcSignaler {
            webrtc,
            primary_ssrc,
            lifecycle,
            status,
            metrics: Arc::clone(&metrics),
        });
        pipeline
            .set_state(gst::State::Playing)
            .context("start WebRTC pipeline")?;
        // Do not rely on a timing-sensitive initial LATENCY message: perform
        // one explicit recalculation after PLAYING on this application thread.
        // Dynamic webrtcbin/nicesink changes remain covered by the bus worker.
        recalculate_pipeline_latency(
            &pipeline,
            &metrics,
            &latency_recalculation_serialization,
            "initial-playing",
        );
        Ok(Self {
            pipeline,
            _latency_recalculation_worker: latency_recalculation_worker,
            latency_recalculation_counters,
            appsrc,
            allocator: DmaBufAllocator::new(),
            layout: layout.clone(),
            width,
            height,
            fps,
            queued,
            encoding,
            completed_rx,
            released_tx,
            released_rx,
            free: (0..slots).collect(),
            encoder_src,
            keyframe_recovery,
            metrics,
            rtx_sender,
            cadence_epoch: None,
            last_cadence_slot: None,
            last_pts_ns: 0,
            signaler,
        })
    }

    pub fn signaler(&self) -> Arc<WebRtcSignaler> {
        Arc::clone(&self.signaler)
    }

    pub fn force_key_unit(&self) -> Result<()> {
        self.force_key_unit_with_source(KeyframeRequestSource::OtherManual)
    }

    pub fn force_key_unit_for_feedback_fallback(&self, uncovered: u64) -> Result<()> {
        ensure!(uncovered > 0, "feedback fallback coverage must be positive");
        self.force_key_unit_with_source(KeyframeRequestSource::FeedbackFallback {
            coverage: uncovered,
        })
    }

    fn force_key_unit_with_source(&self, source: KeyframeRequestSource) -> Result<()> {
        // Inject at the encoder's source pad, the documented entry point for
        // an upstream event. Keep the tracker locked until send_event returns
        // so a concurrently produced keyframe cannot overtake its request.
        let mut tracker = self.keyframe_recovery.lock().unwrap();
        let accepted = self.encoder_src.send_event(
            UpstreamForceKeyUnitEvent::builder()
                .all_headers(true)
                .build(),
        );
        ensure!(accepted, "H.264 encoder rejected force-key-unit request");
        let pending_requests = tracker.note_request(Instant::now(), source);
        let mut metrics = self.metrics.lock().unwrap();
        metrics.active_force_key_unit_requests =
            metrics.active_force_key_unit_requests.saturating_add(1);
        metrics.pending_force_key_unit_requests = pending_requests;
        metrics.last_force_key_unit_request_running_time_ns = None;
        Ok(())
    }

    pub fn acquire_slot(&mut self) -> Result<(usize, Option<Completion>)> {
        let wait_started = Instant::now();
        let mut latest = None;
        while let Ok(completion) = self.completed_rx.try_recv() {
            latest = Some(completion);
        }
        while let Ok(slot) = self.released_rx.try_recv() {
            self.free.push_back(slot);
        }
        if let Some(slot) = self.free.pop_front() {
            self.record_slot_wait(wait_started.elapsed());
            return Ok((slot, latest));
        }
        let slot = self
            .released_rx
            .recv_timeout(Duration::from_secs(3))
            .context("GStreamer did not release a DMA-BUF slot within 3 seconds")?;
        while let Ok(completion) = self.completed_rx.try_recv() {
            latest = Some(completion);
        }
        self.record_slot_wait(wait_started.elapsed());
        Ok((slot, latest))
    }

    pub fn push(&mut self, slot: usize, cadence_slot: u64, image: &ExternalImage) -> Result<()> {
        let push_started = Instant::now();
        ensure!(
            image.layout.modifier == self.layout.modifier
                && image.layout.stride == self.layout.stride,
            "frame slot DMA-BUF layout changed"
        );
        let fd = image
            .dmabuf
            .try_clone()
            .context("duplicate rendered DMA-BUF for GStreamer")?;
        let memory = unsafe {
            self.allocator
                .alloc_dmabuf(fd, image.layout.allocation_bytes as usize)
        }
        .context("wrap frame slot DMA-BUF")?;
        let capture_running_ns = self
            .pipeline
            .current_running_time()
            .context("WebRTC pipeline has no running-time clock")?
            .nseconds();
        if let Some(previous_slot) = self.last_cadence_slot {
            ensure!(
                cadence_slot > previous_slot,
                "video cadence slot must increase: previous={previous_slot}, current={cadence_slot}"
            );
        }
        let epoch = *self.cadence_epoch.get_or_insert(CadenceEpoch {
            slot: cadence_slot,
            pts_ns: capture_running_ns,
        });
        let relative_slot = cadence_slot
            .checked_sub(epoch.slot)
            .context("video cadence slot predates its PTS epoch")?;
        let next_relative_slot = relative_slot
            .checked_add(1)
            .context("video cadence slot overflow")?;
        let pts_ns = epoch
            .pts_ns
            .saturating_add(cadence_offset_ns(relative_slot, self.fps));
        let frame_ns = cadence_offset_ns(next_relative_slot, self.fps)
            .saturating_sub(cadence_offset_ns(relative_slot, self.fps));
        ensure!(frame_ns > 0, "video frame duration rounded to zero");
        ensure!(
            self.last_pts_ns == 0 || pts_ns > self.last_pts_ns,
            "video PTS must increase: previous={}, current={pts_ns}",
            self.last_pts_ns
        );
        self.last_cadence_slot = Some(cadence_slot);
        self.last_pts_ns = pts_ns;
        let mut buffer = gst::Buffer::new();
        {
            let buffer = buffer.get_mut().expect("new GstBuffer is writable");
            buffer.append_memory(memory);
            VideoMeta::add_full(
                buffer,
                VideoFrameFlags::empty(),
                VideoFormat::Bgra,
                self.width,
                self.height,
                &[image.layout.offset as usize],
                &[image.layout.stride as i32],
            )
            .context("attach linear BGRA DMA-BUF layout")?;
            buffer.set_pts(gst::ClockTime::from_nseconds(pts_ns));
            buffer.set_duration(gst::ClockTime::from_nseconds(frame_ns));
        }

        let previous = self.queued.lock().unwrap().insert(
            pts_ns,
            PendingFrame {
                pushed_at: Instant::now(),
            },
        );
        ensure!(previous.is_none(), "duplicate monotonic video PTS {pts_ns}");
        let release = Box::new(FrameReleaseContext {
            slot,
            pts_ns,
            released_tx: self.released_tx.clone(),
            queued: Arc::clone(&self.queued),
            metrics: Arc::clone(&self.metrics),
        });
        unsafe {
            gst::ffi::gst_mini_object_weak_ref(
                buffer
                    .get_mut()
                    .expect("new GstBuffer remains writable")
                    .upcast_mut()
                    .as_mut_ptr(),
                Some(frame_buffer_finalized),
                Box::into_raw(release).cast(),
            );
        }
        if let Err(error) = self.appsrc.push_buffer(buffer) {
            self.metrics.lock().unwrap().push_errors += 1;
            bail!("push DMA-BUF into WebRTC pipeline: {error:?}");
        }
        let push_ms = push_started.elapsed().as_secs_f64() * 1000.0;
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.push_ms = push_ms;
            metrics.push_ms_max = metrics.push_ms_max.max(push_ms);
            metrics.pushed_frames += 1;
            metrics.last_pts_ns = pts_ns;
        }
        Ok(())
    }

    pub fn metrics(&self) -> TransportMetrics {
        let mut metrics = self.metrics.lock().unwrap().clone();
        overlay_latency_message_counters(&mut metrics, &self.latency_recalculation_counters);
        if let Some(rtx_sender) = self.rtx_sender.lock().unwrap().as_ref() {
            metrics.rtx_sender_present = true;
            metrics.rtx_history_packets = rtx_sender.property::<u32>("max-size-packets");
            metrics.rtx_history_ms = rtx_sender.property::<u32>("max-size-time");
            metrics.rtx_requests = u64::from(rtx_sender.property::<u32>("num-rtx-requests"));
            metrics.rtx_packets = u64::from(rtx_sender.property::<u32>("num-rtx-packets"));
        }
        metrics.free_slots = self.free.len() as u64;
        metrics.pending_frames =
            (self.queued.lock().unwrap().len() + self.encoding.lock().unwrap().len()) as u64;
        metrics.appsrc_queued_buffers = self.appsrc.current_level_buffers();
        metrics.appsrc_queued_bytes = self.appsrc.current_level_bytes();
        metrics.appsrc_queued_time_ns = self
            .appsrc
            .current_level_time()
            .map_or(0, gst::ClockTime::nseconds);
        metrics
    }

    pub fn recovery_metrics(&self) -> RecoveryMetrics {
        let metrics = self.metrics.lock().unwrap();
        RecoveryMetrics {
            force_key_unit_requests: metrics.force_key_unit_requests,
            feedback_force_key_unit_requests_recovered: metrics
                .feedback_force_key_unit_requests_recovered,
            pending_force_key_unit_requests: metrics.pending_force_key_unit_requests,
        }
    }

    fn record_slot_wait(&self, elapsed: Duration) {
        let slot_wait_ms = elapsed.as_secs_f64() * 1000.0;
        let mut metrics = self.metrics.lock().unwrap();
        metrics.slot_wait_ms = slot_wait_ms;
        metrics.slot_wait_max_ms = metrics.slot_wait_max_ms.max(slot_wait_ms);
    }

    pub fn stream_ready(&self) -> bool {
        self.signaler.session_is_active()
    }
    pub fn restart_requested(&self) -> bool {
        self.metrics.lock().unwrap().gstreamer_bus_errors > 0 || self.signaler.restart_requested()
    }
}

impl WebRtcSignaler {
    fn restart_requested(&self) -> bool {
        self.lifecycle
            .lock()
            .unwrap()
            .restart_requested(Instant::now())
    }

    fn session_is_active(&self) -> bool {
        // The control-channel lease is the session authority. Once that
        // channel has opened, a transient WebRTC DISCONNECTED/CONNECTING
        // display state must not pause appsrc and leave media PTS behind the
        // running pipeline clock. A genuinely dead path stops renewing the
        // lease and requests a clean process restart after the timeout;
        // Failed/Closed notifications still request one immediately.
        self.lifecycle.lock().unwrap().stream_ready(Instant::now())
    }

    fn answer_offer(&self, offer_sdp: &str) -> Result<OfferOutcome> {
        let disposition = self.lifecycle.lock().unwrap().claim_offer(Instant::now());
        match disposition {
            OfferDisposition::Busy => return Ok(OfferOutcome::Busy),
            OfferDisposition::Restarting => return Ok(OfferOutcome::Restarting),
            OfferDisposition::Claimed => {}
        }
        match self.answer_first_offer(offer_sdp) {
            Ok(answer) => {
                let completed = self
                    .lifecycle
                    .lock()
                    .unwrap()
                    .complete_answer(Instant::now());
                if completed {
                    Ok(OfferOutcome::Answer(answer))
                } else {
                    Ok(OfferOutcome::Restarting)
                }
            }
            Err(error) => {
                // set-remote-description may already have mutated webrtcbin.
                // Never reuse that pipeline for a second owner after any
                // claimed offer fails; run.sh will construct a fresh process.
                self.lifecycle.lock().unwrap().request_restart();
                Err(error)
            }
        }
    }

    fn answer_first_offer(&self, offer_sdp: &str) -> Result<String> {
        let deadline = Instant::now() + CLIENT_HANDSHAKE_TIMEOUT;
        let sdp =
            SDPMessage::parse_buffer(offer_sdp.as_bytes()).context("parse browser SDP offer")?;
        let offer = WebRTCSessionDescription::new(WebRTCSDPType::Offer, sdp);
        let (remote_sender, remote_receiver) = mpsc::sync_channel(1);
        let remote_promise = gst::Promise::with_change_func(move |reply| {
            let result = reply.map(|_| ()).map_err(|error| format!("{error:?}"));
            let _ = remote_sender.send(result);
        });
        self.webrtc
            .emit_by_name::<()>("set-remote-description", &[&offer, &remote_promise]);
        receive_promise_before(
            remote_receiver,
            &remote_promise,
            deadline,
            "set-remote-description",
        )?;

        let (sender, receiver) = mpsc::sync_channel(1);
        let promise = gst::Promise::with_change_func(move |reply| {
            let answer = reply
                .map_err(|error| format!("{error:?}"))
                .and_then(|reply| reply.ok_or_else(|| "missing promise reply".to_owned()))
                .and_then(|structure| {
                    structure
                        .get::<WebRTCSessionDescription>("answer")
                        .map_err(|error| error.to_string())
                });
            let _ = sender.send(answer);
        });
        self.webrtc
            .emit_by_name::<()>("create-answer", &[&None::<gst::Structure>, &promise]);
        let answer = receive_promise_before(receiver, &promise, deadline, "create-answer")?;
        let (local_sender, local_receiver) = mpsc::sync_channel(1);
        let local_promise = gst::Promise::with_change_func(move |reply| {
            let result = reply.map(|_| ()).map_err(|error| format!("{error:?}"));
            let _ = local_sender.send(result);
        });
        self.webrtc
            .emit_by_name::<()>("set-local-description", &[&answer, &local_promise]);
        receive_promise_before(
            local_receiver,
            &local_promise,
            deadline,
            "set-local-description",
        )?;
        loop {
            if self
                .webrtc
                .property::<WebRTCICEGatheringState>("ice-gathering-state")
                == WebRTCICEGatheringState::Complete
            {
                break;
            }
            let remaining = remaining_for(deadline, "ICE gathering")?;
            thread::sleep(remaining.min(Duration::from_millis(20)));
        }
        let local = self
            .webrtc
            .property::<Option<WebRTCSessionDescription>>("local-description")
            .unwrap_or(answer);
        let text = local.sdp().as_text().context("serialize SDP answer")?;
        let recovery = inspect_answer_recovery_capabilities(&text);
        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.negotiated_h264_generic_nack = recovery.generic_nack;
            metrics.negotiated_h264_rtx = recovery.rtx_payload_type.is_some();
            metrics.negotiated_h264_rtx_payload_type = recovery.rtx_payload_type;
            metrics.negotiated_h264_primary_ssrc = recovery.fid_primary_ssrc;
            metrics.negotiated_h264_rtx_ssrc = recovery.fid_rtx_ssrc;
        }
        ensure!(
            recovery.generic_nack
                && recovery.rtx_payload_type.is_some()
                && recovery.has_valid_ssrc_association(self.primary_ssrc),
            "WebRTC answer has an invalid H.264 NACK/RTX association (generic_nack={}, rtx_payload_type={:?}, expected_primary_ssrc={}, fid_primary_ssrc={:?}, fid_rtx_ssrc={:?}, primary_advertised={}, rtx_advertised={})",
            recovery.generic_nack,
            recovery.rtx_payload_type,
            self.primary_ssrc,
            recovery.fid_primary_ssrc,
            recovery.fid_rtx_ssrc,
            recovery.primary_ssrc_advertised,
            recovery.rtx_ssrc_advertised,
        );
        loop {
            let metrics = self.metrics.lock().unwrap();
            let ready = metrics.rtx_sender_present
                && metrics.rtx_history_packets == RTX_HISTORY_PACKETS
                && metrics.rtx_history_ms == RTX_HISTORY_MS;
            drop(metrics);
            if ready {
                break;
            }
            let remaining = remaining_for(deadline, "RTX sender configuration")?;
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
        let metrics = self.metrics.lock().unwrap();
        ensure!(
            metrics.rtx_sender_present
                && metrics.rtx_history_packets == RTX_HISTORY_PACKETS
                && metrics.rtx_history_ms == RTX_HISTORY_MS,
            "WebRTC RTX sender was not configured (present={}, packets={}, ms={})",
            metrics.rtx_sender_present,
            metrics.rtx_history_packets,
            metrics.rtx_history_ms
        );
        drop(metrics);
        self.status.lock().unwrap().state = "NEGOTIATED".into();
        Ok(text)
    }
}

fn remaining_for(deadline: Instant, operation: &str) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .with_context(|| format!("{operation} timed out"))
}

fn receive_promise_before<T>(
    receiver: mpsc::Receiver<std::result::Result<T, String>>,
    promise: &gst::Promise,
    deadline: Instant,
    operation: &str,
) -> Result<T> {
    let remaining = remaining_for(deadline, operation)?;
    match receiver.recv_timeout(remaining) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => bail!("{operation} failed: {error}"),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            promise.interrupt();
            bail!("{operation} timed out")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            promise.interrupt();
            bail!("{operation} promise disconnected")
        }
    }
}

fn connect_data_channel(
    webrtc: &gst::Element,
    destination: Arc<Mutex<DataChannels>>,
    control_tx: mpsc::SyncSender<String>,
    latest_keyframe_request: Arc<Mutex<Option<ControlMessage>>>,
    status: Arc<Mutex<LiveStatus>>,
    lifecycle: Arc<Mutex<SessionLifecycle>>,
) -> Result<()> {
    webrtc.connect("on-data-channel", false, move |values| {
        let channel = values
            .get(1)
            .and_then(|value| value.get::<WebRTCDataChannel>().ok());
        if let Some(channel) = channel {
            let label = channel.label().map(|label| label.to_string());
            let Some(role) = DataChannelRole::from_label(label.as_deref()) else {
                eprintln!(
                    "ignore unknown WebRTC DataChannel label {:?}",
                    label.as_deref()
                );
                // Do not attach message/open/close handlers: traffic on an
                // unknown or unlabeled channel must never renew the owner lease.
                return None;
            };
            let tx = control_tx.clone();
            let priority_request = Arc::clone(&latest_keyframe_request);
            let message_status = Arc::clone(&status);
            let message_lifecycle = Arc::clone(&lifecycle);
            channel.connect_on_message_string(move |_, message| {
                if let Some(message) = message {
                    let route = route_incoming_control(role, message, &tx, &priority_request);
                    account_incoming_control_route(
                        route,
                        Instant::now(),
                        &message_status,
                        &message_lifecycle,
                    );
                }
            });
            if role == DataChannelRole::Control {
                let open_status = Arc::clone(&status);
                let open_lifecycle = Arc::clone(&lifecycle);
                channel.connect_on_open(move |_| {
                    if open_lifecycle.lock().unwrap().open_control(Instant::now()) {
                        open_status.lock().unwrap().control = "OPEN".into();
                    }
                });
                let close_status = Arc::clone(&status);
                let close_lifecycle = Arc::clone(&lifecycle);
                channel.connect_on_close(move |_| {
                    close_status.lock().unwrap().control = "CLOSED".into();
                    close_lifecycle.lock().unwrap().request_restart();
                });
            }
            let mut channels = destination.lock().unwrap();
            match role {
                DataChannelRole::Config => channels.config = Some(channel),
                DataChannelRole::Control => channels.control = Some(channel),
            }
        }
        None
    });
    Ok(())
}

fn connect_peer_status(
    webrtc: &gst::Element,
    status: Arc<Mutex<LiveStatus>>,
    lifecycle: Arc<Mutex<SessionLifecycle>>,
) {
    let peer_status = Arc::clone(&status);
    let peer_lifecycle = Arc::clone(&lifecycle);
    webrtc.connect_notify(Some("connection-state"), move |element, _| {
        let state = element.property::<WebRTCPeerConnectionState>("connection-state");
        peer_status.lock().unwrap().peer = format!("{state:?}").to_uppercase();
        if matches!(
            state,
            WebRTCPeerConnectionState::Failed | WebRTCPeerConnectionState::Closed
        ) {
            peer_lifecycle.lock().unwrap().request_restart();
        }
    });
    let ice_lifecycle = Arc::clone(&lifecycle);
    webrtc.connect_notify(Some("ice-connection-state"), move |element, _| {
        let state = element.property::<WebRTCICEConnectionState>("ice-connection-state");
        status.lock().unwrap().ice = format!("{state:?}").to_uppercase();
        if matches!(
            state,
            WebRTCICEConnectionState::Failed | WebRTCICEConnectionState::Closed
        ) {
            ice_lifecycle.lock().unwrap().request_restart();
        }
    });
}

pub fn start_http(
    bind: IpAddr,
    port: u16,
    signaler: Arc<WebRtcSignaler>,
) -> Result<thread::JoinHandle<()>> {
    ensure!(bind.is_loopback(), "HTTP preview address must be loopback");
    let listener = TcpListener::bind((bind, port))
        .with_context(|| format!("bind HTTP preview address {bind}:{port}"))?;
    let active_connections = Arc::new(AtomicUsize::new(0));
    Ok(thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let Some(permit) = ConnectionPermit::try_acquire(&active_connections) else {
                        let _ = stream.set_write_timeout(Some(HTTP_WRITE_TIMEOUT));
                        let _ = response(
                            &mut stream,
                            503,
                            "text/plain; charset=utf-8",
                            b"too many connections\n",
                        );
                        continue;
                    };
                    let signaler = Arc::clone(&signaler);
                    if let Err(error) =
                        thread::Builder::new()
                            .name("4dgs-http".into())
                            .spawn(move || {
                                let _permit = permit;
                                if let Err(error) = handle_http(stream, &signaler) {
                                    eprintln!("HTTP request: {error:#}");
                                }
                            })
                    {
                        eprintln!("spawn HTTP worker: {error}");
                    }
                }
                Err(error) => eprintln!("HTTP accept: {error}"),
            }
        }
    }))
}

const MAX_HTTP_CONNECTIONS: usize = 16;
const MAX_REQUEST_LINE_BYTES: usize = 2 * 1024;
const MAX_HEADER_BLOCK_BYTES: usize = 16 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 4 * 1024;
const MAX_HEADER_COUNT: usize = 64;
const MAX_OFFER_BODY_BYTES: usize = 64 * 1024;
const MAX_OFFER_SDP_BYTES: usize = 60 * 1024;
const HTTP_READ_DEADLINE: Duration = Duration::from_secs(5);
const HTTP_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

struct ConnectionPermit(Arc<AtomicUsize>);

impl ConnectionPermit {
    fn try_acquire(active: &Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_HTTP_CONNECTIONS).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(active)))
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        let previous = self.0.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct HttpRequestHead {
    method: String,
    path: String,
    content_length: usize,
}

fn remaining_before(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .context("HTTP request deadline exceeded")
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn loopback_authority(authority: &str) -> bool {
    if authority == "localhost" || authority == "[::1]" {
        return true;
    }
    if let Some(port) = authority.strip_prefix("localhost:") {
        return port.parse::<u16>().is_ok();
    }
    authority
        .parse::<std::net::SocketAddr>()
        .is_ok_and(|address| address.ip().is_loopback())
        || authority
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn parse_http_head(bytes: &[u8]) -> Result<HttpRequestHead> {
    ensure!(
        bytes.len() <= MAX_HEADER_BLOCK_BYTES,
        "HTTP header block exceeds {MAX_HEADER_BLOCK_BYTES} bytes"
    );
    ensure!(bytes.is_ascii(), "HTTP request head must be ASCII");
    let head = std::str::from_utf8(bytes).context("HTTP request head UTF-8")?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().context("missing HTTP request line")?;
    ensure!(
        request_line.len() <= MAX_REQUEST_LINE_BYTES,
        "HTTP request line exceeds {MAX_REQUEST_LINE_BYTES} bytes"
    );
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("missing HTTP method")?;
    let target = parts.next().context("missing HTTP target")?;
    let version = parts.next().context("missing HTTP version")?;
    ensure!(parts.next().is_none(), "malformed HTTP request line");
    ensure!(version == "HTTP/1.1", "only HTTP/1.1 is supported");
    ensure!(target.starts_with('/'), "HTTP target must use origin form");
    let path = target.split('?').next().unwrap_or(target);

    let mut headers = HashMap::new();
    let mut header_count = 0_usize;
    for line in lines {
        ensure!(!line.is_empty(), "unexpected empty HTTP header line");
        ensure!(
            line.len() <= MAX_HEADER_LINE_BYTES,
            "HTTP header line exceeds {MAX_HEADER_LINE_BYTES} bytes"
        );
        header_count += 1;
        ensure!(
            header_count <= MAX_HEADER_COUNT,
            "HTTP request exceeds {MAX_HEADER_COUNT} headers"
        );
        ensure!(
            !line.starts_with(' ') && !line.starts_with('\t'),
            "folded HTTP headers are not supported"
        );
        let (name, value) = line.split_once(':').context("malformed HTTP header")?;
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)),
            "malformed HTTP header name"
        );
        let name = name.to_ascii_lowercase();
        ensure!(
            headers
                .insert(name.clone(), value.trim().to_owned())
                .is_none(),
            "duplicate HTTP header {name}"
        );
    }
    ensure!(
        !headers.contains_key("transfer-encoding"),
        "Transfer-Encoding is not supported"
    );
    let host = headers.get("host").context("missing Host header")?;
    ensure!(
        loopback_authority(host),
        "Host must be a loopback authority"
    );

    let content_length = match headers.get("content-length") {
        Some(value) => {
            ensure!(
                !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
                "malformed Content-Length"
            );
            value.parse::<usize>().context("Content-Length overflow")?
        }
        None => 0,
    };
    ensure!(
        content_length <= MAX_OFFER_BODY_BYTES,
        "HTTP body exceeds {MAX_OFFER_BODY_BYTES} bytes"
    );

    match (method, path) {
        ("GET", _) => ensure!(content_length == 0, "GET requests cannot contain a body"),
        ("POST", "/offer") => {
            ensure!(
                headers.contains_key("content-length"),
                "POST /offer requires Content-Length"
            );
            let content_type = headers
                .get("content-type")
                .context("POST /offer requires Content-Type")?
                .split(';')
                .next()
                .unwrap_or_default()
                .trim();
            ensure!(
                content_type.eq_ignore_ascii_case("application/json"),
                "POST /offer requires application/json"
            );
            let origin = headers
                .get("origin")
                .context("POST /offer requires Origin")?;
            ensure!(
                origin == &format!("http://{host}"),
                "Origin must match Host"
            );
        }
        _ => ensure!(
            content_length == 0,
            "unsupported requests cannot contain a body"
        ),
    }

    Ok(HttpRequestHead {
        method: method.to_owned(),
        path: path.to_owned(),
        content_length,
    })
}

fn read_http_request(stream: &mut TcpStream, deadline: Instant) -> Result<HttpRequest> {
    let mut bytes = Vec::with_capacity(2048);
    let header_end = loop {
        if let Some(end) = find_header_end(&bytes) {
            ensure!(
                end + 4 <= MAX_HEADER_BLOCK_BYTES,
                "HTTP header block exceeds {MAX_HEADER_BLOCK_BYTES} bytes"
            );
            break end;
        }
        ensure!(
            bytes.len() < MAX_HEADER_BLOCK_BYTES,
            "HTTP header block exceeds {MAX_HEADER_BLOCK_BYTES} bytes"
        );
        stream.set_read_timeout(Some(remaining_before(deadline)?))?;
        let mut chunk = [0_u8; 2048];
        let capacity = (MAX_HEADER_BLOCK_BYTES - bytes.len()).min(chunk.len());
        let count = stream
            .read(&mut chunk[..capacity])
            .context("read HTTP request head")?;
        ensure!(count > 0, "connection closed before HTTP request head");
        bytes.extend_from_slice(&chunk[..count]);
    };

    let head = parse_http_head(&bytes[..header_end])?;
    let mut body = bytes[header_end + 4..].to_vec();
    ensure!(
        body.len() <= head.content_length,
        "HTTP request contains trailing or pipelined bytes"
    );
    while body.len() < head.content_length {
        stream.set_read_timeout(Some(remaining_before(deadline)?))?;
        let remaining = head.content_length - body.len();
        let mut chunk = [0_u8; 4096];
        let capacity = remaining.min(chunk.len());
        let count = stream
            .read(&mut chunk[..capacity])
            .context("read HTTP request body")?;
        ensure!(count > 0, "connection closed before HTTP request body");
        body.extend_from_slice(&chunk[..count]);
    }
    Ok(HttpRequest {
        method: head.method,
        path: head.path,
        body,
    })
}

fn handle_http(mut stream: TcpStream, signaler: &WebRtcSignaler) -> Result<()> {
    stream.set_write_timeout(Some(HTTP_WRITE_TIMEOUT))?;
    let request = read_http_request(&mut stream, Instant::now() + HTTP_READ_DEADLINE)?;
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => {
            let html = INDEX_HTML.replace("__CLIENT_BUILD__", &client_build());
            response(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                html.as_bytes(),
            )
        }
        ("GET", "/client.js") => response(
            &mut stream,
            200,
            "text/javascript; charset=utf-8",
            CLIENT_JS.as_bytes(),
        ),
        ("GET", "/status") => {
            // Keep the render-thread status mutex only long enough to clone a
            // snapshot; JSON serialization must not share the per-frame lock.
            let snapshot = signaler.status.lock().unwrap().clone();
            let json = serde_json::to_vec(&snapshot)?;
            response(&mut stream, 200, "application/json", &json)
        }
        ("POST", "/offer") => {
            let offer: Offer = serde_json::from_slice(&request.body).context("parse offer JSON")?;
            ensure!(
                offer.sdp.len() <= MAX_OFFER_SDP_BYTES,
                "SDP offer exceeds {MAX_OFFER_SDP_BYTES} bytes"
            );
            if offer.client_build.as_deref() != Some(client_build().as_str())
                || offer.client_protocol != Some(CLIENT_PROTOCOL)
            {
                return response(
                    &mut stream,
                    409,
                    "application/json",
                    br#"{"error":"stale client; reload required"}"#,
                );
            }
            match signaler.answer_offer(&offer.sdp)? {
                OfferOutcome::Answer(sdp) => {
                    let json = serde_json::to_vec(&Answer {
                        type_: "answer",
                        sdp: &sdp,
                    })?;
                    response(&mut stream, 200, "application/json", &json)
                }
                OfferOutcome::Busy => {
                    // A fresh Negotiating owner and a fresh Live owner are both
                    // authoritative. The loser may retry, but must never kill
                    // the owner merely because its DataChannel is not open yet.
                    response(
                        &mut stream,
                        409,
                        "application/json",
                        br#"{"error":"another preview page owns the active WebRTC session"}"#,
                    )
                }
                OfferOutcome::Restarting => response(
                    &mut stream,
                    503,
                    "application/json",
                    br#"{"error":"WebRTC session is restarting"}"#,
                ),
            }
        }
        _ => response(
            &mut stream,
            404,
            "text/plain; charset=utf-8",
            b"not found\n",
        ),
    }
}

fn response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        409 => "Conflict",
        503 => "Service Unavailable",
        404 => "Not Found",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'unsafe-inline'; media-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\nReferrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\nCross-Origin-Resource-Policy: same-origin\r\nPermissions-Policy: camera=(), microphone=(), geolocation=()\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_head(authority: &str) -> Vec<u8> {
        format!("GET /status HTTP/1.1\r\nHost: {authority}").into_bytes()
    }

    fn offer_head(authority: &str, origin: Option<&str>, extra: &str) -> Vec<u8> {
        let origin = origin
            .map(|value| format!("\r\nOrigin: {value}"))
            .unwrap_or_default();
        format!(
            "POST /offer HTTP/1.1\r\nHost: {authority}{origin}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: 2{extra}"
        )
        .into_bytes()
    }

    #[test]
    fn http_host_gate_accepts_loopback_and_ssh_forward_authorities() {
        for authority in [
            "localhost",
            "localhost:4192",
            "127.0.0.1",
            "127.0.0.1:4192",
            "[::1]",
            "[::1]:4192",
        ] {
            assert!(
                parse_http_head(&get_head(authority)).is_ok(),
                "rejected {authority}"
            );
        }
        for authority in ["example.test", "192.0.2.1:4191", "0.0.0.0:4191"] {
            assert!(
                parse_http_head(&get_head(authority)).is_err(),
                "accepted {authority}"
            );
        }
    }

    #[test]
    fn http_offer_requires_exact_same_origin_and_json() {
        assert!(
            parse_http_head(&offer_head(
                "127.0.0.1:4192",
                Some("http://127.0.0.1:4192"),
                ""
            ))
            .is_ok()
        );
        for origin in [None, Some("null"), Some("http://example.test")] {
            assert!(parse_http_head(&offer_head("127.0.0.1:4192", origin, "")).is_err());
        }
        let wrong_type = b"POST /offer HTTP/1.1\r\nHost: 127.0.0.1:4192\r\nOrigin: http://127.0.0.1:4192\r\nContent-Type: text/plain\r\nContent-Length: 2";
        assert!(parse_http_head(wrong_type).is_err());
    }

    #[test]
    fn http_parser_rejects_ambiguous_framing_and_body_abuse() {
        let duplicate_host = b"GET / HTTP/1.1\r\nHost: localhost\r\nHost: 127.0.0.1";
        let duplicate_length = b"POST /offer HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2";
        let transfer_encoding = b"GET / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked";
        let get_body = b"GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1";
        for request in [
            duplicate_host.as_slice(),
            duplicate_length.as_slice(),
            transfer_encoding.as_slice(),
            get_body.as_slice(),
        ] {
            assert!(parse_http_head(request).is_err());
        }
    }

    #[test]
    fn http_parser_enforces_every_declared_size_boundary() {
        let prefix = "GET ";
        let suffix = " HTTP/1.1";
        let path = format!(
            "/{}",
            "a".repeat(MAX_REQUEST_LINE_BYTES - prefix.len() - suffix.len() - 1)
        );
        let exact_line = format!("GET {path} HTTP/1.1\r\nHost: localhost");
        assert_eq!(
            exact_line.split("\r\n").next().unwrap().len(),
            MAX_REQUEST_LINE_BYTES
        );
        assert!(parse_http_head(exact_line.as_bytes()).is_ok());
        let over_line = exact_line.replacen(&path, &format!("{path}a"), 1);
        assert!(parse_http_head(over_line.as_bytes()).is_err());

        let mut exact_count = String::from("GET / HTTP/1.1\r\nHost: localhost");
        for index in 0..MAX_HEADER_COUNT - 1 {
            exact_count.push_str(&format!("\r\nX-{index}: v"));
        }
        assert!(parse_http_head(exact_count.as_bytes()).is_ok());
        exact_count.push_str("\r\nX-over: v");
        assert!(parse_http_head(exact_count.as_bytes()).is_err());

        let exact_body = format!(
            "POST /offer HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nContent-Length: {MAX_OFFER_BODY_BYTES}"
        );
        assert!(parse_http_head(exact_body.as_bytes()).is_ok());
        let over_body = exact_body.replace(
            &MAX_OFFER_BODY_BYTES.to_string(),
            &(MAX_OFFER_BODY_BYTES + 1).to_string(),
        );
        assert!(parse_http_head(over_body.as_bytes()).is_err());

        assert!(parse_http_head(&vec![b'a'; MAX_HEADER_BLOCK_BYTES + 1]).is_err());
        let long_header = format!(
            "GET / HTTP/1.1\r\nHost: localhost\r\nX-Pad: {}",
            "a".repeat(MAX_HEADER_LINE_BYTES)
        );
        assert!(parse_http_head(long_header.as_bytes()).is_err());
    }

    #[test]
    fn http_connection_limit_is_atomic_and_recoverable() {
        let active = Arc::new(AtomicUsize::new(0));
        let permits = (0..MAX_HTTP_CONNECTIONS)
            .map(|_| ConnectionPermit::try_acquire(&active).unwrap())
            .collect::<Vec<_>>();
        assert!(ConnectionPermit::try_acquire(&active).is_none());
        assert_eq!(active.load(Ordering::Acquire), MAX_HTTP_CONNECTIONS);
        drop(permits);
        assert_eq!(active.load(Ordering::Acquire), 0);
        assert!(ConnectionPermit::try_acquire(&active).is_some());
    }

    #[test]
    fn http_read_uses_one_absolute_deadline() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.write_all(b"GET /").unwrap();
        let started = Instant::now();
        assert!(read_http_request(&mut server, started + Duration::from_millis(30)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn gstreamer_promise_wait_is_bounded_by_the_shared_deadline() {
        gst::init().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let promise = gst::Promise::with_change_func(move |reply| {
            let _ = sender.send(reply.map(|_| ()).map_err(|error| format!("{error:?}")));
        });
        let started = Instant::now();
        let error = receive_promise_before(
            receiver,
            &promise,
            started + Duration::from_millis(30),
            "test promise",
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn media_pipeline_is_bounded_and_uses_safe_defaults() {
        let config = MediaConfig {
            bitrate_kbps: DEFAULT_VIDEO_BITRATE_KBPS,
            keyframe_interval_frames: DEFAULT_KEYFRAME_INTERVAL_FRAMES,
            video_slices: DEFAULT_VIDEO_SLICES,
            // Production defaults to a 0.5 s CPB floor.
            cpb_size_kbits: minimum_cpb_size_kbits(DEFAULT_VIDEO_BITRATE_KBPS),
            webrtc_video_priority: DEFAULT_WEBRTC_VIDEO_PRIORITY.to_owned(),
            experimental_lan_pacer: false,
            nice_max_bitrate_bps: DEFAULT_NICE_PACER_BPS,
            nice_pacer_burst_bytes: DEFAULT_NICE_PACER_BURST_BYTES,
        };
        let primary_ssrc = 0x1234_5678;
        let pipeline = media_pipeline_description(&config, primary_ssrc);
        assert!(pipeline.contains("max-buffers=1"));
        assert!(pipeline.contains("leaky-type=downstream"));
        assert!(pipeline.contains("bitrate=6000"));
        assert!(pipeline.contains("cpb-length=500"));
        assert!(pipeline.contains("keyframe-period=600"));
        assert!(pipeline.contains("num-slices=4"));
        assert!(pipeline.contains("quality-level=4"));
        assert!(pipeline.contains("format=NV12,colorimetry=bt709"));
        assert!(pipeline.contains(
            "capssetter replace=false caps=video/x-h264,colorimetry=bt709,chroma-site=jpeg"
        ));
        assert!(pipeline.contains("rtph264pay name=payloader"));
        assert!(pipeline.contains("ssrc=(uint)305419896"));
        assert_eq!(config.nice_max_bitrate_bps, 100_000_000);
        assert_eq!(config.nice_pacer_burst_bytes, 8_192);
        assert_eq!(DEFAULT_WEBRTC_VIDEO_PRIORITY, "inherit");
        assert_eq!(config.webrtc_video_priority, "inherit");
        assert!(!config.experimental_lan_pacer);
        assert_eq!(
            cpb_length_ms(config.cpb_size_kbits, config.bitrate_kbps),
            500
        );
    }

    #[test]
    fn answer_recovery_gate_requires_nack_rtx_and_consistent_ssrcs() {
        const PRIMARY_SSRC: u32 = 0x1234_5678;
        const RTX_SSRC: u32 = 0x9abc_def0;
        let complete = concat!(
            "m=video 9 UDP/TLS/RTP/SAVPF 108 109\r\n",
            "a=rtpmap:108 H264/90000\r\n",
            "a=rtcp-fb:108 nack\r\n",
            "a=rtcp-fb:108 nack pli\r\n",
            "a=rtpmap:109 rtx/90000\r\n",
            "a=fmtp:109 apt=108\r\n",
            "a=ssrc-group:FID 305419896 2596069104\r\n",
            "a=ssrc:305419896 cname:primary\r\n",
            "a=ssrc:2596069104 cname:rtx\r\n",
        );
        let complete_capabilities = inspect_answer_recovery_capabilities(complete);
        assert_eq!(
            complete_capabilities,
            AnswerRecoveryCapabilities {
                generic_nack: true,
                rtx_payload_type: Some(109),
                fid_primary_ssrc: Some(PRIMARY_SSRC),
                fid_rtx_ssrc: Some(RTX_SSRC),
                primary_ssrc_advertised: true,
                rtx_ssrc_advertised: true,
            }
        );
        assert!(complete_capabilities.has_valid_ssrc_association(PRIMARY_SSRC));
        assert!(!complete_capabilities.has_valid_ssrc_association(PRIMARY_SSRC + 1));

        let pli_only = concat!(
            "m=video 9 UDP/TLS/RTP/SAVPF 108\r\n",
            "a=rtcp-fb:108 nack pli\r\n",
            "a=rtcp-fb:108 ccm fir\r\n",
        );
        assert_eq!(
            inspect_answer_recovery_capabilities(pli_only),
            AnswerRecoveryCapabilities {
                generic_nack: false,
                rtx_payload_type: None,
                ..Default::default()
            }
        );

        let wrong_apt = concat!(
            "m=video 9 UDP/TLS/RTP/SAVPF 108 109\r\n",
            "a=rtcp-fb:108 nack\r\n",
            "a=rtpmap:109 rtx/90000\r\n",
            "a=fmtp:109 apt=107\r\n",
        );
        assert_eq!(
            inspect_answer_recovery_capabilities(wrong_apt),
            AnswerRecoveryCapabilities {
                generic_nack: true,
                rtx_payload_type: None,
                ..Default::default()
            }
        );

        let non_video_decoy = concat!(
            "m=audio 9 UDP/TLS/RTP/SAVPF 108 109\r\n",
            "a=rtcp-fb:108 nack\r\n",
            "a=rtpmap:109 rtx/90000\r\n",
            "a=fmtp:109 apt=108\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 108\r\n",
            "a=rtpmap:108 H264/90000\r\n",
        );
        assert_eq!(
            inspect_answer_recovery_capabilities(non_video_decoy),
            AnswerRecoveryCapabilities {
                generic_nack: false,
                rtx_payload_type: None,
                ..Default::default()
            }
        );

        let zero_primary = concat!(
            "m=video 9 UDP/TLS/RTP/SAVPF 108 109\r\n",
            "a=rtcp-fb:108 nack\r\n",
            "a=rtpmap:109 rtx/90000\r\n",
            "a=fmtp:109 apt=108\r\n",
            "a=ssrc-group:FID 0 2596069104\r\n",
            "a=ssrc:2596069104 cname:rtx\r\n",
        );
        let zero_primary_capabilities = inspect_answer_recovery_capabilities(zero_primary);
        assert!(!zero_primary_capabilities.has_valid_ssrc_association(PRIMARY_SSRC));
        assert_eq!(zero_primary_capabilities.fid_primary_ssrc, Some(0));
        assert!(!zero_primary_capabilities.primary_ssrc_advertised);
        assert!(zero_primary_capabilities.rtx_ssrc_advertised);

        let same_ssrc = complete.replace("2596069104", "305419896");
        assert!(
            !inspect_answer_recovery_capabilities(&same_ssrc)
                .has_valid_ssrc_association(PRIMARY_SSRC)
        );

        let zero_rtx = complete.replace("2596069104", "0");
        assert!(
            !inspect_answer_recovery_capabilities(&zero_rtx)
                .has_valid_ssrc_association(PRIMARY_SSRC)
        );

        let missing_primary_declaration =
            complete.replace("a=ssrc:305419896 cname:primary\r\n", "");
        assert!(
            !inspect_answer_recovery_capabilities(&missing_primary_declaration)
                .has_valid_ssrc_association(PRIMARY_SSRC)
        );

        let missing_rtx_declaration = complete.replace("a=ssrc:2596069104 cname:rtx\r\n", "");
        assert!(
            !inspect_answer_recovery_capabilities(&missing_rtx_declaration)
                .has_valid_ssrc_association(PRIMARY_SSRC)
        );
    }

    #[test]
    fn answer_recovery_inspection_does_not_mix_video_sections() {
        let first_video_without_recovery = concat!(
            "m=video 9 UDP/TLS/RTP/SAVPF 108\r\n",
            "a=rtpmap:108 H264/90000\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 108 109\r\n",
            "a=rtcp-fb:108 nack\r\n",
            "a=rtpmap:109 rtx/90000\r\n",
            "a=fmtp:109 apt=108\r\n",
            "a=ssrc-group:FID 305419896 2596069104\r\n",
            "a=ssrc:305419896 cname:primary\r\n",
            "a=ssrc:2596069104 cname:rtx\r\n",
        );
        assert_eq!(
            inspect_answer_recovery_capabilities(first_video_without_recovery),
            AnswerRecoveryCapabilities::default()
        );
    }

    #[test]
    fn primary_ssrc_entropy_never_uses_gstreamer_sentinels() {
        assert_eq!(primary_ssrc_from_entropy([0; 4]), 1);
        assert_eq!(primary_ssrc_from_entropy([0xff; 4]), u32::MAX - 1);
        assert_eq!(
            primary_ssrc_from_entropy(0x1234_5678_u32.to_le_bytes()),
            0x1234_5678
        );
    }

    #[test]
    fn media_environment_values_are_strictly_bounded() {
        assert_eq!(
            parse_bounded_u32("TEST", "8000", 1_000, 50_000).unwrap(),
            8_000
        );
        assert!(parse_bounded_u32("TEST", "999", 1_000, 50_000).is_err());
        assert!(parse_bounded_u32("TEST", "50001", 1_000, 50_000).is_err());
        assert!(parse_bounded_u32("TEST", "not-a-number", 1_000, 50_000).is_err());
        assert!(parse_bounded_u32("TEST", "79999999", 80_000_000, 100_000_000).is_err());
        assert_eq!(minimum_cpb_size_kbits(6_000), 3_000);
        assert_eq!(minimum_cpb_size_kbits(6_001), 3_001);
        assert_eq!(DEFAULT_NICE_PACER_BPS, 100_000_000);
        assert_eq!(parse_webrtc_video_priority("inherit").unwrap(), None);
        assert_eq!(
            parse_webrtc_video_priority("very-low").unwrap(),
            Some(WebRTCPriorityType::VeryLow)
        );
        assert_eq!(
            parse_webrtc_video_priority("low").unwrap(),
            Some(WebRTCPriorityType::Low)
        );
        assert_eq!(
            parse_webrtc_video_priority("medium").unwrap(),
            Some(WebRTCPriorityType::Medium)
        );
        assert_eq!(
            parse_webrtc_video_priority("high").unwrap(),
            Some(WebRTCPriorityType::High)
        );
        assert_eq!(
            parse_webrtc_video_priority(DEFAULT_WEBRTC_VIDEO_PRIORITY).unwrap(),
            None
        );
        assert!(parse_webrtc_video_priority("HIGH").is_err());
        assert_eq!(
            parse_webrtc_video_priority("invalid")
                .unwrap_err()
                .to_string(),
            "PHI_WEBRTC_VIDEO_PRIORITY must be one of inherit, very-low, low, medium, high; got \"invalid\""
        );
        assert_eq!(
            duration_for_wire_bytes(65_536, 80_000_000),
            Duration::from_nanos(6_553_600)
        );
    }

    #[test]
    fn latency_recalculation_requests_are_bounded_and_accounted() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let counters = LatencyRecalculationCounters::default();
        let mut metrics = TransportMetrics::default();

        let scheduled = schedule_latency_recalculation(&sender);
        let coalesced = schedule_latency_recalculation(&sender);
        record_latency_message(&counters, scheduled);
        record_latency_message(&counters, coalesced);
        overlay_latency_message_counters(&mut metrics, &counters);
        assert_eq!(scheduled, LatencyRecalculationSchedule::Scheduled);
        assert_eq!(coalesced, LatencyRecalculationSchedule::Coalesced);
        assert_eq!(metrics.gstreamer_latency_messages, 2);
        assert_eq!(metrics.gstreamer_latency_recalc_coalesced, 1);
        assert_eq!(metrics.gstreamer_latency_recalc_schedule_failures, 0);
        assert_eq!(metrics.gstreamer_latency_recalc_failures, 0);

        drop(receiver);
        let disconnected = schedule_latency_recalculation(&sender);
        record_latency_message(&counters, disconnected);
        overlay_latency_message_counters(&mut metrics, &counters);
        assert_eq!(disconnected, LatencyRecalculationSchedule::Disconnected);
        assert_eq!(metrics.gstreamer_latency_messages, 3);
        assert_eq!(metrics.gstreamer_latency_recalc_schedule_failures, 1);
        assert_eq!(metrics.gstreamer_latency_recalc_failures, 0);

        record_latency_recalculation(&mut metrics, Duration::from_millis(3), true);
        record_latency_recalculation(&mut metrics, Duration::from_millis(7), false);
        record_latency_recalculation(&mut metrics, Duration::from_millis(2), true);
        assert_eq!(metrics.gstreamer_latency_recalc_successes, 2);
        assert_eq!(metrics.gstreamer_latency_recalc_failures, 1);
        assert_eq!(metrics.gstreamer_latency_recalc_last_ms, 2.0);
        assert_eq!(metrics.gstreamer_latency_recalc_max_ms, 7.0);
    }

    #[test]
    fn packet_pacer_reservations_are_controllable() {
        let now = Instant::now();
        let mut pacer = PacketPacer::new_at(now, 80_000_000, 8_192);
        let burst_duration = Duration::from_nanos(819_200);

        assert_eq!(pacer.reserve_at(now, 8_192), Duration::ZERO);
        assert_eq!(pacer.reserve_at(now, 8_192), Duration::ZERO);
        assert_eq!(pacer.reserve_at(now, 8_192), burst_duration);
        assert_eq!(
            pacer.reserve_at(now + burst_duration, 8_192),
            burst_duration
        );
    }

    #[test]
    fn pacer_wait_metrics_distinguish_requested_actual_and_overshoot() {
        let mut metrics = TransportMetrics::default();
        record_pacer_wait(
            &mut metrics,
            classify_pacer_wait(Duration::from_millis(10), Duration::from_millis(14)),
        );
        record_pacer_wait(
            &mut metrics,
            classify_pacer_wait(Duration::from_millis(8), Duration::from_millis(7)),
        );

        // A zero request means no sleep call. Even if a caller supplies a
        // non-zero probe duration, classification reports an exact zero.
        let no_sleep = classify_pacer_wait(Duration::ZERO, Duration::from_millis(99));
        assert_eq!(no_sleep.actual_wait, Duration::ZERO);
        assert_eq!(no_sleep.sleep_overshoot, Duration::ZERO);
        assert!(!no_sleep.slept);
        record_pacer_wait(&mut metrics, no_sleep);

        assert_eq!(metrics.nice_pacer_sleep_count, 2);
        assert_eq!(metrics.nice_pacer_requested_wait_ms, 0.0);
        assert_eq!(metrics.nice_pacer_actual_wait_ms, 0.0);
        assert_eq!(metrics.nice_pacer_sleep_overshoot_ms, 0.0);
        assert_eq!(metrics.nice_pacer_requested_wait_max_ms, 10.0);
        assert_eq!(metrics.nice_pacer_actual_wait_max_ms, 14.0);
        assert_eq!(metrics.nice_pacer_sleep_overshoot_max_ms, 4.0);
        assert_eq!(metrics.nice_pacer_requested_wait_total_ms, 18.0);
        assert_eq!(metrics.nice_pacer_actual_wait_total_ms, 21.0);
        assert_eq!(metrics.nice_pacer_sleep_overshoot_total_ms, 4.0);
        assert_eq!(
            metrics.nice_pacer_wait_ms,
            metrics.nice_pacer_requested_wait_ms
        );
        assert_eq!(
            metrics.nice_pacer_wait_max_ms,
            metrics.nice_pacer_requested_wait_max_ms
        );
    }

    #[test]
    fn cadence_offsets_use_one_exact_owner_and_preserve_explicit_skips() {
        assert_eq!(cadence_offset_ns(0, 60), 0);
        assert_eq!(cadence_offset_ns(1, 60), 16_666_666);
        assert_eq!(cadence_offset_ns(2, 60), 33_333_333);
        assert_eq!(cadence_offset_ns(3, 60), 50_000_000);
        assert_eq!(cadence_offset_ns(60, 60), 1_000_000_000);

        // A scheduler slot jump is represented exactly, while GPU completion
        // jitter is no longer an input to the media timestamp function.
        assert_eq!(
            cadence_offset_ns(8, 60) - cadence_offset_ns(5, 60),
            50_000_000
        );
    }

    #[test]
    fn access_unit_delta_flag_is_the_keyframe_boundary() {
        assert!(encoded_access_unit_is_keyframe(gst::BufferFlags::empty()));
        assert!(encoded_access_unit_is_keyframe(gst::BufferFlags::MARKER));
        assert!(!encoded_access_unit_is_keyframe(
            gst::BufferFlags::DELTA_UNIT
        ));
        assert!(!encoded_access_unit_is_keyframe(
            gst::BufferFlags::DELTA_UNIT | gst::BufferFlags::MARKER
        ));
    }

    #[test]
    fn periodic_keyframes_do_not_fabricate_force_key_unit_recovery() {
        let start = Instant::now();
        let mut tracker = KeyframeRecoveryTracker::default();
        assert_eq!(tracker.note_keyframe(start), None);
        assert_eq!(tracker.pending_requests(), 0);
    }

    #[test]
    fn force_key_unit_retries_coalesce_into_the_next_keyframe() {
        let start = Instant::now();
        let mut tracker = KeyframeRecoveryTracker::default();
        assert_eq!(
            tracker.note_request(start, KeyframeRequestSource::OtherManual),
            1
        );
        assert_eq!(
            tracker.note_request(
                start + Duration::from_millis(10),
                KeyframeRequestSource::OtherManual,
            ),
            2
        );
        assert_eq!(
            tracker.note_keyframe(start + Duration::from_millis(35)),
            Some(KeyframeRecovery {
                requests: 2,
                feedback_coverage: 0,
                rtcp_feedback_requests: 0,
                feedback_fallback_requests: 0,
                other_manual_requests: 2,
                latency: Duration::from_millis(35),
            })
        );
        assert_eq!(tracker.pending_requests(), 0);
        assert_eq!(
            tracker.note_keyframe(start + Duration::from_millis(40)),
            None
        );
    }

    #[test]
    fn manual_recovery_cannot_mask_a_later_feedback_recovery() {
        let start = Instant::now();
        let mut tracker = KeyframeRecoveryTracker::default();
        tracker.note_request(start, KeyframeRequestSource::OtherManual);
        let manual = tracker
            .note_keyframe(start + Duration::from_millis(20))
            .unwrap();
        assert_eq!(manual.requests, 1);
        assert_eq!(manual.feedback_coverage, 0);
        assert_eq!(manual.other_manual_requests, 1);

        tracker.note_request(
            start + Duration::from_millis(30),
            KeyframeRequestSource::RtcpFeedback,
        );
        let feedback = tracker
            .note_keyframe(start + Duration::from_millis(50))
            .unwrap();
        assert_eq!(feedback.requests, 1);
        assert_eq!(feedback.feedback_coverage, 1);
        assert_eq!(feedback.rtcp_feedback_requests, 1);
        assert_eq!(feedback.other_manual_requests, 0);
    }

    #[test]
    fn feedback_fallback_covers_a_missing_native_feedback_batch_once() {
        let start = Instant::now();
        let mut tracker = KeyframeRecoveryTracker::default();
        tracker.note_request(
            start,
            KeyframeRequestSource::FeedbackFallback { coverage: 3 },
        );
        tracker.note_request(
            start + Duration::from_millis(10),
            KeyframeRequestSource::FeedbackFallback { coverage: 3 },
        );
        let recovery = tracker
            .note_keyframe(start + Duration::from_millis(30))
            .unwrap();
        assert_eq!(recovery.requests, 2);
        assert_eq!(recovery.feedback_fallback_requests, 2);
        assert_eq!(recovery.feedback_coverage, 3);
    }

    #[test]
    fn feedback_fallback_does_not_double_count_native_feedback() {
        let start = Instant::now();
        let mut tracker = KeyframeRecoveryTracker::default();
        tracker.note_request(start, KeyframeRequestSource::RtcpFeedback);
        tracker.note_request(
            start + Duration::from_millis(10),
            KeyframeRequestSource::FeedbackFallback { coverage: 1 },
        );
        let recovery = tracker
            .note_keyframe(start + Duration::from_millis(30))
            .unwrap();
        assert_eq!(recovery.requests, 2);
        assert_eq!(recovery.rtcp_feedback_requests, 1);
        assert_eq!(recovery.feedback_fallback_requests, 1);
        assert_eq!(recovery.feedback_coverage, 1);
    }

    #[test]
    fn keyframe_recovery_bypasses_a_saturated_common_control_queue() {
        let (control_tx, control_rx) = mpsc::sync_channel(1);
        let queued = r#"{"type":"set-playing","value":true}"#;
        control_tx.try_send(queued.to_owned()).unwrap();
        let latest_keyframe_request = Arc::new(Mutex::new(None));
        let request = r#"{"type":"keyframe-request","connection_generation":7,"active_ssrc":99,"request_id":3,"last_frames_received":3010,"client_time_ms":42.0,"reason":"frame-stall"}"#;

        assert_eq!(
            route_incoming_control(
                DataChannelRole::Config,
                request,
                &control_tx,
                &latest_keyframe_request,
            ),
            IncomingControlRoute::PriorityStored
        );
        assert_eq!(control_rx.try_recv().unwrap(), queued);
        let stored = latest_keyframe_request.lock().unwrap().take();
        assert!(matches!(
            stored,
            Some(ControlMessage::KeyframeRequest {
                connection_generation: 7,
                active_ssrc: 99,
                request_id: 3,
                last_frames_received: 3010,
                ..
            })
        ));
    }

    #[test]
    fn data_channel_labels_and_message_categories_are_closed() {
        assert_eq!(
            DataChannelRole::from_label(Some("control")),
            Some(DataChannelRole::Control)
        );
        assert_eq!(
            DataChannelRole::from_label(Some("config")),
            Some(DataChannelRole::Config)
        );
        assert_eq!(DataChannelRole::from_label(None), None);
        assert_eq!(DataChannelRole::from_label(Some("diagnostics")), None);

        let camera = ControlMessage::CameraState {
            epoch: 1,
            sequence: 1,
            client_time_ms: 1.0,
            orbit_x: 0.0,
            orbit_y: 0.0,
            zoom: 0.0,
        };
        let progress = ControlMessage::ReceiverProgress {
            progress: ReceiverProgress::default(),
        };
        assert!(DataChannelRole::Control.accepts(&camera));
        assert!(DataChannelRole::Config.accepts(&camera));
        assert!(DataChannelRole::Control.accepts(&progress));
        assert!(DataChannelRole::Config.accepts(&progress));
        assert!(!DataChannelRole::Control.accepts(&ControlMessage::SetPlaying { value: true }));
        assert!(!DataChannelRole::Config.accepts(&ControlMessage::Orbit { dx: 1.0, dy: 2.0 }));
    }

    #[test]
    fn incoming_control_rejects_oversize_malformed_and_wrong_channel_messages() {
        let (control_tx, control_rx) = mpsc::sync_channel(4);
        let latest_keyframe_request = Arc::new(Mutex::new(None));
        let oversized = "x".repeat(MAX_INBOUND_DATA_CHANNEL_STRING_BYTES + 1);
        assert_eq!(
            route_incoming_control(
                DataChannelRole::Control,
                &oversized,
                &control_tx,
                &latest_keyframe_request,
            ),
            IncomingControlRoute::Oversized
        );
        assert_eq!(
            route_incoming_control(
                DataChannelRole::Control,
                "{not-json}",
                &control_tx,
                &latest_keyframe_request,
            ),
            IncomingControlRoute::Malformed
        );
        assert_eq!(
            route_incoming_control(
                DataChannelRole::Control,
                r#"{"type":"set-playing","value":true}"#,
                &control_tx,
                &latest_keyframe_request,
            ),
            IncomingControlRoute::WrongChannel
        );
        assert_eq!(
            route_incoming_control(
                DataChannelRole::Control,
                r#"{"type":"keyframe-request","connection_generation":7,"active_ssrc":99,"request_id":3,"last_frames_received":3010,"client_time_ms":42.0,"reason":"frame-stall"}"#,
                &control_tx,
                &latest_keyframe_request,
            ),
            IncomingControlRoute::WrongChannel
        );
        assert!(control_rx.try_recv().is_err());
        assert!(latest_keyframe_request.lock().unwrap().is_none());
    }

    #[test]
    fn rejected_incoming_messages_are_dropped_without_renewing_the_lease() {
        for route in [
            IncomingControlRoute::Oversized,
            IncomingControlRoute::Malformed,
            IncomingControlRoute::WrongChannel,
        ] {
            let start = Instant::now();
            let opened = start + Duration::from_millis(5);
            let lifecycle = Mutex::new(SessionLifecycle::default());
            {
                let mut lifecycle = lifecycle.lock().unwrap();
                assert_eq!(lifecycle.claim_offer(start), OfferDisposition::Claimed);
                assert!(lifecycle.open_control(opened));
            }
            let status = Mutex::new(LiveStatus::default());
            account_incoming_control_route(
                route,
                opened + Duration::from_secs(2),
                &status,
                &lifecycle,
            );
            assert_eq!(status.lock().unwrap().controls_dropped, 1);
            assert!(
                lifecycle
                    .lock()
                    .unwrap()
                    .stream_ready(opened + CLIENT_HEARTBEAT_TIMEOUT - Duration::from_nanos(1))
            );
            assert!(
                lifecycle
                    .lock()
                    .unwrap()
                    .restart_requested(opened + CLIENT_HEARTBEAT_TIMEOUT)
            );
        }
    }

    #[test]
    fn accepted_control_renews_lease_and_dead_consumer_restarts() {
        let start = Instant::now();
        let opened = start + Duration::from_millis(5);
        let lifecycle = Mutex::new(SessionLifecycle::default());
        {
            let mut lifecycle = lifecycle.lock().unwrap();
            assert_eq!(lifecycle.claim_offer(start), OfferDisposition::Claimed);
            assert!(lifecycle.open_control(opened));
        }
        let status = Mutex::new(LiveStatus::default());
        let renewed = opened + Duration::from_secs(2);
        account_incoming_control_route(IncomingControlRoute::Queued, renewed, &status, &lifecycle);
        assert!(
            lifecycle
                .lock()
                .unwrap()
                .stream_ready(opened + CLIENT_HEARTBEAT_TIMEOUT)
        );

        account_incoming_control_route(
            IncomingControlRoute::QueueDisconnected,
            renewed + Duration::from_millis(1),
            &status,
            &lifecycle,
        );
        assert_eq!(status.lock().unwrap().controls_dropped, 1);
        assert!(
            lifecycle
                .lock()
                .unwrap()
                .restart_requested(renewed + Duration::from_millis(1))
        );
    }

    #[test]
    fn negotiating_owner_rejects_duplicates_without_requesting_restart() {
        let start = Instant::now();
        let mut lifecycle = SessionLifecycle::default();
        assert_eq!(lifecycle.claim_offer(start), OfferDisposition::Claimed);
        assert_eq!(
            lifecycle.claim_offer(start + Duration::from_millis(1)),
            OfferDisposition::Busy
        );
        assert!(
            !lifecycle
                .restart_requested(start + CLIENT_HANDSHAKE_TIMEOUT - Duration::from_nanos(1))
        );
        assert!(lifecycle.complete_answer(start + Duration::from_secs(1)));
    }

    #[test]
    fn handshake_deadline_covers_the_entire_claimed_offer_path() {
        let start = Instant::now();
        let mut lifecycle = SessionLifecycle::default();
        assert_eq!(lifecycle.claim_offer(start), OfferDisposition::Claimed);
        assert!(lifecycle.restart_requested(start + CLIENT_HANDSHAKE_TIMEOUT));
        assert_eq!(
            lifecycle.claim_offer(start + CLIENT_HANDSHAKE_TIMEOUT),
            OfferDisposition::Restarting
        );
        assert!(!lifecycle.open_control(start + CLIENT_HANDSHAKE_TIMEOUT));
    }

    #[test]
    fn live_owner_renews_lease_and_rejects_duplicates() {
        let start = Instant::now();
        let mut lifecycle = SessionLifecycle::default();
        assert_eq!(lifecycle.claim_offer(start), OfferDisposition::Claimed);
        let opened = start + Duration::from_millis(5);
        assert!(lifecycle.open_control(opened));
        assert!(lifecycle.stream_ready(opened));
        assert_eq!(
            lifecycle.claim_offer(opened + Duration::from_millis(1)),
            OfferDisposition::Busy
        );

        let renewed = opened + Duration::from_secs(2);
        lifecycle.record_activity(renewed);
        assert!(
            !lifecycle
                .restart_requested(renewed + CLIENT_HEARTBEAT_TIMEOUT - Duration::from_nanos(1))
        );
        assert!(lifecycle.restart_requested(renewed + CLIENT_HEARTBEAT_TIMEOUT));
    }

    #[test]
    fn live_lease_keeps_streaming_during_a_transient_transport_recovery() {
        let start = Instant::now();
        let mut lifecycle = SessionLifecycle::default();
        assert_eq!(lifecycle.claim_offer(start), OfferDisposition::Claimed);
        let opened = start + Duration::from_millis(5);
        assert!(lifecycle.open_control(opened));

        // Display-only peer state is deliberately not part of this authority:
        // a short disconnected/connecting recovery must preserve continuous
        // appsrc PTS until the control lease itself actually expires.
        assert!(
            lifecycle.stream_ready(opened + CLIENT_HEARTBEAT_TIMEOUT - Duration::from_nanos(1))
        );
        assert!(!lifecycle.stream_ready(opened + CLIENT_HEARTBEAT_TIMEOUT));
    }
}
