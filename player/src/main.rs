mod asset;
mod camera_control;
mod external_image;
mod media;
mod renderer;
mod shader;
mod streaming;

use std::{
    fs,
    io::Write as _,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use asset::Asset;
use camera_control::{
    CameraController, ControlMessage, RECEIVER_PROGRESS_SCHEMA, RECEIVER_TELEMETRY_SCHEMA,
    ReceiverProgress, epoch_is_newer,
};
use renderer::StreamRenderer;
use streaming::{LiveStatus, RecoveryMetrics, StreamTransport, StreamTransportConfig};

const CONTROL_QUEUE_CAPACITY: usize = 128;
const CONTROL_CONFIG_QUEUE_CAPACITY: usize = 32;
const RESTART_EXIT_CODE: i32 = 75;
const INTERACTION_QUALITY_HOLD: Duration = Duration::from_millis(500);
const SCHEDULER_SPIN_THRESHOLD: Duration = Duration::from_millis(2);
const LOD_ALPHA_STEP: f32 = 2.0 / 255.0;
// Bound emergency LOD so overload recovery cannot discard progressively more
// opacity without limit. Every asset/profile still needs its own image gate.
const LOD_MAX_ALPHA: f32 = 10.0 / 255.0;
const LOD_GPU_ESCALATE_RATIO: f64 = 0.78;
const LOD_GPU_RECOVER_RATIO: f64 = 0.65;
const LOD_OVERLOAD_HOLD_FRAMES: u32 = 60;
const LOD_RECOVER_STABLE_FRAMES: u32 = 45;
const KEYFRAME_FEEDBACK_GRACE: Duration = Duration::from_millis(250);
const KEYFRAME_RECOVERY_TIMEOUT: Duration = Duration::from_millis(500);
const KEYFRAME_FEEDBACK_RETRY: Duration = Duration::from_secs(1);
// Receiver telemetry arrives once per second. If RTP packets continue to
// arrive but no complete video frame is assembled for one sample interval,
// request an IDR before the browser's media-progress watchdog tears down the
// whole WebRTC session. Periodic GOP and feedback recovery remain independent.
const RECEIVER_PROGRESS_KEYFRAME_DELAY: Duration = Duration::from_millis(750);
const PINNED_RUST_TOOLCHAIN: &str = "1.95.0";
const BUILD_RUSTC_VERSION: &str = env!("PHI_BUILD_RUSTC_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReceiverFeedbackSession {
    connection_generation: u64,
    active_ssrc: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct BrowserRecoveryFeedback {
    progress_schema: u32,
    sample_time_ms: f64,
    connection_generation: u64,
    active_ssrc: u32,
    frames_received: u64,
    packets_received: u64,
    pli_count: u64,
    fir_count: u64,
}

impl From<&ReceiverProgress> for BrowserRecoveryFeedback {
    fn from(progress: &ReceiverProgress) -> Self {
        Self {
            progress_schema: progress.progress_schema,
            sample_time_ms: progress.sample_time_ms,
            connection_generation: progress.connection_generation,
            active_ssrc: progress.active_ssrc,
            frames_received: progress.frames_received,
            packets_received: progress.packets_received,
            pli_count: progress.pli_count,
            fir_count: progress.fir_count,
        }
    }
}

#[derive(Debug, Default)]
struct KeyframeFeedbackWatchdog {
    session: Option<ReceiverFeedbackSession>,
    browser_base: u64,
    native_base: u64,
    native_recovered_base: u64,
    pending_since: Option<Instant>,
    retry_not_before: Option<Instant>,
}

impl KeyframeFeedbackWatchdog {
    fn observe(
        &mut self,
        now: Instant,
        browser: &BrowserRecoveryFeedback,
        native: &RecoveryMetrics,
    ) -> Option<u64> {
        let session = ReceiverFeedbackSession {
            connection_generation: browser.connection_generation,
            active_ssrc: browser.active_ssrc,
        };
        let browser_requests = browser.pli_count.saturating_add(browser.fir_count);
        let native_requests = native.force_key_unit_requests;
        let native_recovered_requests = native.feedback_force_key_unit_requests_recovered;
        if self.session != Some(session)
            || browser_requests < self.browser_base
            || native_requests < self.native_base
            || native_recovered_requests < self.native_recovered_base
        {
            self.session = Some(session);
            self.browser_base = browser_requests;
            self.native_base = native_requests;
            self.native_recovered_base = native_recovered_requests;
            self.pending_since = None;
            self.retry_not_before = None;
            return None;
        }

        let expected = browser_requests.saturating_sub(self.browser_base);
        // This counter advances only when the encoded-output probe observes a
        // non-delta access unit for outstanding ForceKeyUnit requests. Event
        // acceptance alone is deliberately not considered recovery.
        let recovered = native_recovered_requests.saturating_sub(self.native_recovered_base);
        if expected <= recovered {
            self.pending_since = None;
            self.retry_not_before = None;
            return None;
        }
        if self.retry_not_before.is_some_and(|deadline| now < deadline) {
            return None;
        }
        let pending_since = *self.pending_since.get_or_insert(now);
        let native_coverage = native_requests.saturating_sub(self.native_base);
        let wait = if native_coverage >= expected {
            KEYFRAME_RECOVERY_TIMEOUT
        } else {
            KEYFRAME_FEEDBACK_GRACE
        };
        (now.saturating_duration_since(pending_since) >= wait).then_some(expected - recovered)
    }

    fn record_fallback(&mut self, now: Instant, _uncovered: u64, _accepted: bool) {
        // Both accepted and rejected requests remain pending until the encoded
        // output probe observes a keyframe. Limit retries to one per second.
        self.retry_not_before = Some(now + KEYFRAME_FEEDBACK_RETRY);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverRecoverySource {
    Client,
    ProgressFallback,
    FeedbackFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutstandingReceiverRecovery {
    source: ReceiverRecoverySource,
    frames_at_request: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverProgressAction {
    RequestKeyframe(ReceiverRecoverySource),
    Recovered(ReceiverRecoverySource),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientRecoveryAction {
    RequestKeyframe,
    RecoveredThenRequestKeyframe(ReceiverRecoverySource),
    Coalesced,
    Rejected,
}

#[derive(Debug, Default)]
struct ReceiverProgressWatchdog {
    session: Option<ReceiverFeedbackSession>,
    last_sample_time_ms: Option<f64>,
    last_frames_received: u64,
    last_packets_received: u64,
    last_frame_progress_at: Option<Instant>,
    last_client_request_id: u64,
    outstanding: Option<OutstandingReceiverRecovery>,
}

impl ReceiverProgressWatchdog {
    fn reset(&mut self, now: Instant, stats: &BrowserRecoveryFeedback) {
        self.session = Some(ReceiverFeedbackSession {
            connection_generation: stats.connection_generation,
            active_ssrc: stats.active_ssrc,
        });
        self.last_sample_time_ms = Some(stats.sample_time_ms);
        self.last_frames_received = stats.frames_received;
        self.last_packets_received = stats.packets_received;
        self.last_frame_progress_at = Some(now);
        self.last_client_request_id = 0;
        self.outstanding = None;
    }

    fn session_is_compatible(a: ReceiverFeedbackSession, b: ReceiverFeedbackSession) -> bool {
        a.connection_generation == b.connection_generation
            && (a.active_ssrc == 0 || b.active_ssrc == 0 || a.active_ssrc == b.active_ssrc)
    }

    fn begin_client_request(
        &mut self,
        now: Instant,
        connection_generation: u64,
        active_ssrc: u32,
        request_id: u64,
        last_frames_received: u64,
    ) -> ClientRecoveryAction {
        if request_id == 0 {
            return ClientRecoveryAction::Rejected;
        }
        let requested_session = ReceiverFeedbackSession {
            connection_generation,
            active_ssrc,
        };
        match self.session {
            Some(current) if connection_generation < current.connection_generation => {
                return ClientRecoveryAction::Rejected;
            }
            Some(current) if Self::session_is_compatible(current, requested_session) => {
                if current.active_ssrc == 0 && active_ssrc != 0 {
                    self.session = Some(requested_session);
                }
            }
            Some(current) if connection_generation == current.connection_generation => {
                return ClientRecoveryAction::Rejected;
            }
            Some(_) | None => {
                self.session = Some(requested_session);
                self.last_sample_time_ms = None;
                self.last_frames_received = last_frames_received;
                self.last_packets_received = 0;
                self.last_frame_progress_at = Some(now);
                self.last_client_request_id = 0;
                self.outstanding = None;
            }
        }
        if request_id <= self.last_client_request_id {
            return ClientRecoveryAction::Coalesced;
        }
        self.last_client_request_id = request_id;
        if last_frames_received < self.last_frames_received {
            return ClientRecoveryAction::Coalesced;
        }
        // A reliable client request can prove the previous recovery before the
        // next one-second progress sample reaches this state machine. A newer
        // request with a higher frame count is therefore both the completion
        // signal for the old stall and the start of a distinct new stall.
        let recovered = self
            .outstanding
            .filter(|request| last_frames_received > request.frames_at_request)
            .map(|request| request.source);
        if recovered.is_some() {
            self.outstanding = None;
            self.last_frame_progress_at = Some(now);
        } else if self.outstanding.is_some() {
            return ClientRecoveryAction::Coalesced;
        }
        self.last_frames_received = last_frames_received;
        self.outstanding = Some(OutstandingReceiverRecovery {
            source: ReceiverRecoverySource::Client,
            frames_at_request: last_frames_received,
        });
        recovered.map_or(
            ClientRecoveryAction::RequestKeyframe,
            ClientRecoveryAction::RecoveredThenRequestKeyframe,
        )
    }

    fn begin_fallback_request(
        &mut self,
        source: ReceiverRecoverySource,
        frames_at_request: u64,
    ) -> bool {
        if self.outstanding.is_some() {
            return false;
        }
        self.outstanding = Some(OutstandingReceiverRecovery {
            source,
            frames_at_request,
        });
        true
    }

    fn observe(
        &mut self,
        now: Instant,
        stats: &BrowserRecoveryFeedback,
    ) -> Option<ReceiverProgressAction> {
        if !stats.sample_time_ms.is_finite() {
            return None;
        }
        let session = ReceiverFeedbackSession {
            connection_generation: stats.connection_generation,
            active_ssrc: stats.active_ssrc,
        };
        if let Some(current) = self.session {
            if stats.connection_generation < current.connection_generation {
                return None;
            }
            if Self::session_is_compatible(current, session) {
                if current.active_ssrc == 0 && session.active_ssrc != 0 {
                    self.session = Some(session);
                }
            } else {
                self.reset(now, stats);
                return None;
            }
        } else {
            self.reset(now, stats);
            return None;
        }
        let Some(last_sample_time_ms) = self.last_sample_time_ms else {
            self.last_sample_time_ms = Some(stats.sample_time_ms);
            self.last_packets_received = stats.packets_received;
            let recovery = self
                .outstanding
                .filter(|request| stats.frames_received > request.frames_at_request);
            self.last_frames_received = self.last_frames_received.max(stats.frames_received);
            if let Some(recovery) = recovery {
                self.last_frame_progress_at = Some(now);
                self.outstanding = None;
                return Some(ReceiverProgressAction::Recovered(recovery.source));
            }
            return None;
        };
        if stats.sample_time_ms < last_sample_time_ms
            || stats.packets_received < self.last_packets_received
        {
            return None;
        }
        // The render loop reads one immutable status snapshot many times. Only
        // a fresh getStats sample may advance this state machine or request an
        // IDR, otherwise one stalled sample would produce a keyframe per frame.
        if stats.sample_time_ms <= last_sample_time_ms {
            return None;
        }

        let frames_advanced = stats.frames_received > self.last_frames_received;
        let packets_advanced = stats.packets_received > self.last_packets_received;
        self.last_sample_time_ms = Some(stats.sample_time_ms);
        self.last_frames_received = self.last_frames_received.max(stats.frames_received);
        self.last_packets_received = stats.packets_received;

        if frames_advanced {
            self.last_frame_progress_at = Some(now);
            return self
                .outstanding
                .take()
                .map(|request| ReceiverProgressAction::Recovered(request.source));
        }
        let stalled_long_enough = self.last_frame_progress_at.is_some_and(|progress| {
            now.saturating_duration_since(progress) >= RECEIVER_PROGRESS_KEYFRAME_DELAY
        });
        if packets_advanced
            && stalled_long_enough
            && self.begin_fallback_request(
                ReceiverRecoverySource::ProgressFallback,
                stats.frames_received,
            )
        {
            return Some(ReceiverProgressAction::RequestKeyframe(
                ReceiverRecoverySource::ProgressFallback,
            ));
        }
        None
    }

    fn record_request_result(&mut self, accepted: bool) {
        if !accepted {
            self.outstanding = None;
        }
    }
}

#[derive(Debug)]
struct AdaptiveLod {
    base_alpha_min: f32,
    alpha_min: f32,
    hold_frames: u32,
    stable_frames: u32,
    overload_events: u64,
    recovery_events: u64,
}

impl AdaptiveLod {
    fn new(base_alpha_min: f32) -> Self {
        Self {
            base_alpha_min,
            alpha_min: base_alpha_min,
            hold_frames: 0,
            stable_frames: 0,
            overload_events: 0,
            recovery_events: 0,
        }
    }

    fn alpha_for_frame(&mut self, interactive: bool, interaction_floor: f32) -> f32 {
        let floor = if interactive {
            interaction_floor
        } else {
            self.base_alpha_min
        };
        if self.alpha_min < floor {
            self.alpha_min = floor;
            self.stable_frames = 0;
        }
        self.alpha_min
    }

    fn observe_gpu(
        &mut self,
        gpu_ms: f64,
        frame_period_ms: f64,
        interactive: bool,
        interaction_floor: f32,
    ) {
        let floor = if interactive {
            interaction_floor
        } else {
            self.base_alpha_min
        };
        let overloaded = !gpu_ms.is_finite() || gpu_ms > frame_period_ms * LOD_GPU_ESCALATE_RATIO;
        if overloaded {
            let next = (self.alpha_min + LOD_ALPHA_STEP).min(LOD_MAX_ALPHA);
            if next > self.alpha_min + f32::EPSILON {
                self.alpha_min = next;
                self.overload_events += 1;
            }
            self.hold_frames = LOD_OVERLOAD_HOLD_FRAMES;
            self.stable_frames = 0;
            return;
        }
        if self.hold_frames > 0 {
            self.hold_frames -= 1;
            self.stable_frames = 0;
            return;
        }
        if gpu_ms < frame_period_ms * LOD_GPU_RECOVER_RATIO && self.alpha_min > floor + f32::EPSILON
        {
            self.stable_frames += 1;
            if self.stable_frames >= LOD_RECOVER_STABLE_FRAMES {
                let next = self.alpha_min - LOD_ALPHA_STEP;
                self.alpha_min = if next <= floor + f32::EPSILON {
                    floor
                } else {
                    next
                };
                self.recovery_events += 1;
                self.stable_frames = 0;
            }
        } else {
            self.stable_frames = 0;
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Linux remote reference player for explicit 4D Gaussian assets")]
struct Args {
    /// Explicit-v1 asset manifest. Required until a bundled synthetic sample is selected.
    #[arg(long)]
    manifest: PathBuf,
    /// Shader directory; defaults to ./shaders relative to the process working directory.
    #[arg(long)]
    shaders: Option<PathBuf>,
    #[arg(long, default_value_t = 640)]
    width: u32,
    #[arg(long, default_value_t = 360)]
    height: u32,
    /// Normalized render time; overrides manifest time.initial when supplied.
    #[arg(long)]
    time: Option<f32>,
    /// Reference image required in evidence mode (when --serve is absent).
    #[arg(long, conflicts_with_all = ["write_golden", "serve"])]
    golden: Option<PathBuf>,
    /// Create a new raw RGBA8 reference without overwriting an existing file.
    #[arg(long, conflicts_with_all = ["golden", "serve"])]
    write_golden: Option<PathBuf>,
    #[arg(long, default_value = "output", conflicts_with = "serve")]
    output_dir: PathBuf,
    /// Run the persistent remote renderer and WebRTC preview server.
    #[arg(long)]
    serve: bool,
    /// HTTP signaling address. v0.1 intentionally accepts loopback only.
    #[arg(long, default_value = "127.0.0.1", requires = "serve")]
    bind: IpAddr,
    #[arg(long, default_value_t = 4191, requires = "serve")]
    port: u16,
    #[arg(long, default_value_t = 30, requires = "serve")]
    fps: u32,
    #[arg(long, default_value_t = 3, requires = "serve")]
    slots: usize,
    #[arg(long, hide = true)]
    force_interactive: bool,
    #[arg(long, hide = true)]
    interaction_alpha_min: Option<f32>,
    #[arg(long, conflicts_with = "serve", hide = true)]
    raster_reference: bool,
    #[arg(long, requires = "serve", hide = true)]
    zoom_stress: bool,
}

#[derive(Debug, Serialize)]
struct ImageMetrics {
    policy: &'static str,
    min_psnr_db: f64,
    max_allowed_abs: u8,
    exact_match: bool,
    psnr_db: Option<f64>,
    max_abs: u8,
    mean_abs: f64,
    golden_sha256: String,
    actual_sha256: String,
    pass: bool,
}

#[derive(Debug, Serialize)]
struct Receipt<'a> {
    schema: &'static str,
    stage: &'static str,
    renderer: &'static str,
    source: SourceIdentity,
    asset: AssetIdentity<'a>,
    frame: &'a renderer::FrameResult,
    image: ImageMetrics,
    transport: TransportAudit,
}

#[derive(Debug, Serialize)]
struct ReferenceReceipt<'a> {
    schema: &'static str,
    stage: &'static str,
    renderer: &'static str,
    source: SourceIdentity,
    asset: AssetIdentity<'a>,
    frame: &'a renderer::FrameResult,
    reference: ReferenceImage,
}

#[derive(Debug, Serialize)]
struct AssetIdentity<'a> {
    name: &'a str,
    manifest_sha256: &'a str,
    geometry_sha256: &'a str,
    appearance_sha256: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct ReferenceImage {
    rgba8_bytes: usize,
    rgba8_sha256: String,
    review_status: &'static str,
}

#[derive(Debug, Deserialize)]
struct ReviewedReferenceReceipt {
    schema: String,
    asset: ReviewedAssetIdentity,
    frame: ReviewedFrameIdentity,
    reference: ReviewedReferenceImage,
}

#[derive(Debug, Deserialize)]
struct ReviewedAssetIdentity {
    name: String,
    manifest_sha256: String,
    geometry_sha256: String,
    appearance_sha256: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ReviewedFrameIdentity {
    width: u32,
    height: u32,
    time: f32,
}

#[derive(Debug, Deserialize)]
struct ReviewedReferenceImage {
    rgba8_bytes: u64,
    rgba8_sha256: String,
    review_status: String,
}

#[derive(Debug, Serialize)]
struct TransportAudit {
    browser_gpu_api: &'static str,
    gpu_to_cpu_readback: &'static str,
    render_target: &'static str,
    exported_handle: &'static str,
    note: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct SourceIdentity {
    package_version: &'static str,
    rust_toolchain: &'static str,
    git_commit: Option<&'static str>,
    native_source_sha256: String,
    shader_bundle_sha256: String,
    client_build: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        args.width > 0 && args.height > 0,
        "resolution must be non-zero"
    );
    ensure!(
        args.bind.is_loopback(),
        "v0.1 only serves on loopback; use an SSH tunnel for remote access"
    );
    if args.serve {
        return serve(args);
    }
    validate(args)
}

fn source_identity(shader_bundle_sha256: &str) -> SourceIdentity {
    let mut digest = Sha256::new();
    for source in [
        include_bytes!("asset.rs").as_slice(),
        include_bytes!("camera_control.rs").as_slice(),
        include_bytes!("external_image.rs").as_slice(),
        include_bytes!("main.rs").as_slice(),
        include_bytes!("media.rs").as_slice(),
        include_bytes!("renderer.rs").as_slice(),
        include_bytes!("shader.rs").as_slice(),
        include_bytes!("streaming.rs").as_slice(),
        include_bytes!("../web/client.js").as_slice(),
        include_bytes!("../web/index.html").as_slice(),
        include_bytes!("../build.rs").as_slice(),
        include_bytes!("../Cargo.toml").as_slice(),
        include_bytes!("../Cargo.lock").as_slice(),
        include_bytes!("../rust-toolchain.toml").as_slice(),
    ] {
        digest.update(source);
        digest.update([0]);
    }
    SourceIdentity {
        package_version: env!("CARGO_PKG_VERSION"),
        rust_toolchain: BUILD_RUSTC_VERSION,
        git_commit: option_env!("PHI_GIT_COMMIT").filter(|value| {
            (40..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        }),
        native_source_sha256: format!("{:x}", digest.finalize()),
        shader_bundle_sha256: shader_bundle_sha256.to_owned(),
        client_build: streaming::client_build(),
    }
}

fn asset_identity(asset: &Asset) -> AssetIdentity<'_> {
    AssetIdentity {
        name: &asset.manifest.name,
        manifest_sha256: &asset.manifest_sha256,
        geometry_sha256: &asset.sha256,
        appearance_sha256: asset.appearance_sha256.as_deref(),
    }
}

fn adjacent_receipt_path(raw_path: &Path) -> PathBuf {
    let mut receipt_name = raw_path.as_os_str().to_os_string();
    receipt_name.push(".json");
    PathBuf::from(receipt_name)
}

fn load_reviewed_golden(
    golden_path: &Path,
    expected_asset: &AssetIdentity<'_>,
    width: u32,
    height: u32,
    time: f32,
) -> Result<Vec<u8>> {
    let golden =
        fs::read(golden_path).with_context(|| format!("read golden {}", golden_path.display()))?;
    let expected_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .context("golden RGBA8 byte count overflow")?;
    ensure!(
        golden.len() as u64 == expected_bytes,
        "golden byte count {} does not match {}x{} RGBA8 ({expected_bytes})",
        golden.len(),
        width,
        height
    );

    let receipt_path = adjacent_receipt_path(golden_path);
    let receipt_bytes = fs::read(&receipt_path)
        .with_context(|| format!("read reference receipt {}", receipt_path.display()))?;
    let receipt: ReviewedReferenceReceipt = serde_json::from_slice(&receipt_bytes)
        .with_context(|| format!("parse reference receipt {}", receipt_path.display()))?;
    ensure!(
        receipt.schema == "phi.4dgs.remote-native.reference.v1",
        "reference receipt schema must be phi.4dgs.remote-native.reference.v1, got {}",
        receipt.schema
    );
    ensure!(
        receipt.reference.review_status == "REVIEWED",
        "reference receipt review_status must be REVIEWED, got {}",
        receipt.reference.review_status
    );
    ensure!(
        receipt.reference.rgba8_bytes == golden.len() as u64,
        "reference receipt RGBA8 byte count {} does not match golden {}",
        receipt.reference.rgba8_bytes,
        golden.len()
    );
    let golden_sha256 = format!("{:x}", Sha256::digest(&golden));
    ensure!(
        receipt.reference.rgba8_sha256 == golden_sha256,
        "reference receipt RGBA8 SHA-256 does not match golden"
    );
    ensure!(
        receipt.asset.name == expected_asset.name,
        "reference receipt asset name {} does not match current asset {}",
        receipt.asset.name,
        expected_asset.name
    );
    ensure!(
        receipt.asset.manifest_sha256 == expected_asset.manifest_sha256,
        "reference receipt asset manifest SHA-256 does not match current asset"
    );
    ensure!(
        receipt.asset.geometry_sha256 == expected_asset.geometry_sha256,
        "reference receipt geometry SHA-256 does not match current asset"
    );
    ensure!(
        receipt.asset.appearance_sha256.is_null() || receipt.asset.appearance_sha256.is_string(),
        "reference receipt appearance_sha256 must be a string or null"
    );
    ensure!(
        receipt.asset.appearance_sha256.as_str() == expected_asset.appearance_sha256,
        "reference receipt appearance SHA-256 does not match current asset"
    );
    ensure!(
        receipt.frame.width == width,
        "reference receipt frame width {} does not match requested {width}",
        receipt.frame.width
    );
    ensure!(
        receipt.frame.height == height,
        "reference receipt frame height {} does not match requested {height}",
        receipt.frame.height
    );
    ensure!(
        receipt.frame.time == time,
        "reference receipt frame time {} does not match requested {time}",
        receipt.frame.time
    );
    Ok(golden)
}

type LatestCameraControl = Arc<Mutex<Option<ControlMessage>>>;
type LatestKeyframeRequest = Arc<Mutex<Option<ControlMessage>>>;

fn start_control_router(
    control_rx: mpsc::Receiver<String>,
    status: Arc<Mutex<LiveStatus>>,
) -> Result<(LatestCameraControl, mpsc::Receiver<ControlMessage>)> {
    let latest_camera = Arc::new(Mutex::new(None));
    let latest_camera_thread = Arc::clone(&latest_camera);
    let (config_tx, config_rx) = mpsc::sync_channel(CONTROL_CONFIG_QUEUE_CAPACITY);
    thread::Builder::new()
        .name("4dgs-control-router".into())
        .spawn(move || {
            let mut last_epoch = None;
            let mut last_sequence = 0_u64;
            for json in control_rx {
                let message = match serde_json::from_str::<ControlMessage>(&json) {
                    Ok(message) => message,
                    Err(error) => {
                        eprintln!("ignore malformed control message: {error}");
                        status.lock().unwrap().controls_dropped += 1;
                        continue;
                    }
                };
                match message {
                    ControlMessage::ReceiverStats { stats } => {
                        let mut live = status.lock().unwrap();
                        if stats.telemetry_schema == RECEIVER_TELEMETRY_SCHEMA
                            && stats.stats_sample_time_ms.is_finite()
                        {
                            live.receiver_progress = ReceiverProgress::from(stats.as_ref());
                            live.browser = *stats;
                        } else {
                            live.controls_dropped += 1;
                        }
                    }
                    ControlMessage::ReceiverProgress { progress } => {
                        let mut live = status.lock().unwrap();
                        if progress.progress_schema == RECEIVER_PROGRESS_SCHEMA
                            && progress.sample_time_ms.is_finite()
                        {
                            live.receiver_progress = progress;
                        } else {
                            live.controls_dropped += 1;
                        }
                    }
                    message @ ControlMessage::CameraState {
                        epoch,
                        sequence,
                        client_time_ms,
                        ..
                    } => {
                        if let Some(current_epoch) = last_epoch
                            && epoch != current_epoch
                            && !epoch_is_newer(epoch, current_epoch)
                        {
                            continue;
                        }
                        if last_epoch != Some(epoch) {
                            last_epoch = Some(epoch);
                            last_sequence = 0;
                        } else if sequence <= last_sequence {
                            continue;
                        }
                        let mut live = status.lock().unwrap();
                        live.controls += 1;
                        live.input_sequence_gaps += sequence.saturating_sub(last_sequence + 1);
                        last_sequence = sequence;
                        live.input_age_ms = (unix_time_ms() - client_time_ms).clamp(0.0, 60_000.0);
                        drop(live);
                        *latest_camera_thread.lock().unwrap() = Some(message);
                    }
                    message => match config_tx.try_send(message) {
                        Ok(()) => status.lock().unwrap().controls += 1,
                        Err(mpsc::TrySendError::Full(_)) => {
                            status.lock().unwrap().controls_dropped += 1;
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => break,
                    },
                }
            }
        })
        .context("spawn control router")?;
    Ok((latest_camera, config_rx))
}

fn unix_time_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}

fn wait_until(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let remaining = deadline - now;
        if remaining > SCHEDULER_SPIN_THRESHOLD {
            thread::sleep(remaining - SCHEDULER_SPIN_THRESHOLD);
        } else {
            std::hint::spin_loop();
        }
    }
}

fn resolve_interaction_alpha_min(base_alpha_min: f32, override_value: Option<f32>) -> Result<f32> {
    let alpha_min =
        override_value.unwrap_or_else(|| base_alpha_min.max(renderer::INTERACTIVE_ALPHA_MIN));
    ensure!(
        alpha_min.is_finite() && alpha_min >= base_alpha_min && alpha_min < 1.0,
        "interaction alpha cutoff must be in [{base_alpha_min}, 1)"
    );
    Ok(alpha_min)
}

fn resolve_render_time(manifest_initial: f32, override_value: Option<f32>) -> Result<f32> {
    let time = override_value.unwrap_or(manifest_initial);
    ensure!(
        time.is_finite() && (0.0..=1.0).contains(&time),
        "time must be finite and in the normalized [0, 1] domain"
    );
    Ok(time)
}

fn validate(args: Args) -> Result<()> {
    ensure!(
        BUILD_RUSTC_VERSION == PINNED_RUST_TOOLCHAIN,
        "evidence mode requires rustc {PINNED_RUST_TOOLCHAIN}, but this binary was built with {BUILD_RUSTC_VERSION}"
    );
    let asset = Asset::load(&args.manifest)?;
    let time = resolve_render_time(asset.manifest.time.initial, args.time)?;
    let interaction_alpha_min =
        resolve_interaction_alpha_min(asset.manifest.policy.alpha_min, args.interaction_alpha_min)?;
    let reviewed_golden = args
        .golden
        .as_deref()
        .map(|path| {
            load_reviewed_golden(path, &asset_identity(&asset), args.width, args.height, time)
        })
        .transpose()?;
    ensure!(
        reviewed_golden.is_some() || args.write_golden.is_some(),
        "--golden is required in evidence mode"
    );
    let shader_dir = args.shaders.unwrap_or_else(|| PathBuf::from("shaders"));
    let alpha_min = if args.force_interactive {
        interaction_alpha_min
    } else {
        asset.manifest.policy.alpha_min
    };
    let result = pollster::block_on(renderer::render_once(
        &asset,
        &shader_dir,
        args.width,
        args.height,
        time,
        alpha_min,
        !args.raster_reference,
    ))?;
    fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("create {}", args.output_dir.display()))?;
    let actual_path = args.output_dir.join("native.rgba8");
    fs::write(&actual_path, &result.rgba8)
        .with_context(|| format!("write {}", actual_path.display()))?;
    if let Some(golden_path) = args.write_golden {
        let reference_receipt_path = adjacent_receipt_path(&golden_path);
        ensure!(
            !reference_receipt_path.exists(),
            "reference receipt {} already exists (existing files are never overwritten)",
            reference_receipt_path.display()
        );
        if let Some(parent) = golden_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("create golden directory {}", parent.display()))?;
        }
        let mut golden_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&golden_path)
            .with_context(|| {
                format!(
                    "create golden {} (existing files are never overwritten)",
                    golden_path.display()
                )
            })?;
        golden_file
            .write_all(&result.rgba8)
            .with_context(|| format!("write golden {}", golden_path.display()))?;
        golden_file.sync_all()?;
        let reference_sha256 = format!("{:x}", Sha256::digest(&result.rgba8));
        let reference_receipt = ReferenceReceipt {
            schema: "phi.4dgs.remote-native.reference.v1",
            stage: "native-vulkan-dmabuf-vaapi",
            renderer: "Rust/wgpu/WGSL/Vulkan",
            source: source_identity(&result.shader_bundle_sha256),
            asset: asset_identity(&asset),
            frame: &result,
            reference: ReferenceImage {
                rgba8_bytes: result.rgba8.len(),
                rgba8_sha256: reference_sha256.clone(),
                review_status: "UNREVIEWED",
            },
        };
        let reference_json = serde_json::to_string_pretty(&reference_receipt)? + "\n";
        let mut receipt_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&reference_receipt_path)
            .with_context(|| {
                format!(
                    "create reference receipt {} (existing files are never overwritten)",
                    reference_receipt_path.display()
                )
            })?;
        receipt_file
            .write_all(reference_json.as_bytes())
            .with_context(|| format!("write {}", reference_receipt_path.display()))?;
        receipt_file.sync_all()?;
        println!(
            "created {} ({} bytes, sha256 {}) and {} with review_status=UNREVIEWED; inspect the image before marking it REVIEWED",
            golden_path.display(),
            result.rgba8.len(),
            reference_sha256,
            reference_receipt_path.display()
        );
        println!("{reference_json}");
        return Ok(());
    }
    let golden = reviewed_golden.context("--golden is required in evidence mode")?;
    ensure!(
        golden.len() == result.rgba8.len(),
        "golden byte count {} != actual {}",
        golden.len(),
        result.rgba8.len()
    );
    let image = compare(
        &result.rgba8,
        &golden,
        alpha_min,
        asset.manifest.policy.alpha_min,
    );
    let receipt = Receipt {
        schema: "phi.4dgs.remote-native.receipt.v1",
        stage: "native-vulkan-dmabuf-vaapi",
        renderer: "Rust/wgpu/WGSL/Vulkan",
        source: source_identity(&result.shader_bundle_sha256),
        asset: asset_identity(&asset),
        frame: &result,
        image,
        transport: TransportAudit {
            browser_gpu_api: "none",
            gpu_to_cpu_readback: "validation-only",
            render_target: "ash-created Vulkan image wrapped by wgpu without a pixel copy",
            exported_handle: "DMA-BUF fd",
            note: "Full-frame pixel readback exists only for parity evidence; the streaming media path consumes the exported DMA-BUF.",
        },
    };
    let json = serde_json::to_string_pretty(&receipt)? + "\n";
    let receipt_path = args.output_dir.join("receipt.json");
    fs::write(&receipt_path, &json).with_context(|| format!("write {}", receipt_path.display()))?;
    println!("{json}");
    ensure!(
        receipt.image.pass,
        "image validation failed: PSNR {:.2} dB, max abs {}",
        receipt.image.psnr_db.unwrap_or(f64::INFINITY),
        receipt.image.max_abs
    );
    Ok(())
}

fn serve(args: Args) -> Result<()> {
    ensure!((1..=240).contains(&args.fps), "fps must be in 1..=240");
    ensure!(args.slots >= 2, "at least two DMA-BUF slots are required");
    let asset = Asset::load(&args.manifest)?;
    let time = resolve_render_time(asset.manifest.time.initial, args.time)?;
    let base_alpha_min = asset.manifest.policy.alpha_min;
    let interaction_alpha_min =
        resolve_interaction_alpha_min(base_alpha_min, args.interaction_alpha_min)?;
    let shader_dir = args.shaders.unwrap_or_else(|| PathBuf::from("shaders"));
    let mut renderer = pollster::block_on(StreamRenderer::new(
        &asset,
        &shader_dir,
        args.width,
        args.height,
        args.slots,
    ))?;
    ensure!(
        renderer.slot_count() == args.slots,
        "renderer slot count changed"
    );
    let output_layout = renderer.output_layout().clone();

    let status = Arc::new(Mutex::new(LiveStatus {
        state: "WAITING_FOR_BROWSER".into(),
        resolution: [args.width, args.height],
        transport: "wgpu/WGSL -> Vulkan DMA-BUF -> VA-API H.264 -> WebRTC".into(),
        peer: "NEW".into(),
        ice: "NEW".into(),
        control: "PENDING".into(),
        controls: 0,
        client_build: streaming::client_build(),
        ..Default::default()
    }));
    let (control_tx, control_rx) = mpsc::sync_channel(CONTROL_QUEUE_CAPACITY);
    let (latest_camera_control, config_rx) = start_control_router(control_rx, Arc::clone(&status))?;
    let latest_keyframe_request: LatestKeyframeRequest = Arc::new(Mutex::new(None));
    let mut transport = StreamTransport::new(
        StreamTransportConfig {
            layout: &output_layout,
            width: args.width,
            height: args.height,
            fps: args.fps,
            slots: args.slots,
        },
        control_tx,
        Arc::clone(&latest_keyframe_request),
        Arc::clone(&status),
    )?;
    let _http = streaming::start_http(args.bind, args.port, transport.signaler())?;

    let mut camera = CameraController::new(asset.manifest.camera.fixed.clone(), time);
    let frame_period = Duration::from_secs_f64(1.0 / args.fps as f64);
    let mut frame_id = 1_u64;
    let mut cadence_slot = 0_u64;
    let mut deadline_misses = 0_u64;
    let mut skipped_frames = 0_u64;
    let mut last_tick = Instant::now();
    let mut next_frame_at = Instant::now();
    let mut last_camera_input = Instant::now() - Duration::from_secs(1);
    let mut camera_orbit_updates_applied = 0_u64;
    let mut camera_zoom_updates_applied = 0_u64;
    let mut statistics_window = Instant::now();
    let mut statistics_frames = 0_u64;
    let zoom_stress_started = Instant::now();
    let mut adaptive_lod = AdaptiveLod::new(base_alpha_min);
    let mut keyframe_feedback_watchdog = KeyframeFeedbackWatchdog::default();
    let mut receiver_progress_watchdog = ReceiverProgressWatchdog::default();

    println!("remote renderer ready: http://{}:{}", args.bind, args.port);
    println!("browser role: video decode/display + DataChannel controls; WebGPU: none");
    println!("frame path: wgpu/WGSL -> Vulkan DMA-BUF -> VA-API H.264 -> WebRTC");

    loop {
        if transport.restart_requested() {
            println!("browser requested a fresh WebRTC session; restarting transport");
            // `process::exit` does not run destructors. Explicitly drop the
            // transport so its Drop implementation transitions the GStreamer
            // pipeline to NULL and closes the old RTP/SCTP session before the
            // supervisor starts a replacement process.
            drop(transport);
            std::process::exit(RESTART_EXIT_CODE);
        }
        if !transport.stream_ready() {
            thread::sleep(Duration::from_millis(20));
            next_frame_at = Instant::now();
            last_tick = next_frame_at;
            continue;
        }

        // Copy the compact 1 Hz receiver progress sample needed by the 30 Hz
        // recovery state machine. Full browser diagnostics are opt-in.
        let browser_feedback =
            { BrowserRecoveryFeedback::from(&status.lock().unwrap().receiver_progress) };
        let keyframe_request = { latest_keyframe_request.lock().unwrap().take() };
        if let Some(ControlMessage::KeyframeRequest {
            connection_generation,
            active_ssrc,
            request_id,
            last_frames_received,
            client_time_ms,
            reason,
        }) = keyframe_request
        {
            status
                .lock()
                .unwrap()
                .receiver_client_keyframe_requests_received += 1;
            let valid = client_time_ms.is_finite()
                && matches!(reason.as_str(), "first-frame" | "frame-stall");
            let action = valid.then(|| {
                receiver_progress_watchdog.begin_client_request(
                    Instant::now(),
                    connection_generation,
                    active_ssrc,
                    request_id,
                    last_frames_received,
                )
            });
            match action {
                Some(ClientRecoveryAction::RequestKeyframe)
                | Some(ClientRecoveryAction::RecoveredThenRequestKeyframe(_)) => {
                    if matches!(
                        action,
                        Some(ClientRecoveryAction::RecoveredThenRequestKeyframe(_))
                    ) {
                        status.lock().unwrap().receiver_stall_keyframe_recoveries += 1;
                    }
                    // A PLI/FIR may already have reached the encoder before the
                    // reliable application request. Let that in-flight IDR
                    // satisfy this stall instead of generating a duplicate.
                    if transport.recovery_metrics().pending_force_key_unit_requests > 0 {
                        status
                            .lock()
                            .unwrap()
                            .receiver_client_keyframe_requests_coalesced += 1;
                    } else {
                        let result = transport.force_key_unit();
                        receiver_progress_watchdog.record_request_result(result.is_ok());
                        let mut live = status.lock().unwrap();
                        if result.is_ok() {
                            live.receiver_stall_keyframe_requests += 1;
                            live.receiver_client_keyframe_requests_forced += 1;
                        } else {
                            live.receiver_stall_keyframe_request_errors += 1;
                        }
                        drop(live);
                        if let Err(error) = result {
                            eprintln!("client receiver-stall force-key-unit failed: {error:#}");
                        }
                    }
                }
                Some(ClientRecoveryAction::Coalesced) => {
                    status
                        .lock()
                        .unwrap()
                        .receiver_client_keyframe_requests_coalesced += 1;
                }
                Some(ClientRecoveryAction::Rejected) | None => {
                    status
                        .lock()
                        .unwrap()
                        .receiver_client_keyframe_requests_rejected += 1;
                }
            }
        }

        // GStreamer normally turns RTCP PLI/FIR into an upstream
        // GstForceKeyUnit event. Compare the independent receiver counter with
        // the event count observed at rtph264pay. Only inject a fallback after
        // a grace period, and arbitrate it with client/progress recovery so a
        // single damaged frame can produce at most one extra IDR.
        if browser_feedback.progress_schema == RECEIVER_PROGRESS_SCHEMA {
            let recovery_metrics = transport.recovery_metrics();
            if let Some(uncovered) = keyframe_feedback_watchdog.observe(
                Instant::now(),
                &browser_feedback,
                &recovery_metrics,
            ) {
                let requested_at = Instant::now();
                // `observe` already waited KEYFRAME_RECOVERY_TIMEOUT and proved
                // that no encoded IDR followed the receiver feedback. Retry the
                // encoder event even when a client/progress recovery owns the
                // same stall: that owner may itself have coalesced behind the
                // now-stale transport-pending request. The feedback watchdog's
                // one-second retry deadline bounds repeated retries.
                let owns_recovery = receiver_progress_watchdog.begin_fallback_request(
                    ReceiverRecoverySource::FeedbackFallback,
                    browser_feedback.frames_received,
                );
                let result = transport.force_key_unit_for_feedback_fallback(uncovered);
                if owns_recovery {
                    receiver_progress_watchdog.record_request_result(result.is_ok());
                }
                keyframe_feedback_watchdog.record_fallback(requested_at, uncovered, result.is_ok());
                if let Err(error) = result {
                    eprintln!("force-key-unit feedback fallback failed: {error:#}");
                }
            }
            match receiver_progress_watchdog.observe(Instant::now(), &browser_feedback) {
                Some(ReceiverProgressAction::RequestKeyframe(source))
                    if transport.recovery_metrics().pending_force_key_unit_requests == 0 =>
                {
                    // If RTCP PLI/FIR or the reliable client request already
                    // reached the encoder, keep the watchdog outstanding but
                    // let that in-flight IDR satisfy it. A second ForceKeyUnit
                    // here would make one damaged frame generate two large
                    // access-unit bursts on the same transport path.
                    let result = transport.force_key_unit();
                    receiver_progress_watchdog.record_request_result(result.is_ok());
                    let mut live = status.lock().unwrap();
                    if result.is_ok() {
                        live.receiver_stall_keyframe_requests += 1;
                        if source == ReceiverRecoverySource::ProgressFallback {
                            live.receiver_progress_fallback_requests_forced += 1;
                        }
                    } else {
                        live.receiver_stall_keyframe_request_errors += 1;
                    }
                    drop(live);
                    if let Err(error) = result {
                        eprintln!("receiver-stall force-key-unit failed: {error:#}");
                    }
                }
                Some(ReceiverProgressAction::RequestKeyframe(_)) => {}
                Some(ReceiverProgressAction::Recovered(_source)) => {
                    let mut live = status.lock().unwrap();
                    live.receiver_stall_keyframe_recoveries += 1;
                }
                None => {}
            }
        }

        wait_until(next_frame_at);
        let frame_started = Instant::now();
        let schedule_lateness = frame_started.saturating_duration_since(next_frame_at);
        let mut skipped_this_frame = 0_u64;
        if schedule_lateness >= frame_period {
            let skipped = (schedule_lateness.as_nanos() / frame_period.as_nanos()) as u64;
            skipped_frames += skipped;
            skipped_this_frame = skipped;
            next_frame_at += frame_period.mul_f64(skipped as f64);
        }
        cadence_slot = cadence_slot
            .checked_add(skipped_this_frame.saturating_add(1))
            .context("video cadence slot overflow")?;
        let loop_started = Instant::now();

        // Slot release can block, so preserve acquire-before-control ordering
        // and sample the newest cumulative camera state afterwards.
        let slot_wait_started = Instant::now();
        let (slot, completion) = transport.acquire_slot()?;
        let slot_wait = slot_wait_started.elapsed();

        let latest_message = { latest_camera_control.lock().unwrap().take() };
        if let Some(message) = latest_message {
            let result = camera.apply(message, args.width, args.height);
            camera_orbit_updates_applied =
                camera_orbit_updates_applied.saturating_add(u64::from(result.orbit_input));
            camera_zoom_updates_applied =
                camera_zoom_updates_applied.saturating_add(u64::from(result.zoom_input));
            if result.camera_input {
                last_camera_input = Instant::now();
            }
        }
        for message in config_rx.try_iter() {
            let result = camera.apply(message, args.width, args.height);
            camera_orbit_updates_applied =
                camera_orbit_updates_applied.saturating_add(u64::from(result.orbit_input));
            camera_zoom_updates_applied =
                camera_zoom_updates_applied.saturating_add(u64::from(result.zoom_input));
            if result.camera_input {
                last_camera_input = Instant::now();
            }
        }
        let now = Instant::now();
        let tick_seconds = now.duration_since(last_tick).as_secs_f32();
        camera.tick(tick_seconds);
        last_tick = now;

        if args.zoom_stress {
            let phase = zoom_stress_started.elapsed().as_secs_f32() % 4.0;
            let direction = if phase < 2.0 { -1.0 } else { 1.0 };
            camera.apply(
                ControlMessage::Zoom {
                    delta: direction * tick_seconds * 450.0,
                },
                args.width,
                args.height,
            );
            last_camera_input = Instant::now();
        }

        let slot_wait_ms = slot_wait.as_secs_f64() * 1000.0;
        let encode_ms = completion.as_ref().map_or(0.0, |value| value.encode_ms);

        let uniform = camera.uniform(args.width, args.height);
        let interactive = args.force_interactive
            || camera.is_settling()
            || last_camera_input.elapsed() < INTERACTION_QUALITY_HOLD;
        let alpha_min = adaptive_lod.alpha_for_frame(interactive, interaction_alpha_min);
        let frame = renderer.render(frame_id, slot, &uniform, camera.time, true, alpha_min)?;
        let push_started = Instant::now();
        transport.push(slot, cadence_slot, renderer.output(slot))?;
        let push_ms = push_started.elapsed().as_secs_f64() * 1000.0;
        statistics_frames += 1;

        let elapsed = statistics_window.elapsed().as_secs_f64();
        // Refresh aggregate transport metrics for /status once per second;
        // per-frame renderer fields remain current without serializing status.
        let refresh_status = elapsed >= 1.0;
        let fps = if refresh_status {
            statistics_frames as f64 / elapsed
        } else {
            status.lock().unwrap().fps
        };
        {
            let mut live = status.lock().unwrap();
            live.state = "STREAMING".into();
            live.frames = frame_id;
            live.fps = fps;
            live.render_ms = frame.render_wait_ms;
            live.preprocess_ms = frame.preprocess_ms;
            live.sort_ms = frame.sort_ms;
            live.tile_bin_ms = frame.tile_bin_ms;
            live.tile_render_ms = frame.tile_render_ms;
            live.splat_ms = frame.splat_ms;
            live.resolve_ms = frame.resolve_ms;
            live.gpu_ms = frame.gpu_ms;
            live.render_scale = frame.render_scale;
            live.lod_alpha_min = frame.lod_alpha_min;
            live.interaction_active = interactive;
            live.lod_overload_events = adaptive_lod.overload_events;
            live.lod_recovery_events = adaptive_lod.recovery_events;
            if encode_ms > 0.0 {
                live.encode_ms = encode_ms;
            }
            live.active = frame.counters.active;
            live.visible = frame.counters.visible;
            live.tile_overlaps = frame.counters.tile_overlaps;
            live.tile_overflow = frame.counters.tile_overflow;
            live.max_tile_load = frame.counters.max_tile_load;
            live.early_terminated_pixels = frame.counters.early_terminated_pixels;
            live.pixel_splat_tests = frame.counters.pixel_splat_tests;
            live.budget_limited_pixels = frame.counters.budget_limited_pixels;
            live.max_pixel_splat_tests = frame.counters.max_pixel_splat_tests;
            live.max_budget_remaining_transmittance =
                frame.counters.max_budget_remaining_transmittance;
            live.persistent_workload_flags = frame.counters.persistent_workload_flags;
            live.camera_distance = camera.distance();
            live.camera_target_distance = camera.target_distance();
            live.camera_orbit_updates_applied = camera_orbit_updates_applied;
            live.camera_zoom_updates_applied = camera_zoom_updates_applied;
            live.time = camera.time;
            live.slot_wait_ms = slot_wait_ms;
            live.push_ms = push_ms;
        }
        let frame_completed = Instant::now();
        adaptive_lod.observe_gpu(
            frame.gpu_ms,
            frame_period.as_secs_f64() * 1000.0,
            interactive,
            interaction_alpha_min,
        );
        next_frame_at += frame_period;
        if frame_completed > next_frame_at {
            deadline_misses += 1;
        }
        {
            let mut live = status.lock().unwrap();
            live.dropped = deadline_misses;
            live.deadline_misses = deadline_misses;
            live.skipped_frames = skipped_frames;
            live.schedule_lateness_ms = schedule_lateness.as_secs_f64() * 1000.0;
            live.loop_ms = loop_started.elapsed().as_secs_f64() * 1000.0;
        }
        if refresh_status {
            statistics_window = Instant::now();
            statistics_frames = 0;
            status.lock().unwrap().transport_metrics = transport.metrics();
        }

        frame_id += 1;
    }
}

fn compare(actual: &[u8], expected: &[u8], alpha_min: f32, base_alpha_min: f32) -> ImageMetrics {
    let mut squared = 0_f64;
    let mut absolute = 0_u64;
    let mut max_abs = 0_u8;
    for (&a, &b) in actual.iter().zip(expected) {
        let delta = a.abs_diff(b);
        max_abs = max_abs.max(delta);
        absolute += delta as u64;
        squared += (delta as f64) * (delta as f64);
    }
    let mse = squared / actual.len() as f64;
    let exact_match = mse == 0.0;
    let psnr_db = (!exact_match).then(|| 10.0 * ((255.0 * 255.0) / mse).log10());
    let interactive = alpha_min > base_alpha_min + f32::EPSILON;
    let (policy, min_psnr_db, max_allowed_abs) = if interactive {
        ("motion-preview", 30.0, 128)
    } else {
        ("reference-parity", 24.0, 96)
    };
    ImageMetrics {
        policy,
        min_psnr_db,
        max_allowed_abs,
        exact_match,
        psnr_db,
        max_abs,
        mean_abs: absolute as f64 / actual.len() as f64,
        golden_sha256: format!("{:x}", Sha256::digest(expected)),
        actual_sha256: format!("{:x}", Sha256::digest(actual)),
        pass: (exact_match || psnr_db.is_some_and(|value| value >= min_psnr_db))
            && max_abs <= max_allowed_abs,
    }
}

#[cfg(test)]
mod reviewed_reference_tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use super::{AssetIdentity, adjacent_receipt_path, load_reviewed_golden};

    const MANIFEST_SHA256: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    const GEOMETRY_SHA256: &str =
        "2222222222222222222222222222222222222222222222222222222222222222";
    const APPEARANCE_SHA256: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    const WIDTH: u32 = 2;
    const HEIGHT: u32 = 1;
    const TIME: f32 = 0.25;
    const GOLDEN: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "phi-reviewed-reference-{}-{}",
                std::process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn golden_path(&self) -> PathBuf {
            self.0.join("reference.rgba8")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn expected_asset() -> AssetIdentity<'static> {
        AssetIdentity {
            name: "test-asset",
            manifest_sha256: MANIFEST_SHA256,
            geometry_sha256: GEOMETRY_SHA256,
            appearance_sha256: Some(APPEARANCE_SHA256),
        }
    }

    fn reviewed_receipt() -> Value {
        json!({
            "schema": "phi.4dgs.remote-native.reference.v1",
            "asset": {
                "name": "test-asset",
                "manifest_sha256": MANIFEST_SHA256,
                "geometry_sha256": GEOMETRY_SHA256,
                "appearance_sha256": APPEARANCE_SHA256
            },
            "frame": {
                "width": WIDTH,
                "height": HEIGHT,
                "time": TIME
            },
            "reference": {
                "rgba8_bytes": GOLDEN.len(),
                "rgba8_sha256": format!("{:x}", Sha256::digest(GOLDEN)),
                "review_status": "REVIEWED"
            }
        })
    }

    fn write_case(directory: &TestDir, receipt: &Value) -> PathBuf {
        let golden_path = directory.golden_path();
        fs::write(&golden_path, GOLDEN).unwrap();
        fs::write(
            adjacent_receipt_path(&golden_path),
            serde_json::to_vec_pretty(receipt).unwrap(),
        )
        .unwrap();
        golden_path
    }

    fn rejection(receipt: &Value) -> String {
        let directory = TestDir::new();
        let golden_path = write_case(&directory, receipt);
        load_reviewed_golden(&golden_path, &expected_asset(), WIDTH, HEIGHT, TIME)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn accepts_matching_reviewed_reference() {
        let directory = TestDir::new();
        let golden_path = write_case(&directory, &reviewed_receipt());
        let loaded =
            load_reviewed_golden(&golden_path, &expected_asset(), WIDTH, HEIGHT, TIME).unwrap();
        assert_eq!(loaded, GOLDEN);
    }

    #[test]
    fn requires_adjacent_reference_receipt_with_reviewed_status() {
        let directory = TestDir::new();
        let golden_path = directory.golden_path();
        fs::write(&golden_path, GOLDEN).unwrap();
        let error = load_reviewed_golden(&golden_path, &expected_asset(), WIDTH, HEIGHT, TIME)
            .unwrap_err()
            .to_string();
        assert!(error.contains("read reference receipt"));

        let mut unreviewed = reviewed_receipt();
        unreviewed["reference"]["review_status"] = json!("UNREVIEWED");
        assert!(rejection(&unreviewed).contains("review_status must be REVIEWED"));

        let mut wrong_schema = reviewed_receipt();
        wrong_schema["schema"] = json!("phi.4dgs.remote-native.receipt.v1");
        assert!(rejection(&wrong_schema).contains("reference receipt schema"));
    }

    #[test]
    fn rejects_reference_byte_count_or_hash_mismatch() {
        let mut wrong_bytes = reviewed_receipt();
        wrong_bytes["reference"]["rgba8_bytes"] = json!(7);
        assert!(rejection(&wrong_bytes).contains("RGBA8 byte count"));

        let mut wrong_hash = reviewed_receipt();
        wrong_hash["reference"]["rgba8_sha256"] = json!(MANIFEST_SHA256);
        assert!(rejection(&wrong_hash).contains("RGBA8 SHA-256"));
    }

    #[test]
    fn rejects_reference_for_another_asset() {
        let cases = [
            ("name", json!("other-asset"), "asset name"),
            (
                "manifest_sha256",
                json!(GEOMETRY_SHA256),
                "manifest SHA-256",
            ),
            (
                "geometry_sha256",
                json!(MANIFEST_SHA256),
                "geometry SHA-256",
            ),
            ("appearance_sha256", Value::Null, "appearance SHA-256"),
        ];
        for (field, value, expected_error) in cases {
            let mut receipt = reviewed_receipt();
            receipt["asset"][field] = value;
            assert!(
                rejection(&receipt).contains(expected_error),
                "field {field}"
            );
        }

        let mut missing_appearance = reviewed_receipt();
        missing_appearance["asset"]
            .as_object_mut()
            .unwrap()
            .remove("appearance_sha256");
        assert!(rejection(&missing_appearance).contains("parse reference receipt"));

        let mut invalid_appearance = reviewed_receipt();
        invalid_appearance["asset"]["appearance_sha256"] = json!(7);
        assert!(
            rejection(&invalid_appearance).contains("appearance_sha256 must be a string or null")
        );
    }

    #[test]
    fn rejects_reference_for_another_frame() {
        let cases = [
            ("width", json!(3), "frame width"),
            ("height", json!(2), "frame height"),
            ("time", json!(0.5), "frame time"),
        ];
        for (field, value, expected_error) in cases {
            let mut receipt = reviewed_receipt();
            receipt["frame"][field] = value;
            assert!(
                rejection(&receipt).contains(expected_error),
                "field {field}"
            );
        }
    }

    #[test]
    fn rejects_raw_golden_with_wrong_dimensions_before_rendering() {
        let directory = TestDir::new();
        let golden_path = directory.golden_path();
        fs::write(&golden_path, &GOLDEN[..4]).unwrap();
        let error = load_reviewed_golden(
            Path::new(&golden_path),
            &expected_asset(),
            WIDTH,
            HEIGHT,
            TIME,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("does not match 2x1 RGBA8"));
    }
}

#[cfg(test)]
mod adaptive_lod_tests {
    use super::*;

    const FRAME_MS: f64 = 1000.0 / 60.0;
    const BASE_FLOOR: f32 = 2.0 / 255.0;
    const INTERACTION_FLOOR: f32 = 8.0 / 255.0;

    #[test]
    fn public_defaults_require_an_explicit_asset_and_use_loopback() {
        let args = Args::try_parse_from([
            "phi-4dgs-player",
            "--manifest",
            "sample/manifest.json",
            "--serve",
        ])
        .unwrap();
        assert_eq!(args.manifest, PathBuf::from("sample/manifest.json"));
        assert_eq!(args.golden, None);
        assert_eq!(args.write_golden, None);
        assert_eq!(args.shaders, None);
        assert_eq!(args.time, None);
        assert_eq!(args.interaction_alpha_min, None);
        assert!(args.bind.is_loopback());
        assert_eq!(args.port, 4191);
    }

    #[test]
    fn golden_creation_and_comparison_are_mutually_exclusive() {
        assert!(
            Args::try_parse_from([
                "phi-4dgs-player",
                "--manifest",
                "sample/manifest.json",
                "--golden",
                "old.rgba8",
                "--write-golden",
                "new.rgba8",
            ])
            .is_err()
        );
    }

    #[test]
    fn serve_rejects_evidence_only_arguments_during_cli_parsing() {
        for evidence_argument in [
            ["--golden", "reference.rgba8"],
            ["--write-golden", "reference.rgba8"],
            ["--raster-reference", ""],
            ["--output-dir", "output"],
        ] {
            let mut command = vec![
                "phi-4dgs-player",
                "--manifest",
                "sample/manifest.json",
                "--serve",
                evidence_argument[0],
            ];
            if !evidence_argument[1].is_empty() {
                command.push(evidence_argument[1]);
            }
            assert!(
                Args::try_parse_from(command).is_err(),
                "{} must not be accepted with --serve",
                evidence_argument[0]
            );
        }
    }

    #[test]
    fn zoom_stress_requires_serve_during_cli_parsing() {
        assert!(
            Args::try_parse_from([
                "phi-4dgs-player",
                "--manifest",
                "sample/manifest.json",
                "--zoom-stress",
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from([
                "phi-4dgs-player",
                "--manifest",
                "sample/manifest.json",
                "--serve",
                "--zoom-stress",
            ])
            .is_ok()
        );
    }

    #[test]
    fn explicitly_setting_server_arguments_requires_serve() {
        for server_argument in [
            ["--bind", "127.0.0.1"],
            ["--port", "4192"],
            ["--fps", "60"],
            ["--slots", "4"],
        ] {
            assert!(
                Args::try_parse_from([
                    "phi-4dgs-player",
                    "--manifest",
                    "sample/manifest.json",
                    server_argument[0],
                    server_argument[1],
                ])
                .is_err(),
                "{} must require --serve when explicitly set",
                server_argument[0]
            );
        }
    }

    #[test]
    fn server_argument_defaults_do_not_block_evidence_mode() {
        assert!(
            Args::try_parse_from([
                "phi-4dgs-player",
                "--manifest",
                "sample/manifest.json",
                "--golden",
                "reference.rgba8",
            ])
            .is_ok()
        );
    }

    #[test]
    fn serve_accepts_an_explicit_initial_time() {
        let args = Args::try_parse_from([
            "phi-4dgs-player",
            "--manifest",
            "sample/manifest.json",
            "--serve",
            "--time",
            "0.25",
        ])
        .unwrap();
        assert_eq!(args.time, Some(0.25));
    }

    #[test]
    fn manifest_initial_time_is_used_unless_the_cli_overrides_it() {
        assert_eq!(resolve_render_time(0.125, None).unwrap(), 0.125);
        assert_eq!(resolve_render_time(0.125, Some(0.75)).unwrap(), 0.75);
        assert!(resolve_render_time(0.125, Some(-0.01)).is_err());
        assert!(resolve_render_time(0.125, Some(f32::NAN)).is_err());
    }

    #[test]
    fn interaction_floor_defaults_to_the_manifest_or_renderer_floor_whichever_is_higher() {
        let high_manifest_floor = 16.0 / 255.0;
        assert_eq!(
            resolve_interaction_alpha_min(high_manifest_floor, None).unwrap(),
            high_manifest_floor
        );

        let low_manifest_floor = 2.0 / 255.0;
        assert_eq!(
            resolve_interaction_alpha_min(low_manifest_floor, None).unwrap(),
            renderer::INTERACTIVE_ALPHA_MIN
        );
    }

    #[test]
    fn explicit_interaction_floor_must_not_weaken_the_manifest_policy() {
        let base_alpha_min = 16.0 / 255.0;
        let explicit_alpha_min = 24.0 / 255.0;
        assert_eq!(
            resolve_interaction_alpha_min(base_alpha_min, Some(explicit_alpha_min)).unwrap(),
            explicit_alpha_min
        );
        assert!(resolve_interaction_alpha_min(base_alpha_min, Some(8.0 / 255.0)).is_err());
        assert!(resolve_interaction_alpha_min(base_alpha_min, Some(f32::NAN)).is_err());
        assert!(resolve_interaction_alpha_min(base_alpha_min, Some(1.0)).is_err());
    }

    #[test]
    fn interaction_floor_is_applied_before_the_first_motion_frame() {
        let mut lod = AdaptiveLod::new(BASE_FLOOR);
        assert_eq!(
            lod.alpha_for_frame(true, INTERACTION_FLOOR),
            INTERACTION_FLOOR
        );
    }

    #[test]
    fn gpu_overload_increases_cutoff_without_waiting_for_a_deadline_counter() {
        let mut lod = AdaptiveLod::new(BASE_FLOOR);
        let before = lod.alpha_for_frame(true, INTERACTION_FLOOR);
        lod.observe_gpu(14.0, FRAME_MS, true, INTERACTION_FLOOR);
        assert!(lod.alpha_for_frame(true, INTERACTION_FLOOR) > before);
        assert_eq!(lod.overload_events, 1);
    }

    #[test]
    fn stable_idle_frames_recover_quality_but_never_below_base() {
        let mut lod = AdaptiveLod::new(BASE_FLOOR);
        lod.alpha_min = INTERACTION_FLOOR;
        for _ in 0..(LOD_RECOVER_STABLE_FRAMES * 4) {
            lod.observe_gpu(5.0, FRAME_MS, false, INTERACTION_FLOOR);
        }
        assert_eq!(lod.alpha_min, BASE_FLOOR);
        assert!(lod.recovery_events > 0);
    }

    #[test]
    fn recovery_during_motion_stops_at_the_interaction_floor() {
        let mut lod = AdaptiveLod::new(BASE_FLOOR);
        lod.alpha_min = 12.0 / 255.0;
        for _ in 0..(LOD_RECOVER_STABLE_FRAMES * 4) {
            lod.observe_gpu(5.0, FRAME_MS, true, INTERACTION_FLOOR);
        }
        assert_eq!(lod.alpha_min, INTERACTION_FLOOR);
    }

    fn feedback_sample(
        generation: u64,
        ssrc: u32,
        pli_count: u64,
        fir_count: u64,
        native_requests: u64,
        native_recovered_requests: u64,
    ) -> (BrowserRecoveryFeedback, RecoveryMetrics) {
        (
            BrowserRecoveryFeedback {
                connection_generation: generation,
                active_ssrc: ssrc,
                pli_count,
                fir_count,
                ..Default::default()
            },
            RecoveryMetrics {
                force_key_unit_requests: native_requests,
                feedback_force_key_unit_requests_recovered: native_recovered_requests,
                ..Default::default()
            },
        )
    }

    fn observe_feedback(
        watchdog: &mut KeyframeFeedbackWatchdog,
        now: Instant,
        sample: (BrowserRecoveryFeedback, RecoveryMetrics),
    ) -> Option<u64> {
        watchdog.observe(now, &sample.0, &sample.1)
    }

    #[test]
    fn keyframe_watchdog_uses_the_first_receiver_sample_as_a_baseline() {
        let now = Instant::now();
        let mut watchdog = KeyframeFeedbackWatchdog::default();
        assert_eq!(
            observe_feedback(&mut watchdog, now, feedback_sample(1, 77, 4, 1, 9, 3)),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + KEYFRAME_FEEDBACK_GRACE * 2,
                feedback_sample(1, 77, 4, 1, 9, 3),
            ),
            None
        );
    }

    #[test]
    fn native_feedback_before_receiver_telemetry_needs_no_fallback() {
        let now = Instant::now();
        let mut watchdog = KeyframeFeedbackWatchdog::default();
        assert_eq!(
            observe_feedback(&mut watchdog, now, feedback_sample(1, 77, 0, 0, 0, 0)),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_millis(10),
                feedback_sample(1, 77, 0, 0, 1, 1),
            ),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_secs(1),
                feedback_sample(1, 77, 1, 0, 1, 1),
            ),
            None
        );
    }

    fn receiver_progress(
        generation: u64,
        ssrc: u32,
        sample_time_ms: f64,
        frames_received: u64,
        packets_received: u64,
    ) -> BrowserRecoveryFeedback {
        BrowserRecoveryFeedback {
            progress_schema: RECEIVER_PROGRESS_SCHEMA,
            connection_generation: generation,
            active_ssrc: ssrc,
            sample_time_ms,
            frames_received,
            packets_received,
            ..Default::default()
        }
    }

    #[test]
    fn receiver_packet_progress_without_frames_requests_one_idr_per_stall() {
        let now = Instant::now();
        let mut watchdog = ReceiverProgressWatchdog::default();
        assert_eq!(
            watchdog.observe(now, &receiver_progress(1, 77, 1_000.0, 2_980, 62_977)),
            None
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(1),
                &receiver_progress(1, 77, 2_000.0, 3_010, 63_663),
            ),
            None
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(2),
                &receiver_progress(1, 77, 3_000.0, 3_010, 64_225),
            ),
            Some(ReceiverProgressAction::RequestKeyframe(
                ReceiverRecoverySource::ProgressFallback
            ))
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(2) + Duration::from_millis(10),
                &receiver_progress(1, 77, 3_000.0, 3_010, 64_225),
            ),
            None
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(3),
                &receiver_progress(1, 77, 4_000.0, 3_010, 64_900),
            ),
            None
        );
    }

    #[test]
    fn receiver_progress_after_an_idr_is_counted_as_recovery() {
        let now = Instant::now();
        let mut watchdog = ReceiverProgressWatchdog::default();
        assert_eq!(
            watchdog.observe(now, &receiver_progress(1, 77, 1_000.0, 100, 1_000)),
            None
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(1),
                &receiver_progress(1, 77, 2_000.0, 100, 1_500),
            ),
            Some(ReceiverProgressAction::RequestKeyframe(
                ReceiverRecoverySource::ProgressFallback
            ))
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_millis(1_100),
                &receiver_progress(1, 77, 3_000.0, 130, 2_000),
            ),
            Some(ReceiverProgressAction::Recovered(
                ReceiverRecoverySource::ProgressFallback
            ))
        );
    }

    #[test]
    fn client_request_preempts_the_same_stall_fallback_and_recovers_once() {
        let now = Instant::now();
        let mut watchdog = ReceiverProgressWatchdog::default();
        assert_eq!(
            watchdog.observe(now, &receiver_progress(7, 99, 1_000.0, 100, 1_000)),
            None
        );
        assert_eq!(
            watchdog.begin_client_request(now + Duration::from_secs(1), 7, 99, 1, 100,),
            ClientRecoveryAction::RequestKeyframe
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(1),
                &receiver_progress(7, 99, 2_000.0, 100, 1_600),
            ),
            None
        );
        assert_eq!(
            watchdog.begin_client_request(now + Duration::from_millis(1_100), 7, 99, 1, 100,),
            ClientRecoveryAction::Coalesced
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(2),
                &receiver_progress(7, 99, 3_000.0, 130, 2_000),
            ),
            Some(ReceiverProgressAction::Recovered(
                ReceiverRecoverySource::Client
            ))
        );
    }

    #[test]
    fn fallback_first_coalesces_a_late_client_request() {
        let now = Instant::now();
        let mut watchdog = ReceiverProgressWatchdog::default();
        assert_eq!(
            watchdog.observe(now, &receiver_progress(3, 55, 1_000.0, 100, 1_000)),
            None
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(1),
                &receiver_progress(3, 55, 2_000.0, 100, 1_500),
            ),
            Some(ReceiverProgressAction::RequestKeyframe(
                ReceiverRecoverySource::ProgressFallback
            ))
        );
        assert_eq!(
            watchdog.begin_client_request(now + Duration::from_millis(1_100), 3, 55, 1, 100,),
            ClientRecoveryAction::Coalesced
        );
    }

    #[test]
    fn browser_progress_between_stats_samples_opens_a_distinct_second_stall() {
        let now = Instant::now();
        let mut watchdog = ReceiverProgressWatchdog::default();
        assert_eq!(
            watchdog.observe(now, &receiver_progress(7, 99, 1_000.0, 100, 1_000)),
            None
        );
        assert_eq!(
            watchdog.begin_client_request(now + Duration::from_secs(1), 7, 99, 1, 100),
            ClientRecoveryAction::RequestKeyframe
        );

        // The browser saw frame 101 and then entered another stall before the
        // next full ReceiverStats sample reported frame 101 to the server.
        assert_eq!(
            watchdog.begin_client_request(now + Duration::from_secs(2), 7, 99, 2, 101),
            ClientRecoveryAction::RecoveredThenRequestKeyframe(ReceiverRecoverySource::Client)
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(3),
                &receiver_progress(7, 99, 2_000.0, 102, 2_000),
            ),
            Some(ReceiverProgressAction::Recovered(
                ReceiverRecoverySource::Client
            ))
        );
    }

    #[test]
    fn receiver_progress_watchdog_does_not_request_without_rtp_progress() {
        let now = Instant::now();
        let mut watchdog = ReceiverProgressWatchdog::default();
        assert_eq!(
            watchdog.observe(now, &receiver_progress(1, 77, 1_000.0, 100, 1_000)),
            None
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(2),
                &receiver_progress(1, 77, 3_000.0, 100, 1_000),
            ),
            None
        );
    }

    #[test]
    fn receiver_progress_watchdog_resets_for_a_new_media_session() {
        let now = Instant::now();
        let mut watchdog = ReceiverProgressWatchdog::default();
        assert_eq!(
            watchdog.observe(now, &receiver_progress(1, 77, 1_000.0, 100, 1_000)),
            None
        );
        assert_eq!(
            watchdog.observe(
                now + Duration::from_secs(2),
                &receiver_progress(2, 88, 100.0, 1, 10),
            ),
            None
        );
    }

    #[test]
    fn missing_native_feedback_forces_once_after_the_grace_period() {
        let now = Instant::now();
        let mut watchdog = KeyframeFeedbackWatchdog::default();
        assert_eq!(
            observe_feedback(&mut watchdog, now, feedback_sample(1, 77, 0, 0, 0, 0)),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_secs(1),
                feedback_sample(1, 77, 1, 0, 0, 0),
            ),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_secs(1) + KEYFRAME_FEEDBACK_GRACE,
                feedback_sample(1, 77, 1, 0, 0, 0),
            ),
            Some(1)
        );
        watchdog.record_fallback(
            now + Duration::from_secs(1) + KEYFRAME_FEEDBACK_GRACE,
            1,
            true,
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_secs(2),
                feedback_sample(1, 77, 1, 0, 0, 0),
            ),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_secs(1) + KEYFRAME_FEEDBACK_GRACE + KEYFRAME_FEEDBACK_RETRY,
                feedback_sample(1, 77, 1, 0, 1, 0),
            ),
            Some(1)
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_secs(3),
                feedback_sample(1, 77, 1, 0, 1, 1),
            ),
            None
        );
    }

    #[test]
    fn swallowed_native_feedback_retries_while_client_owns_the_same_stall() {
        let now = Instant::now();
        let mut feedback = KeyframeFeedbackWatchdog::default();
        let mut progress = ReceiverProgressWatchdog::default();
        assert_eq!(
            observe_feedback(&mut feedback, now, feedback_sample(1, 77, 0, 0, 0, 0)),
            None
        );
        assert_eq!(
            progress.observe(now, &receiver_progress(1, 77, 1_000.0, 100, 1_000)),
            None
        );
        assert_eq!(
            progress.begin_client_request(now + Duration::from_millis(900), 1, 77, 1, 100,),
            ClientRecoveryAction::RequestKeyframe
        );

        // The payloader observed the browser PLI (native_requests=1), but no
        // non-delta access unit was encoded. That leaves the transport pending
        // counter at one until a manual fallback finally produces an IDR.
        let transport_pending_force_key_unit_requests = 1_u64;
        assert_eq!(
            observe_feedback(
                &mut feedback,
                now + Duration::from_secs(1),
                feedback_sample(1, 77, 1, 0, 1, 0),
            ),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut feedback,
                now + Duration::from_secs(1) + KEYFRAME_RECOVERY_TIMEOUT,
                feedback_sample(1, 77, 1, 0, 1, 0),
            ),
            Some(1)
        );
        assert!(transport_pending_force_key_unit_requests > 0);
        // The client keeps recovery attribution, but that ownership must not
        // veto the manual retry proven necessary by feedback.observe above.
        assert!(!progress.begin_fallback_request(ReceiverRecoverySource::FeedbackFallback, 100,));
    }

    #[test]
    fn encoded_recovery_arriving_during_grace_cancels_the_fallback() {
        let now = Instant::now();
        let mut watchdog = KeyframeFeedbackWatchdog::default();
        assert_eq!(
            observe_feedback(&mut watchdog, now, feedback_sample(1, 77, 0, 0, 0, 0)),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_secs(1),
                feedback_sample(1, 77, 1, 0, 0, 0),
            ),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_secs(1) + Duration::from_millis(100),
                feedback_sample(1, 77, 1, 0, 1, 0),
            ),
            None
        );
        assert_eq!(
            observe_feedback(
                &mut watchdog,
                now + Duration::from_secs(1) + Duration::from_millis(200),
                feedback_sample(1, 77, 1, 0, 1, 1),
            ),
            None
        );
    }
}
