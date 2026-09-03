use std::{path::Path, sync::mpsc, time::Instant};

use anyhow::{Context, Result, ensure};
use bytemuck::{Pod, Zeroable};
use serde::Serialize;
use wgpu::util::DeviceExt;

use crate::media::{self, MediaAudit};
use crate::{
    asset::{Asset, FixedCamera},
    external_image::{DmabufLayout, ExternalImage},
    shader,
};

const SCREEN_RECORD_BYTES: u64 = 48;
const HISTOGRAM_BYTES: u64 = 256 * 256 * 4;
const COUNTER_BYTES: u64 = 64;
const TILE_SIZE: u32 = 16;
const TILE_MASK_RANKS_PER_BIT: u64 = 1;
const TILE_MASK_SHARDS: usize = 3;
const TIMESTAMP_COUNT: u32 = 6;
const TIMESTAMP_BYTES: u64 = TIMESTAMP_COUNT as u64 * 8;
const PERSISTENT_FLAG_BYTES: u64 = 4;
const TELEMETRY_BYTES: u64 = COUNTER_BYTES + TIMESTAMP_BYTES + PERSISTENT_FLAG_BYTES;
const TELEMETRY_RING_SIZE: usize = 8;
const LEGACY_ALPHA_CAP: f32 = 0.99;
const LEGACY_TRANSMITTANCE_MIN: f32 = 1.0 / 255.0;
const SCENE_FLAG_TELEMETRY: u32 = 1;
const SCENE_FLAG_INTERACTIVE: u32 = 2;
const SCENE_FLAG_LINEAR_TO_SRGB: u32 = 4;
const SCENE_FLAG_OPACITY_COMPENSATION: u32 = 8;
const SCENE_FLAG_EXPLICIT_RASTER_POLICY: u32 = 16;
pub const INTERACTIVE_ALPHA_MIN: f32 = 8.0 / 255.0;

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct FrameCounters {
    pub active: u32,
    pub visible: u32,
    pub invalid: u32,
    pub culled_temporal: u32,
    pub culled_frustum: u32,
    pub culled_footprint: u32,
    pub equal_depth: u32,
    pub tile_overlaps: u32,
    pub tile_overflow: u32,
    pub max_tile_load: u32,
    pub early_terminated_pixels: u32,
    pub pixel_splat_tests: u32,
    pub budget_limited_pixels: u32,
    pub max_pixel_splat_tests: u32,
    pub max_budget_remaining_transmittance: f32,
    pub persistent_workload_flags: u32,
}

#[derive(Debug, Serialize)]
pub struct FrameResult {
    pub adapter: String,
    pub backend: String,
    pub driver: String,
    pub driver_info: String,
    pub shader_bundle_sha256: String,
    pub width: u32,
    pub height: u32,
    pub time: f32,
    pub submit_wait_ms: f64,
    pub counters: FrameCounters,
    pub dmabuf: DmabufLayout,
    pub media: MediaAudit,
    #[serde(skip)]
    pub rgba8: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CameraUniform {
    pub world_to_camera: [f32; 16],
    pub intrinsics: [f32; 4],
    pub near: f32,
    pub far: f32,
    pub eye: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
struct SceneRuntime {
    time: f32,
    temporal_cull: bool,
    alpha_min: f32,
    telemetry_enabled: bool,
    interactive: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamFrame {
    pub frame_id: u64,
    pub slot: usize,
    pub render_wait_ms: f64,
    pub preprocess_ms: f64,
    pub sort_ms: f64,
    pub tile_bin_ms: f64,
    pub tile_render_ms: f64,
    pub splat_ms: f64,
    pub resolve_ms: f64,
    pub gpu_ms: f64,
    pub render_scale: f64,
    pub lod_alpha_min: f64,
    pub counters: FrameCounters,
}

pub struct StreamRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    state: FrameState,
    pipelines: Pipelines,
    bindings: Bindings,
    tile: TileState,
    tile_bindings: TileBindings,
    _accumulation: wgpu::Texture,
    resolve_binding: wgpu::BindGroup,
    outputs: Vec<ExternalImage>,
    timestamp_queries: wgpu::QuerySet,
    timestamp_resolve: wgpu::Buffer,
    telemetry: TelemetryRing,
    timestamp_period_ns: f64,
    manifest: crate::asset::Manifest,
    sh_degree: u32,
    width: u32,
    height: u32,
    last_counters: FrameCounters,
    last_gpu_timings: [f64; 6],
    last_telemetry_frame: u64,
}

struct TelemetrySample {
    frame_id: u64,
    bytes: Vec<u8>,
}

struct TelemetrySlot {
    readback: wgpu::Buffer,
    frame_id: u64,
    pending: Option<mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>>,
}

struct TelemetryRing {
    slots: Vec<TelemetrySlot>,
    next: usize,
}

impl TelemetryRing {
    fn new(device: &wgpu::Device) -> Self {
        let slots = (0..TELEMETRY_RING_SIZE)
            .map(|index| TelemetrySlot {
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("stream telemetry ring {index}")),
                    size: TELEMETRY_BYTES,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                frame_id: 0,
                pending: None,
            })
            .collect();
        Self { slots, next: 0 }
    }

    fn acquire(&mut self) -> Option<usize> {
        for offset in 0..self.slots.len() {
            let index = (self.next + offset) % self.slots.len();
            if self.slots[index].pending.is_none() {
                self.next = (index + 1) % self.slots.len();
                return Some(index);
            }
        }
        None
    }

    fn start_map(&mut self, index: usize, frame_id: u64) {
        let slot = &mut self.slots[index];
        debug_assert!(slot.pending.is_none());
        let (tx, rx) = mpsc::channel();
        slot.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        slot.frame_id = frame_id;
        slot.pending = Some(rx);
    }

    fn collect_ready(&mut self) -> Result<Vec<TelemetrySample>> {
        let mut ready = Vec::new();
        for slot in &mut self.slots {
            let Some(receiver) = slot.pending.as_ref() else {
                continue;
            };
            match receiver.try_recv() {
                Ok(result) => {
                    result.context("map asynchronous stream telemetry")?;
                    let bytes = slot
                        .readback
                        .slice(..)
                        .get_mapped_range()
                        .map_err(|error| anyhow::anyhow!("get telemetry range: {error}"))?
                        .to_vec();
                    slot.readback.unmap();
                    slot.pending = None;
                    ready.push(TelemetrySample {
                        frame_id: slot.frame_id,
                        bytes,
                    });
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    anyhow::bail!("stream telemetry map callback disconnected")
                }
            }
        }
        Ok(ready)
    }
}

impl StreamRenderer {
    pub async fn new(
        asset: &Asset,
        shader_dir: &Path,
        width: u32,
        height: u32,
        slots: usize,
    ) -> Result<Self> {
        ensure!(width > 0 && height > 0, "resolution must be non-zero");
        ensure!(slots >= 2, "stream renderer needs at least two frame slots");
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(instance_desc);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .context("request Vulkan stream adapter")?;
        let external_feature = wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
        let timestamp_features =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        ensure!(
            adapter
                .features()
                .contains(external_feature | timestamp_features),
            "Vulkan adapter does not expose DMA-BUF external memory and timestamp queries"
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("4DGS stream renderer"),
                required_features: external_feature | timestamp_features,
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .context("request Vulkan stream device")?;
        let state = FrameState::new(&device, asset);
        let pipelines = Pipelines::new(&device, shader_dir)?;
        let bindings = Bindings::new(&device, &state, &pipelines);
        queue.write_buffer(
            &state.background,
            0,
            bytemuck::cast_slice(&asset.manifest.render.background),
        );
        let accumulation = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stream accumulation"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let tile = TileState::new(&device, asset.manifest.gaussian_count, width, height)?;
        let tile_bindings = TileBindings::new(
            &device,
            &state,
            &tile,
            &pipelines,
            &accumulation.create_view(&Default::default()),
        );
        let resolve_binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stream resolve binding"),
            layout: &pipelines.resolve.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &accumulation.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: state.background.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: state.scene.as_entire_binding(),
                },
            ],
        });
        let mut outputs = Vec::with_capacity(slots);
        for _ in 0..slots {
            outputs.push(ExternalImage::create(&device, width, height)?);
        }
        let reference = outputs.first().unwrap().layout.clone();
        ensure!(
            outputs
                .iter()
                .all(|output| output.layout.modifier == reference.modifier
                    && output.layout.stride == reference.stride),
            "DMA-BUF pool layouts are inconsistent"
        );
        let timestamp_queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("stream pass timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: TIMESTAMP_COUNT,
        });
        let timestamp_resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stream timestamp resolve"),
            size: TIMESTAMP_BYTES,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let telemetry = TelemetryRing::new(&device);
        let timestamp_period_ns = queue.get_timestamp_period() as f64;
        Ok(Self {
            device,
            queue,
            state,
            pipelines,
            bindings,
            tile,
            tile_bindings,
            _accumulation: accumulation,
            resolve_binding,
            outputs,
            timestamp_queries,
            timestamp_resolve,
            telemetry,
            timestamp_period_ns,
            manifest: asset.manifest.clone(),
            sh_degree: asset.sh_degree(),
            width,
            height,
            last_counters: FrameCounters::default(),
            last_gpu_timings: [0.0; 6],
            last_telemetry_frame: 0,
        })
    }

    pub fn output(&self, slot: usize) -> &ExternalImage {
        &self.outputs[slot]
    }
    pub fn output_layout(&self) -> &DmabufLayout {
        &self.outputs[0].layout
    }
    pub fn slot_count(&self) -> usize {
        self.outputs.len()
    }
    pub fn render(
        &mut self,
        frame_id: u64,
        slot: usize,
        camera: &CameraUniform,
        time: f32,
        temporal_cull: bool,
        alpha_min: f32,
    ) -> Result<StreamFrame> {
        ensure!(slot < self.outputs.len(), "invalid frame slot {slot}");
        ensure!(alpha_min.is_finite() && alpha_min > 0.0 && alpha_min < 1.0);
        self.collect_ready_telemetry()?;
        let render_width = self.width;
        let render_height = self.height;
        let telemetry_slot = self.telemetry.acquire();
        let telemetry_enabled = telemetry_slot.is_some();
        let interactive = alpha_min > self.manifest.policy.alpha_min + f32::EPSILON;
        let scene = scene_uniform_values(
            &self.manifest,
            self.sh_degree,
            camera,
            [render_width, render_height],
            SceneRuntime {
                time,
                temporal_cull,
                alpha_min,
                telemetry_enabled,
                interactive,
            },
        );
        self.queue.write_buffer(&self.state.scene, 0, &scene);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("4DGS stream frame"),
            });
        if telemetry_enabled {
            encoder.write_timestamp(&self.timestamp_queries, 0);
        }
        encoder.clear_buffer(&self.state.counters, 0, None);
        encoder.clear_buffer(&self.state.draw, 0, None);
        encoder.clear_buffer(&self.tile.counts, 0, None);
        for rank_mask in &self.tile.rank_masks {
            encoder.clear_buffer(rank_mask, 0, None);
        }
        compute(
            &mut encoder,
            &self.pipelines.preprocess,
            &self.bindings.preprocess,
            self.manifest.gaussian_count.div_ceil(256),
        );
        if telemetry_enabled {
            encoder.write_timestamp(&self.timestamp_queries, 1);
        }
        compute(
            &mut encoder,
            &self.pipelines.indirect,
            &self.bindings.indirect,
            1,
        );
        for pass in 0..4 {
            compute_indirect(
                &mut encoder,
                &self.pipelines.histogram,
                &self.bindings.histogram[pass],
                &self.state.dispatch,
            );
            compute_indirect(
                &mut encoder,
                &self.pipelines.scatter,
                &self.bindings.scatter[pass],
                &self.state.dispatch,
            );
        }
        compute_indirect(
            &mut encoder,
            &self.pipelines.equal_depth,
            &self.bindings.equal_depth,
            &self.state.dispatch,
        );
        if telemetry_enabled {
            encoder.write_timestamp(&self.timestamp_queries, 2);
        }
        compute(
            &mut encoder,
            &self.pipelines.tile_bin,
            &self.tile_bindings.bin,
            self.manifest.gaussian_count.div_ceil(256),
        );
        if telemetry_enabled {
            encoder.write_timestamp(&self.timestamp_queries, 3);
        }
        compute_2d(
            &mut encoder,
            &self.pipelines.tile_render,
            &self.tile_bindings.render,
            self.tile.columns,
            self.tile.rows,
        );
        if telemetry_enabled {
            encoder.write_timestamp(&self.timestamp_queries, 4);
        }
        {
            let view = self.outputs[slot].texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stream resolve to DMA-BUF"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipelines.resolve);
            pass.set_bind_group(0, &self.resolve_binding, &[]);
            pass.draw(0..3, 0..1);
        }
        if let Some(telemetry_slot) = telemetry_slot {
            encoder.write_timestamp(&self.timestamp_queries, 5);
            let readback = &self.telemetry.slots[telemetry_slot].readback;
            encoder.copy_buffer_to_buffer(&self.state.counters, 0, readback, 0, COUNTER_BYTES);
            encoder.resolve_query_set(
                &self.timestamp_queries,
                0..TIMESTAMP_COUNT,
                &self.timestamp_resolve,
                0,
            );
            encoder.copy_buffer_to_buffer(
                &self.timestamp_resolve,
                0,
                readback,
                COUNTER_BYTES,
                TIMESTAMP_BYTES,
            );
            encoder.copy_buffer_to_buffer(
                &self.tile.persistent_flags,
                0,
                readback,
                COUNTER_BYTES + TIMESTAMP_BYTES,
                PERSISTENT_FLAG_BYTES,
            );
        }
        let started = Instant::now();
        self.queue.submit([encoder.finish()]);
        if let Some(telemetry_slot) = telemetry_slot {
            self.telemetry.start_map(telemetry_slot, frame_id);
        }
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("wait for DMA-BUF render completion")?;
        self.collect_ready_telemetry()?;
        Ok(StreamFrame {
            frame_id,
            slot,
            render_wait_ms: started.elapsed().as_secs_f64() * 1000.0,
            preprocess_ms: self.last_gpu_timings[0],
            sort_ms: self.last_gpu_timings[1],
            tile_bin_ms: self.last_gpu_timings[2],
            tile_render_ms: self.last_gpu_timings[3],
            splat_ms: self.last_gpu_timings[2] + self.last_gpu_timings[3],
            resolve_ms: self.last_gpu_timings[4],
            gpu_ms: self.last_gpu_timings[5],
            render_scale: 1.0,
            lod_alpha_min: alpha_min as f64,
            counters: self.last_counters,
        })
    }

    fn collect_ready_telemetry(&mut self) -> Result<()> {
        let mut samples = self.telemetry.collect_ready()?;
        samples.sort_by_key(|sample| sample.frame_id);
        for sample in samples {
            if sample.frame_id <= self.last_telemetry_frame {
                continue;
            }
            let counter_bytes = &sample.bytes[..COUNTER_BYTES as usize];
            let words: &[u32] = bytemuck::cast_slice(counter_bytes);
            let persistent_offset = (COUNTER_BYTES + TIMESTAMP_BYTES) as usize;
            let persistent_flags = u32::from_ne_bytes(
                sample.bytes[persistent_offset..persistent_offset + 4]
                    .try_into()
                    .unwrap(),
            );
            self.last_counters = FrameCounters {
                active: words[0],
                visible: words[1],
                invalid: words[2],
                culled_temporal: words[3],
                culled_frustum: words[4],
                culled_footprint: words[5],
                equal_depth: words[6],
                tile_overlaps: words[7],
                tile_overflow: words[8] | (persistent_flags & 1),
                max_tile_load: words[9],
                early_terminated_pixels: words[10],
                pixel_splat_tests: words[11],
                budget_limited_pixels: words[12],
                max_pixel_splat_tests: words[13],
                max_budget_remaining_transmittance: f32::from_bits(words[14]),
                persistent_workload_flags: persistent_flags,
            };
            let timestamp_offset = COUNTER_BYTES as usize;
            let timestamp_bytes =
                &sample.bytes[timestamp_offset..timestamp_offset + TIMESTAMP_BYTES as usize];
            let mut timestamps = [0_u64; TIMESTAMP_COUNT as usize];
            for (target, bytes) in timestamps.iter_mut().zip(timestamp_bytes.chunks_exact(8)) {
                *target = u64::from_ne_bytes(bytes.try_into().unwrap());
            }
            let elapsed_ms = |start: usize, end: usize| {
                timestamps[end].wrapping_sub(timestamps[start]) as f64 * self.timestamp_period_ns
                    / 1_000_000.0
            };
            self.last_gpu_timings = [
                elapsed_ms(0, 1),
                elapsed_ms(1, 2),
                elapsed_ms(2, 3),
                elapsed_ms(3, 4),
                elapsed_ms(4, 5),
                elapsed_ms(0, 5),
            ];
            self.last_telemetry_frame = sample.frame_id;
        }
        Ok(())
    }
}

pub fn fixed_camera(camera: &FixedCamera, width: u32, height: u32) -> CameraUniform {
    let (world_to_camera, intrinsics, eye) = fixed_camera_uniform(camera, width, height);
    CameraUniform {
        world_to_camera,
        intrinsics,
        near: camera.near,
        far: camera.far,
        eye,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PassIndex {
    value: u32,
    padding: [u32; 3],
}

pub async fn render_once(
    asset: &Asset,
    shader_dir: &Path,
    width: u32,
    height: u32,
    time: f32,
    alpha_min: f32,
    tile_renderer: bool,
) -> Result<FrameResult> {
    ensure!(width > 0 && height > 0, "resolution must be non-zero");
    ensure!(alpha_min.is_finite() && alpha_min > 0.0 && alpha_min < 1.0);
    ensure!(
        tile_renderer || asset.manifest.policy.transmittance_epsilon.is_none(),
        "the direct raster reference path cannot honor explicit transmittance termination; use the tile renderer"
    );
    let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
    instance_desc.backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(instance_desc);
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        })
        .await
        .context("request Vulkan adapter")?;
    let info = adapter.get_info();
    let external_feature = wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
    ensure!(
        adapter.features().contains(external_feature),
        "Vulkan adapter does not expose DMA-BUF external memory"
    );
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("4DGS renderer"),
            required_features: external_feature,
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .context("request Vulkan device")?;

    let shader_bundle_sha256 = shader::renderer_bundle_sha256(shader_dir)?;
    let state = FrameState::new(&device, asset);
    let pipelines = Pipelines::new(&device, shader_dir)?;
    let bindings = Bindings::new(&device, &state, &pipelines);
    let interactive = alpha_min > asset.manifest.policy.alpha_min + f32::EPSILON;
    let scene_bytes = scene_uniform(
        asset,
        width,
        height,
        SceneRuntime {
            time,
            temporal_cull: true,
            alpha_min,
            telemetry_enabled: true,
            interactive,
        },
    );
    queue.write_buffer(&state.scene, 0, &scene_bytes);
    queue.write_buffer(
        &state.background,
        0,
        bytemuck::cast_slice(&asset.manifest.render.background),
    );

    let accumulation = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("front-to-back accumulation"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let tile = TileState::new(&device, asset.manifest.gaussian_count, width, height)?;
    let tile_bindings = TileBindings::new(
        &device,
        &state,
        &tile,
        &pipelines,
        &accumulation.create_view(&Default::default()),
    );
    let output = ExternalImage::create(&device, width, height)?;
    let resolve_binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("resolve binding"),
        layout: &pipelines.resolve.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &accumulation.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: state.background.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: state.scene.as_entire_binding(),
            },
        ],
    });

    let padded_bpr = align(width * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let image_readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("one-off image validation readback"),
        size: padded_bpr as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let counter_readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("one-off counter validation readback"),
        size: COUNTER_BYTES + PERSISTENT_FLAG_BYTES,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("4DGS native frame"),
    });
    encoder.clear_buffer(&state.counters, 0, None);
    encoder.clear_buffer(&state.draw, 0, None);
    encoder.clear_buffer(&tile.counts, 0, None);
    for rank_mask in &tile.rank_masks {
        encoder.clear_buffer(rank_mask, 0, None);
    }
    compute(
        &mut encoder,
        &pipelines.preprocess,
        &bindings.preprocess,
        asset.manifest.gaussian_count.div_ceil(256),
    );
    compute(&mut encoder, &pipelines.indirect, &bindings.indirect, 1);
    for pass in 0..4 {
        compute_indirect(
            &mut encoder,
            &pipelines.histogram,
            &bindings.histogram[pass],
            &state.dispatch,
        );
        compute_indirect(
            &mut encoder,
            &pipelines.scatter,
            &bindings.scatter[pass],
            &state.dispatch,
        );
    }
    compute_indirect(
        &mut encoder,
        &pipelines.equal_depth,
        &bindings.equal_depth,
        &state.dispatch,
    );
    if tile_renderer {
        compute(
            &mut encoder,
            &pipelines.tile_bin,
            &tile_bindings.bin,
            asset.manifest.gaussian_count.div_ceil(256),
        );
        compute_2d(
            &mut encoder,
            &pipelines.tile_render,
            &tile_bindings.render,
            tile.columns,
            tile.rows,
        );
    } else {
        let accumulation_view = accumulation.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("splat render"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &accumulation_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipelines.splat);
        pass.set_bind_group(0, &bindings.splat, &[]);
        pass.draw_indirect(&state.draw, 0);
    }
    {
        let output_view = output.texture.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipelines.resolve);
        pass.set_bind_group(0, &resolve_binding, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_buffer_to_buffer(&state.counters, 0, &counter_readback, 0, COUNTER_BYTES);
    encoder.copy_buffer_to_buffer(
        &tile.persistent_flags,
        0,
        &counter_readback,
        COUNTER_BYTES,
        PERSISTENT_FLAG_BYTES,
    );
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &output.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &image_readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bpr),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let started = Instant::now();
    queue.submit([encoder.finish()]);
    let counters_raw = map_read(&device, &counter_readback)?;
    let image_raw = map_read(&device, &image_readback)?;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let words: &[u32] = bytemuck::cast_slice(&counters_raw);
    let persistent_flags = words[COUNTER_BYTES as usize / 4];
    let counters = FrameCounters {
        active: words[0],
        visible: words[1],
        invalid: words[2],
        culled_temporal: words[3],
        culled_frustum: words[4],
        culled_footprint: words[5],
        equal_depth: words[6],
        tile_overlaps: words[7],
        tile_overflow: words[8] | (persistent_flags & 1),
        max_tile_load: words[9],
        early_terminated_pixels: words[10],
        pixel_splat_tests: words[11],
        budget_limited_pixels: words[12],
        max_pixel_splat_tests: words[13],
        max_budget_remaining_transmittance: f32::from_bits(words[14]),
        persistent_workload_flags: persistent_flags,
    };
    let mut rgba8 = vec![0; width as usize * height as usize * 4];
    for row in 0..height as usize {
        let source =
            &image_raw[row * padded_bpr as usize..row * padded_bpr as usize + width as usize * 4];
        let target = &mut rgba8[row * width as usize * 4..(row + 1) * width as usize * 4];
        for (src, dst) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
            dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
        }
    }
    let media = media::encode_one(&output, width, height, &rgba8)?;

    Ok(FrameResult {
        adapter: info.name,
        backend: format!("{:?}", info.backend),
        driver: info.driver,
        driver_info: info.driver_info,
        shader_bundle_sha256,
        width,
        height,
        time,
        submit_wait_ms: elapsed,
        counters,
        dmabuf: output.layout.clone(),
        media,
        rgba8,
    })
}

fn align(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn map_read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>> {
    let slice = buffer.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .context("poll GPU")?;
    rx.recv()
        .context("receive map result")?
        .context("map validation buffer")?;
    let bytes = slice
        .get_mapped_range()
        .map_err(|error| anyhow::anyhow!("get mapped range: {error}"))?
        .to_vec();
    buffer.unmap();
    Ok(bytes)
}

fn compute(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    binding: &wgpu::BindGroup,
    x: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("compute"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, binding, &[]);
    pass.dispatch_workgroups(x, 1, 1);
}

fn compute_2d(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    binding: &wgpu::BindGroup,
    x: u32,
    y: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("2D compute"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, binding, &[]);
    pass.dispatch_workgroups(x, y, 1);
}

fn compute_indirect(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    binding: &wgpu::BindGroup,
    dispatch: &wgpu::Buffer,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("indirect compute"),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, binding, &[]);
    pass.dispatch_workgroups_indirect(dispatch, 0);
}

fn scene_uniform(asset: &Asset, width: u32, height: u32, runtime: SceneRuntime) -> [u8; 160] {
    let camera = fixed_camera(&asset.manifest.camera.fixed, width, height);
    scene_uniform_values(
        &asset.manifest,
        asset.sh_degree(),
        &camera,
        [width, height],
        runtime,
    )
}

fn scene_uniform_values(
    manifest: &crate::asset::Manifest,
    sh_degree: u32,
    camera: &CameraUniform,
    viewport: [u32; 2],
    runtime: SceneRuntime,
) -> [u8; 160] {
    let [width, height] = viewport;
    let mut words = [0_u32; 40];
    set_floats(&mut words, 0, &camera.world_to_camera);
    set_floats(&mut words, 16, &camera.intrinsics);
    set_floats(
        &mut words,
        20,
        &[width as f32, height as f32, camera.near, camera.far],
    );
    set_floats(
        &mut words,
        24,
        &[
            runtime.time,
            manifest.time.max_duration,
            runtime.alpha_min,
            manifest.policy.temporal_threshold,
        ],
    );
    set_floats(
        &mut words,
        28,
        &raster_policy_values(manifest, runtime.alpha_min),
    );
    set_floats(
        &mut words,
        32,
        &[
            camera.eye[0],
            camera.eye[1],
            camera.eye[2],
            manifest.policy.low_pass,
        ],
    );
    words[36..40].copy_from_slice(&[
        manifest.gaussian_count,
        runtime.temporal_cull as u32,
        sh_degree,
        scene_flags(manifest, runtime.telemetry_enabled, runtime.interactive),
    ]);
    bytemuck::cast(words)
}

fn raster_policy_values(manifest: &crate::asset::Manifest, alpha_min: f32) -> [f32; 4] {
    match (
        manifest.policy.alpha_cap,
        manifest.policy.pixel_alpha_min,
        manifest.policy.transmittance_epsilon,
    ) {
        (Some(alpha_cap), Some(pixel_alpha_min), Some(transmittance_epsilon)) => {
            [alpha_cap, pixel_alpha_min, transmittance_epsilon, 0.0]
        }
        (None, None, None) => [
            LEGACY_ALPHA_CAP,
            alpha_min,
            LEGACY_TRANSMITTANCE_MIN.max(alpha_min),
            0.0,
        ],
        _ => unreachable!("validated raster policy is either absent or complete"),
    }
}

fn asset_flags(manifest: &crate::asset::Manifest) -> u32 {
    let linear_to_srgb =
        u32::from(manifest.render.working_space == "linear-rgb") * SCENE_FLAG_LINEAR_TO_SRGB;
    let opacity_compensation = u32::from(
        manifest
            .policy
            .opacity_compensation
            .as_deref()
            .unwrap_or("determinant-ratio")
            == "determinant-ratio",
    ) * SCENE_FLAG_OPACITY_COMPENSATION;
    let explicit_raster_policy =
        u32::from(manifest.policy.alpha_cap.is_some()) * SCENE_FLAG_EXPLICIT_RASTER_POLICY;
    linear_to_srgb | opacity_compensation | explicit_raster_policy
}

fn scene_flags(
    manifest: &crate::asset::Manifest,
    telemetry_enabled: bool,
    interactive: bool,
) -> u32 {
    let mut flags = asset_flags(manifest);
    if telemetry_enabled {
        flags |= SCENE_FLAG_TELEMETRY;
    }
    if interactive {
        flags |= SCENE_FLAG_INTERACTIVE;
    }
    flags
}

fn set_floats(target: &mut [u32], offset: usize, values: &[f32]) {
    for (index, value) in values.iter().enumerate() {
        target[offset + index] = value.to_bits();
    }
}

fn fixed_camera_uniform(
    camera: &FixedCamera,
    width: u32,
    height: u32,
) -> ([f32; 16], [f32; 4], [f32; 3]) {
    let scale = (width as f32 / camera.source_size[0] as f32)
        .min(height as f32 / camera.source_size[1] as f32);
    let offset_x = (width as f32 - camera.source_size[0] as f32 * scale) * 0.5;
    let offset_y = (height as f32 - camera.source_size[1] as f32 * scale) * 0.5;
    let rows = camera.world_to_camera_row_major;
    let translation = [rows[0][3], rows[1][3], rows[2][3]];
    let eye = [
        -(rows[0][0] * translation[0] + rows[1][0] * translation[1] + rows[2][0] * translation[2]),
        -(rows[0][1] * translation[0] + rows[1][1] * translation[1] + rows[2][1] * translation[2]),
        -(rows[0][2] * translation[0] + rows[1][2] * translation[1] + rows[2][2] * translation[2]),
    ];
    let mut matrix = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            matrix[column * 4 + row] = rows[row][column];
        }
    }
    let [fx, fy, cx, cy] = camera.intrinsics;
    (
        matrix,
        [
            fx * scale,
            fy * scale,
            cx * scale + offset_x,
            cy * scale + offset_y,
        ],
        eye,
    )
}

struct FrameState {
    gaussians: wgpu::Buffer,
    sh_coefficients: wgpu::Buffer,
    screens: wgpu::Buffer,
    keys_a: wgpu::Buffer,
    keys_b: wgpu::Buffer,
    ids_a: wgpu::Buffer,
    ids_b: wgpu::Buffer,
    counters: wgpu::Buffer,
    dispatch: wgpu::Buffer,
    radix_params: wgpu::Buffer,
    draw: wgpu::Buffer,
    histograms: wgpu::Buffer,
    scene: wgpu::Buffer,
    background: wgpu::Buffer,
    pass_indices: [wgpu::Buffer; 4],
}

impl FrameState {
    fn new(device: &wgpu::Device, asset: &Asset) -> Self {
        let count = asset.manifest.gaussian_count.max(1) as u64;
        let storage_copy = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST;
        let gaussians = init(
            device,
            "4D Gaussian records",
            &asset.records,
            wgpu::BufferUsages::STORAGE,
        );
        let sh0_unused = [0_u8; 4];
        let (sh_label, sh_bytes) = asset.sh_coefficients.as_deref().map_or(
            ("SH0 unused coefficients", sh0_unused.as_slice()),
            |coefficients| ("SH3 appearance coefficients", coefficients),
        );
        let sh_coefficients = init(device, sh_label, sh_bytes, wgpu::BufferUsages::STORAGE);
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            gaussians,
            sh_coefficients,
            screens: buffer(
                "compacted screen Gaussians",
                count * SCREEN_RECORD_BYTES,
                storage_copy,
            ),
            keys_a: buffer("depth keys A", count * 4, storage_copy),
            keys_b: buffer("depth keys B", count * 4, storage_copy),
            ids_a: buffer("instance ids A", count * 4, storage_copy),
            ids_b: buffer("instance ids B", count * 4, storage_copy),
            counters: buffer("frame counters", COUNTER_BYTES, storage_copy),
            dispatch: buffer(
                "radix dispatch indirect",
                12,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            radix_params: buffer("radix parameters", 64, storage_copy),
            draw: buffer(
                "draw indirect",
                16,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            histograms: buffer("radix histograms", HISTOGRAM_BYTES, storage_copy),
            scene: buffer(
                "scene uniform",
                160,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            background: buffer(
                "resolve background",
                16,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            pass_indices: std::array::from_fn(|pass| {
                init(
                    device,
                    "radix pass",
                    bytemuck::bytes_of(&PassIndex {
                        value: pass as u32,
                        padding: [0; 3],
                    }),
                    wgpu::BufferUsages::UNIFORM,
                )
            }),
        }
    }
}

struct TileState {
    counts: wgpu::Buffer,
    rank_masks: [wgpu::Buffer; TILE_MASK_SHARDS],
    persistent_flags: wgpu::Buffer,
    columns: u32,
    rows: u32,
}

impl TileState {
    fn new(device: &wgpu::Device, gaussian_count: u32, width: u32, height: u32) -> Result<Self> {
        let columns = width.div_ceil(TILE_SIZE);
        let rows = height.div_ceil(TILE_SIZE);
        let tile_count = columns as u64 * rows as u64;
        let rank_words_per_tile = (gaussian_count as u64).div_ceil(TILE_MASK_RANKS_PER_BIT * 32);
        let level_one_words_per_tile = rank_words_per_tile.div_ceil(32);
        let level_two_words_per_tile = level_one_words_per_tile.div_ceil(32);
        let mask_words_per_tile =
            rank_words_per_tile + level_one_words_per_tile + level_two_words_per_tile;
        let tiles_per_shard = tile_count.div_ceil(TILE_MASK_SHARDS as u64);
        let mask_bytes = tiles_per_shard * mask_words_per_tile * 4;
        ensure!(
            mask_bytes <= device.limits().max_storage_buffer_binding_size,
            "tile rank mask needs {mask_bytes} bytes, exceeding max storage binding {}",
            device.limits().max_storage_buffer_binding_size
        );
        let counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile splat counts"),
            size: tile_count * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rank_masks = std::array::from_fn(|shard| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("tile exact depth-rank mask shard {shard}")),
                size: mask_bytes.max(4),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let persistent_flags = init(
            device,
            "persistent tile workload flags",
            &[0_u8; PERSISTENT_FLAG_BYTES as usize],
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        Ok(Self {
            counts,
            rank_masks,
            persistent_flags,
            columns,
            rows,
        })
    }
}

struct TileBindings {
    bin: wgpu::BindGroup,
    render: wgpu::BindGroup,
}

impl TileBindings {
    fn new(
        device: &wgpu::Device,
        state: &FrameState,
        tile: &TileState,
        pipelines: &Pipelines,
        accumulation: &wgpu::TextureView,
    ) -> Self {
        let bin = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tile bin"),
            layout: &pipelines.tile_bin.get_bind_group_layout(0),
            entries: &[
                b(0, &state.scene),
                b(1, &state.screens),
                b(2, &state.ids_a),
                b(3, &tile.counts),
                b(4, &tile.rank_masks[0]),
                b(5, &tile.rank_masks[1]),
                b(6, &tile.rank_masks[2]),
                b(7, &state.counters),
                b(8, &tile.persistent_flags),
            ],
        });
        let render = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tile render"),
            layout: &pipelines.tile_render.get_bind_group_layout(0),
            entries: &[
                b(0, &state.scene),
                b(1, &state.screens),
                b(2, &state.ids_a),
                b(3, &tile.counts),
                b(4, &tile.rank_masks[0]),
                b(5, &tile.rank_masks[1]),
                b(6, &tile.rank_masks[2]),
                b(7, &state.counters),
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(accumulation),
                },
                b(9, &tile.persistent_flags),
            ],
        });
        Self { bin, render }
    }
}

fn init(
    device: &wgpu::Device,
    label: &str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytes,
        usage: usage | wgpu::BufferUsages::COPY_DST,
    })
}

struct Pipelines {
    preprocess: wgpu::ComputePipeline,
    indirect: wgpu::ComputePipeline,
    histogram: wgpu::ComputePipeline,
    scatter: wgpu::ComputePipeline,
    equal_depth: wgpu::ComputePipeline,
    tile_bin: wgpu::ComputePipeline,
    tile_render: wgpu::ComputePipeline,
    splat: wgpu::RenderPipeline,
    resolve: wgpu::RenderPipeline,
}

impl Pipelines {
    fn new(device: &wgpu::Device, dir: &Path) -> Result<Self> {
        let module = |name: &str| -> Result<wgpu::ShaderModule> {
            let path = dir.join(name);
            let source = shader::load_with_includes(&path)?;
            Ok(device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            }))
        };
        let preprocess_m = module("preprocess.wgsl")?;
        let indirect_m = module("build-indirect.wgsl")?;
        let histogram_m = module("radix-histogram.wgsl")?;
        let scatter_m = module("radix-scatter.wgsl")?;
        let equal_m = module("count-equal-depth.wgsl")?;
        let tile_bin_m = module("tile-bin.wgsl")?;
        let tile_render_m = module("tile-render.wgsl")?;
        let splat_m = module("splat.wgsl")?;
        let resolve_m = module("resolve.wgsl")?;
        let compute = |label, shader: &wgpu::ShaderModule| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: None,
                module: shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let vertex = |module| wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        };
        let splat = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("splat"),
            layout: None,
            vertex: vertex(&splat_m),
            fragment: Some(wgpu::FragmentState {
                module: &splat_m,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            operation: wgpu::BlendOperation::Add,
                            src_factor: wgpu::BlendFactor::DstAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                        },
                        alpha: wgpu::BlendComponent {
                            operation: wgpu::BlendOperation::Add,
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let resolve = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("resolve"),
            layout: None,
            vertex: vertex(&resolve_m),
            fragment: Some(wgpu::FragmentState {
                module: &resolve_m,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Ok(Self {
            preprocess: compute("preprocess", &preprocess_m),
            indirect: compute("indirect", &indirect_m),
            histogram: compute("radix histogram", &histogram_m),
            scatter: compute("radix scatter", &scatter_m),
            equal_depth: compute("equal depth audit", &equal_m),
            tile_bin: compute("tile bin", &tile_bin_m),
            tile_render: compute("tile render", &tile_render_m),
            splat,
            resolve,
        })
    }
}

struct Bindings {
    preprocess: wgpu::BindGroup,
    indirect: wgpu::BindGroup,
    histogram: [wgpu::BindGroup; 4],
    scatter: [wgpu::BindGroup; 4],
    equal_depth: wgpu::BindGroup,
    splat: wgpu::BindGroup,
}

impl Bindings {
    fn new(device: &wgpu::Device, s: &FrameState, p: &Pipelines) -> Self {
        let bind =
            |label: &str, layout: &wgpu::BindGroupLayout, entries: &[wgpu::BindGroupEntry]| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout,
                    entries,
                })
            };
        let preprocess = bind(
            "preprocess",
            &p.preprocess.get_bind_group_layout(0),
            &[
                b(0, &s.scene),
                b(1, &s.gaussians),
                b(2, &s.screens),
                b(3, &s.keys_a),
                b(4, &s.ids_a),
                b(5, &s.counters),
                b(6, &s.sh_coefficients),
            ],
        );
        let indirect = bind(
            "indirect",
            &p.indirect.get_bind_group_layout(0),
            &[
                b(0, &s.counters),
                b(1, &s.dispatch),
                b(2, &s.radix_params),
                b(3, &s.draw),
            ],
        );
        let histogram = std::array::from_fn(|pass| {
            let input = if pass % 2 == 0 { &s.keys_a } else { &s.keys_b };
            bind(
                "histogram",
                &p.histogram.get_bind_group_layout(0),
                &[
                    b(0, &s.pass_indices[pass]),
                    b(1, &s.radix_params),
                    b(2, input),
                    b(3, &s.histograms),
                ],
            )
        });
        let scatter = std::array::from_fn(|pass| {
            let (ki, ko, ii, io) = if pass % 2 == 0 {
                (&s.keys_a, &s.keys_b, &s.ids_a, &s.ids_b)
            } else {
                (&s.keys_b, &s.keys_a, &s.ids_b, &s.ids_a)
            };
            bind(
                "scatter",
                &p.scatter.get_bind_group_layout(0),
                &[
                    b(0, &s.pass_indices[pass]),
                    b(1, &s.radix_params),
                    b(2, ki),
                    b(3, ko),
                    b(4, ii),
                    b(5, io),
                    b(6, &s.histograms),
                ],
            )
        });
        let equal_depth = bind(
            "equal depth",
            &p.equal_depth.get_bind_group_layout(0),
            &[b(0, &s.radix_params), b(1, &s.keys_a), b(2, &s.counters)],
        );
        let splat = bind(
            "splat",
            &p.splat.get_bind_group_layout(0),
            &[b(0, &s.scene), b(1, &s.screens), b(2, &s.ids_a)],
        );
        Self {
            preprocess,
            indirect,
            histogram,
            scatter,
            equal_depth,
            splat,
        }
    }
}

fn b(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::asset::Manifest;

    use super::{
        LEGACY_ALPHA_CAP, LEGACY_TRANSMITTANCE_MIN, SCENE_FLAG_EXPLICIT_RASTER_POLICY,
        SCENE_FLAG_INTERACTIVE, SCENE_FLAG_LINEAR_TO_SRGB, SCENE_FLAG_OPACITY_COMPENSATION,
        SCENE_FLAG_TELEMETRY, SceneRuntime, fixed_camera, scene_uniform_values,
    };

    fn manifest() -> Manifest {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/minimal-sh0/manifest.json");
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn word(scene: &[u8; 160], index: usize) -> u32 {
        u32::from_ne_bytes(scene[index * 4..index * 4 + 4].try_into().unwrap())
    }

    fn scalar(scene: &[u8; 160], index: usize) -> f32 {
        f32::from_bits(word(scene, index))
    }

    #[test]
    fn omitted_raster_policy_preserves_legacy_uniform_values() {
        let manifest = manifest();
        let alpha_min = 8.0 / 255.0;
        let camera = fixed_camera(&manifest.camera.fixed, 640, 360);
        let scene = scene_uniform_values(
            &manifest,
            0,
            &camera,
            [640, 360],
            SceneRuntime {
                time: 0.5,
                temporal_cull: true,
                alpha_min,
                telemetry_enabled: true,
                interactive: true,
            },
        );

        assert_eq!(scalar(&scene, 28), LEGACY_ALPHA_CAP);
        assert_eq!(scalar(&scene, 29), alpha_min);
        assert_eq!(scalar(&scene, 30), LEGACY_TRANSMITTANCE_MIN.max(alpha_min));
        assert_eq!(
            word(&scene, 39),
            SCENE_FLAG_TELEMETRY | SCENE_FLAG_INTERACTIVE | SCENE_FLAG_OPACITY_COMPENSATION
        );
    }

    #[test]
    fn one_frame_uniform_keeps_asset_flags_and_explicit_classic_policy() {
        let mut manifest = manifest();
        manifest.render.working_space = "linear-rgb".into();
        manifest.render.output_transfer = Some("srgb".into());
        manifest.policy.opacity_compensation = Some("none".into());
        manifest.policy.alpha_cap = Some(0.999);
        manifest.policy.pixel_alpha_min = Some(1.0 / 255.0);
        manifest.policy.transmittance_epsilon = Some(1.0e-4);
        let camera = fixed_camera(&manifest.camera.fixed, 1280, 720);
        let scene = scene_uniform_values(
            &manifest,
            0,
            &camera,
            [1280, 720],
            SceneRuntime {
                time: 0.5,
                temporal_cull: true,
                alpha_min: manifest.policy.alpha_min,
                telemetry_enabled: true,
                interactive: false,
            },
        );

        assert_eq!(scalar(&scene, 28), 0.999);
        assert_eq!(scalar(&scene, 29), 1.0 / 255.0);
        assert_eq!(scalar(&scene, 30), 1.0e-4);
        assert_eq!(
            word(&scene, 39),
            SCENE_FLAG_TELEMETRY | SCENE_FLAG_LINEAR_TO_SRGB | SCENE_FLAG_EXPLICIT_RASTER_POLICY
        );
    }
}
