use std::{
    io::Write,
    process::{Command, Stdio},
    time::Instant,
};

use anyhow::{Context, Result, bail, ensure};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_allocators::{DmaBufAllocator, DmaBufAllocatorExtManual};
use gstreamer_app::{AppSink, AppSrc};
use gstreamer_video::{VideoFormat, VideoFrameFlags, VideoMeta};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::external_image::ExternalImage;

const SOURCE_COLORIMETRY: &str = "sRGB";
const ENCODE_COLORIMETRY: &str = "bt709";
// The target GStreamer 1.24/radeonsi path fixates BGRA -> NV12 conversion to
// JPEG/centered chroma. Claiming MPEG-2/left here would make the RTP colorspace
// extension disagree with the surface actually fed to the encoder.
const ENCODE_CHROMA_SITE: &str = "jpeg";
const AUDIT_BITRATE_KBPS: u32 = 6_000;
const AUDIT_TARGET_USAGE: u32 = 4;
const AUDIT_VIDEO_SLICES: u32 = 4;
const MIN_ROUNDTRIP_PSNR_DB: f64 = 30.0;
const MAX_ROUNDTRIP_RGB_MAE: f64 = 8.0;
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

#[derive(Debug, Clone, Serialize)]
pub struct PermutationAudit {
    /// Reference channel order compared with decoded R,G,B. RGB is identity;
    /// BGR is the common red/blue swap failure.
    pub reference_order: &'static str,
    pub sse: u64,
    pub psnr_db: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaAudit {
    pub input_memory: &'static str,
    pub input_format: String,
    pub drm_memory_bytes_little_endian: &'static str,
    pub appsrc_colorimetry: &'static str,
    pub nv12_colorimetry: &'static str,
    pub nv12_chroma_site: &'static str,
    pub h264_caps: String,
    pub color_conversion: &'static str,
    pub reference_importer: &'static str,
    pub reference_width: u32,
    pub reference_height: u32,
    pub reference_rgba_bytes: usize,
    pub reference_rgba_sha256: String,
    pub decoded_rgba_bytes: usize,
    pub decoded_rgba_sha256: String,
    pub encoder: &'static str,
    pub decoder: &'static str,
    pub codec: &'static str,
    pub h264_sps_vui_color_description: &'static str,
    pub display_color_contract: &'static str,
    pub bitrate_kbps: u32,
    pub target_usage: u32,
    pub video_slices: u32,
    pub b_frames: u32,
    pub encoded_bytes: usize,
    pub encode_ms: f64,
    pub decode_ms: f64,
    pub decoded_rgb_mae: [f64; 3],
    pub reference_rgb_mean: [f64; 3],
    pub decoded_rgb_mean: [f64; 3],
    pub decoded_rgb_mean_delta: [f64; 3],
    pub decoded_rgb_max_abs: [u8; 3],
    pub permutation_scores: Vec<PermutationAudit>,
    pub best_permutation: &'static str,
    pub pixel_gate_passed: bool,
    /// The streaming input remains DMA-BUF zero-copy. The one-shot wgpu
    /// evidence reference and independently decoded output are deliberately
    /// CPU-visible for the validation gate.
    pub cpu_pixel_copy: bool,
    pub validation_cpu_readback: bool,
}

struct EncodedFrame {
    bytes: Vec<u8>,
    caps_text: String,
    encode_ms: f64,
}

struct RoundtripMetrics {
    reference_mean: [f64; 3],
    decoded_mean: [f64; 3],
    channel_mae: [f64; 3],
    channel_mean_delta: [f64; 3],
    channel_max_abs: [u8; 3],
    permutations: Vec<PermutationAudit>,
    best_permutation: &'static str,
}

fn encode_pipeline_description() -> String {
    format!(
        "appsrc name=source is-live=true block=true format=time \
         ! vaapipostproc name=vpp \
         ! video/x-raw(memory:VASurface),format=NV12,colorimetry={ENCODE_COLORIMETRY} \
         ! vaapih264enc name=encoder bitrate={AUDIT_BITRATE_KBPS} cpb-length=500 max-bframes=0 refs=1 keyframe-period=60 num-slices={AUDIT_VIDEO_SLICES} rate-control=cbr quality-level={AUDIT_TARGET_USAGE} cabac=false dct8x8=false \
         ! h264parse config-interval=-1 \
         ! video/x-h264,profile=constrained-baseline,stream-format=byte-stream,alignment=au \
         ! capssetter replace=false caps=video/x-h264,colorimetry={ENCODE_COLORIMETRY},chroma-site={ENCODE_CHROMA_SITE} \
         ! appsink name=encoded sync=false max-buffers=1 drop=false"
    )
}

pub fn encode_one(
    image: &ExternalImage,
    width: u32,
    height: u32,
    reference_rgba: &[u8],
) -> Result<MediaAudit> {
    gst::init().context("initialize GStreamer")?;
    for element in [
        "appsrc",
        "vaapipostproc",
        "vaapih264enc",
        "h264parse",
        "capssetter",
        "appsink",
    ] {
        ensure!(
            gst::ElementFactory::find(element).is_some(),
            "required GStreamer element {element} is unavailable"
        );
    }

    let drm_format = if image.layout.modifier == 0 {
        image.layout.drm_fourcc_name.to_string()
    } else {
        format!(
            "{}:0x{:016x}",
            image.layout.drm_fourcc_name, image.layout.modifier
        )
    };
    ensure!(
        image.layout.modifier == DRM_FORMAT_MOD_LINEAR,
        "media path requires linear DMA-BUF modifier 0, got {}",
        image.layout.modifier_hex
    );
    let expected_rgba_bytes = width as usize * height as usize * 4;
    ensure!(
        reference_rgba.len() == expected_rgba_bytes,
        "same-frame wgpu reference has {} bytes, expected {expected_rgba_bytes} for {width}x{height} RGBA",
        reference_rgba.len()
    );
    let reference_rgba_sha256 = sha256_hex(reference_rgba);
    let encoded = encode_dmabuf(image, width, height, reference_rgba)?;
    let (decoded_rgba, decode_ms) = decode_h264_software(&encoded.bytes, width, height)?;
    let decoded_rgba_sha256 = sha256_hex(&decoded_rgba);
    let metrics = compare_rgb(reference_rgba, &decoded_rgba, width, height)?;
    let identity = metrics
        .permutations
        .iter()
        .find(|score| score.reference_order == "RGB")
        .context("identity permutation score")?;
    let maximum_mae = metrics.channel_mae.into_iter().fold(0.0_f64, f64::max);
    ensure!(
        metrics.best_permutation == "RGB"
            && identity.psnr_db >= MIN_ROUNDTRIP_PSNR_DB
            && maximum_mae <= MAX_ROUNDTRIP_RGB_MAE,
        "H.264 pixel gate failed: best={}, identity_psnr_db={:.3} (min {:.3}), reference_mean={:?}, decoded_mean={:?}, rgb_mae={:?} (max {:.3}), permutation_sse={:?}",
        metrics.best_permutation,
        identity.psnr_db,
        MIN_ROUNDTRIP_PSNR_DB,
        metrics.reference_mean,
        metrics.decoded_mean,
        metrics.channel_mae,
        MAX_ROUNDTRIP_RGB_MAE,
        metrics
            .permutations
            .iter()
            .map(|score| (score.reference_order, score.sse))
            .collect::<Vec<_>>()
    );

    Ok(MediaAudit {
        input_memory: "DMA-BUF",
        input_format: drm_format,
        drm_memory_bytes_little_endian: "AR24 = B,G,R,A bytes (DRM ARGB8888 word)",
        appsrc_colorimetry: SOURCE_COLORIMETRY,
        nv12_colorimetry: ENCODE_COLORIMETRY,
        nv12_chroma_site: ENCODE_CHROMA_SITE,
        h264_caps: encoded.caps_text,
        color_conversion: "vaapipostproc GPU AR24(sRGB/full)->NV12(bt709/limited,jpeg-centered)",
        reference_importer: "same-frame wgpu evidence readback BGRA -> RGBA (one-shot validation only)",
        reference_width: width,
        reference_height: height,
        reference_rgba_bytes: reference_rgba.len(),
        reference_rgba_sha256,
        decoded_rgba_bytes: decoded_rgba.len(),
        decoded_rgba_sha256,
        encoder: "vaapih264enc (AMD VA-API)",
        decoder: "ffmpeg/libavcodec software (independent pixel gate)",
        codec: "H.264 constrained-baseline byte-stream",
        h264_sps_vui_color_description: "not asserted by this gate; transport caps carry the color contract",
        display_color_contract: "GStreamer H.264 caps + negotiated WebRTC RTP colorspace extension: BT.709 limited, JPEG/centered chroma",
        bitrate_kbps: AUDIT_BITRATE_KBPS,
        target_usage: AUDIT_TARGET_USAGE,
        video_slices: AUDIT_VIDEO_SLICES,
        b_frames: 0,
        encoded_bytes: encoded.bytes.len(),
        encode_ms: encoded.encode_ms,
        decode_ms,
        decoded_rgb_mae: metrics.channel_mae,
        reference_rgb_mean: metrics.reference_mean,
        decoded_rgb_mean: metrics.decoded_mean,
        decoded_rgb_mean_delta: metrics.channel_mean_delta,
        decoded_rgb_max_abs: metrics.channel_max_abs,
        permutation_scores: metrics.permutations,
        best_permutation: metrics.best_permutation,
        pixel_gate_passed: true,
        cpu_pixel_copy: false,
        validation_cpu_readback: true,
    })
}

fn encode_dmabuf(
    image: &ExternalImage,
    width: u32,
    height: u32,
    reference_rgba: &[u8],
) -> Result<EncodedFrame> {
    let pipeline = gst::parse::launch(&encode_pipeline_description())
        .context("construct DMA-BUF reference and VA-API encode pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("GStreamer launch did not return a pipeline"))?;
    let appsrc = appsrc(&pipeline, "source")?;
    let encoded_sink = appsink(&pipeline, "encoded")?;
    let caps = gst::Caps::builder("video/x-raw")
        .features(["memory:DMABuf"])
        .field("format", "BGRA")
        .field("width", width as i32)
        .field("height", height as i32)
        .field("framerate", gst::Fraction::new(60, 1))
        .field("colorimetry", SOURCE_COLORIMETRY)
        .build();
    appsrc.set_caps(Some(&caps));

    let allocator = DmaBufAllocator::new();
    let owned_fd = image
        .dmabuf
        .try_clone()
        .context("duplicate rendered DMA-BUF fd for GStreamer")?;
    let memory =
        unsafe { allocator.alloc_dmabuf(owned_fd, image.layout.allocation_bytes as usize) }
            .context("wrap rendered DMA-BUF in GstMemory")?;
    let mut buffer = gst::Buffer::new();
    {
        let buffer = buffer.get_mut().expect("new buffer is writable");
        buffer.append_memory(memory);
        VideoMeta::add_full(
            buffer,
            VideoFrameFlags::empty(),
            VideoFormat::Bgra,
            width,
            height,
            &[image.layout.offset as usize],
            &[image.layout.stride as i32],
        )
        .context("attach linear BGRA DMA-BUF layout")?;
        buffer.set_pts(gst::ClockTime::ZERO);
        buffer.set_duration(gst::ClockTime::from_nseconds(1_000_000_000 / 60));
    }

    pipeline
        .set_state(gst::State::Playing)
        .context("start DMA-BUF audit pipeline")?;
    let started = Instant::now();
    appsrc
        .push_buffer(buffer)
        .context("push rendered DMA-BUF")?;
    appsrc.end_of_stream().context("finish one-frame stream")?;
    let encoded_sample = pull_sample(&encoded_sink, &pipeline, "H.264")?;
    let encode_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let encoded_caps = encoded_sample
        .caps()
        .context("encoded sample has no caps")?
        .to_owned();
    let encoded_structure = encoded_caps
        .structure(0)
        .context("encoded caps have no structure")?;
    let encoded_colorimetry = encoded_structure
        .get::<String>("colorimetry")
        .context("encoded caps have no colorimetry")?;
    let encoded_chroma_site = encoded_structure
        .get::<String>("chroma-site")
        .context("encoded caps have no chroma-site")?;
    ensure!(
        encoded_colorimetry == ENCODE_COLORIMETRY,
        "encoded colorimetry is {encoded_colorimetry}, expected {ENCODE_COLORIMETRY}"
    );
    ensure!(
        encoded_chroma_site == ENCODE_CHROMA_SITE,
        "encoded chroma-site is {encoded_chroma_site}, expected {ENCODE_CHROMA_SITE}"
    );
    let encoded_buffer = encoded_sample
        .buffer()
        .context("encoded sample has no buffer")?;
    let encoded_map = encoded_buffer
        .map_readable()
        .map_err(|_| anyhow::anyhow!("map encoded H.264 access unit"))?;
    let bytes = encoded_map.as_slice().to_vec();
    if let Some(directory) = std::env::var_os("PHI_MEDIA_AUDIT_DUMP") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).context("create media audit dump directory")?;
        std::fs::write(directory.join("reference.rgba"), reference_rgba)
            .context("write media audit reference")?;
        std::fs::write(directory.join("encoded.h264"), &bytes)
            .context("write media audit H.264")?;
    }
    let caps_text = encoded_caps.to_string();
    pipeline
        .set_state(gst::State::Null)
        .context("stop DMA-BUF audit pipeline")?;
    ensure!(
        !bytes.is_empty(),
        "VA-API encoder returned an empty H.264 access unit"
    );
    Ok(EncodedFrame {
        bytes,
        caps_text,
        encode_ms,
    })
}

fn decode_h264_software(encoded: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, f64)> {
    ensure!(
        !encoded.is_empty(),
        "cannot decode an empty H.264 access unit"
    );
    let started = Instant::now();
    let mut child = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "h264",
            "-i",
            "pipe:0",
            "-frames:v",
            "1",
            "-vf",
            "scale=in_range=tv:in_color_matrix=bt709:out_range=pc:out_color_matrix=bt709,format=rgba",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start independent ffmpeg software decoder")?;
    child
        .stdin
        .take()
        .context("ffmpeg decoder stdin")?
        .write_all(encoded)
        .context("write H.264 access unit to ffmpeg")?;
    let output = child
        .wait_with_output()
        .context("wait for independent ffmpeg software decoder")?;
    ensure!(
        output.status.success(),
        "independent ffmpeg software decoder failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let expected = width as usize * height as usize * 4;
    ensure!(
        output.stdout.len() == expected,
        "independent decoder returned {} RGBA bytes, expected {expected} for {width}x{height}",
        output.stdout.len()
    );
    Ok((output.stdout, started.elapsed().as_secs_f64() * 1_000.0))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn appsrc(pipeline: &gst::Pipeline, name: &str) -> Result<AppSrc> {
    pipeline
        .by_name(name)
        .with_context(|| format!("pipeline appsrc {name}"))?
        .downcast::<AppSrc>()
        .map_err(|_| anyhow::anyhow!("{name} is not AppSrc"))
}

fn appsink(pipeline: &gst::Pipeline, name: &str) -> Result<AppSink> {
    pipeline
        .by_name(name)
        .with_context(|| format!("pipeline appsink {name}"))?
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("{name} is not AppSink"))
}

fn pull_sample(appsink: &AppSink, pipeline: &gst::Pipeline, label: &str) -> Result<gst::Sample> {
    if let Some(sample) = appsink.try_pull_sample(gst::ClockTime::from_seconds(8)) {
        return Ok(sample);
    }
    let detail = pipeline
        .bus()
        .and_then(|bus| {
            bus.timed_pop_filtered(
                gst::ClockTime::ZERO,
                &[gst::MessageType::Error, gst::MessageType::Warning],
            )
        })
        .map(|message| format!(": {message:?}"))
        .unwrap_or_default();
    let _ = pipeline.set_state(gst::State::Null);
    bail!("pipeline produced no {label} sample{detail}")
}

fn compare_rgb(
    reference: &[u8],
    decoded: &[u8],
    width: u32,
    height: u32,
) -> Result<RoundtripMetrics> {
    let pixels = width as usize * height as usize;
    let expected = pixels * 4;
    ensure!(
        reference.len() == expected && decoded.len() == expected,
        "RGBA comparison size mismatch: reference={}, decoded={}, expected={expected}",
        reference.len(),
        decoded.len()
    );
    ensure!(pixels > 0, "cannot compare an empty frame");

    const PERMUTATIONS: [(&str, [usize; 3]); 6] = [
        ("RGB", [0, 1, 2]),
        ("RBG", [0, 2, 1]),
        ("GRB", [1, 0, 2]),
        ("GBR", [1, 2, 0]),
        ("BRG", [2, 0, 1]),
        ("BGR", [2, 1, 0]),
    ];
    let mut permutation_sse = [0_u64; PERMUTATIONS.len()];
    let mut channel_abs = [0_u64; 3];
    let mut channel_delta = [0_i64; 3];
    let mut reference_sum = [0_u64; 3];
    let mut decoded_sum = [0_u64; 3];
    let mut channel_max_abs = [0_u8; 3];
    for (reference_pixel, decoded_pixel) in reference.chunks_exact(4).zip(decoded.chunks_exact(4)) {
        for channel in 0..3 {
            reference_sum[channel] += u64::from(reference_pixel[channel]);
            decoded_sum[channel] += u64::from(decoded_pixel[channel]);
            let delta = i16::from(decoded_pixel[channel]) - i16::from(reference_pixel[channel]);
            let absolute = delta.unsigned_abs() as u8;
            channel_abs[channel] += u64::from(absolute);
            channel_delta[channel] += i64::from(delta);
            channel_max_abs[channel] = channel_max_abs[channel].max(absolute);
        }
        for (score, (_, permutation)) in permutation_sse.iter_mut().zip(PERMUTATIONS) {
            for channel in 0..3 {
                let delta = i32::from(decoded_pixel[channel])
                    - i32::from(reference_pixel[permutation[channel]]);
                *score += (delta * delta) as u64;
            }
        }
    }
    let samples = (pixels * 3) as f64;
    let permutations = PERMUTATIONS
        .into_iter()
        .zip(permutation_sse)
        .map(|((reference_order, _), sse)| {
            let mse = sse as f64 / samples;
            let psnr_db = if mse == 0.0 {
                99.0
            } else {
                10.0 * (255.0_f64 * 255.0 / mse).log10()
            };
            PermutationAudit {
                reference_order,
                sse,
                psnr_db,
            }
        })
        .collect::<Vec<_>>();
    let best_permutation = permutations
        .iter()
        .min_by_key(|score| score.sse)
        .context("RGB permutation scores")?
        .reference_order;
    let pixel_count = pixels as f64;
    Ok(RoundtripMetrics {
        reference_mean: reference_sum.map(|value| value as f64 / pixel_count),
        decoded_mean: decoded_sum.map(|value| value as f64 / pixel_count),
        channel_mae: channel_abs.map(|value| value as f64 / pixel_count),
        channel_mean_delta: channel_delta.map(|value| value as f64 / pixel_count),
        channel_max_abs,
        permutations,
        best_permutation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_pipelines_pin_the_color_contract_and_reference_quality_mode() {
        let encode = encode_pipeline_description();
        assert!(encode.contains("appsrc name=source"));
        assert!(encode.contains("format=NV12,colorimetry=bt709"));
        assert!(encode.contains("bitrate=6000 cpb-length=500"));
        assert!(encode.contains("num-slices=4"));
        assert!(encode.contains("quality-level=4"));
        assert!(encode.contains(
            "capssetter replace=false caps=video/x-h264,colorimetry=bt709,chroma-site=jpeg"
        ));
        assert!(encode.contains("profile=constrained-baseline"));
    }

    #[test]
    fn permutation_gate_prefers_identity_for_an_identity_frame() {
        let reference = vec![240, 80, 20, 255, 10, 100, 230, 255];
        let metrics = compare_rgb(&reference, &reference, 2, 1).unwrap();
        assert_eq!(metrics.best_permutation, "RGB");
        assert_eq!(metrics.permutations[0].sse, 0);
        assert_eq!(metrics.permutations[0].psnr_db, 99.0);
    }

    #[test]
    fn permutation_gate_identifies_a_red_blue_swap() {
        let reference = vec![240, 80, 20, 255, 10, 100, 230, 255];
        let decoded = vec![20, 80, 240, 255, 230, 100, 10, 255];
        let metrics = compare_rgb(&reference, &decoded, 2, 1).unwrap();
        assert_eq!(metrics.best_permutation, "BGR");
        let identity = metrics
            .permutations
            .iter()
            .find(|score| score.reference_order == "RGB")
            .unwrap();
        let swapped = metrics
            .permutations
            .iter()
            .find(|score| score.reference_order == "BGR")
            .unwrap();
        assert!(identity.sse > 0);
        assert_eq!(swapped.sse, 0);
    }
}
