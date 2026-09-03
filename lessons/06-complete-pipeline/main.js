import {
  createGpuContext,
  withGpuErrorScopes,
} from '../infra/gpu.js';
import { createLessonSurface } from '../infra/page.js';
import { ASSET_INPUT, loadLessonAsset } from './asset-contract.js';
import {
  colorDistance,
  compositeAtNdc,
  evaluateReference,
} from './reference.js';

const RECORD_STRIDE = 64;
const PROJECTED_STRIDE = 64;
const SORT_ENTRY_STRIDE = 16;
const COUNTER_BYTES = 16;
const DRAW_ARGS_BYTES = 16;
const TEXTURE_BYTES_PER_ROW = 256;
const WORKGROUP_SIZE = 64;
const PROJECTION_TOLERANCE = 0.001;
const PIXEL_TOLERANCE = 5 / 255;

const canvas = document.querySelector('canvas');
const hud = document.querySelector('#hud');
const surface = createLessonSurface(6);

async function start() {
  surface.progress('resolving and validating the asset input');
  const asset = await loadLessonAsset(ASSET_INPUT);
  const { manifest, records, source } = asset;
  let time = manifest.time.initial;

  const gpu = await createGpuContext(canvas);
  const { device, context, format } = gpu;
  device.addEventListener('uncapturederror', (event) => {
    surface.fail(new Error(`Uncaptured WebGPU error: ${event.error.message}`));
  });
  device.lost.then((info) => {
    surface.fail(new Error(`WebGPU device lost (${info.reason}): ${info.message}`));
  });

  surface.progress('compiling projection, sorting and splat stages');
  const [computeCode, renderCode] = await Promise.all([
    fetchText(new URL('./complete-pipeline.wgsl', import.meta.url)),
    fetchText(new URL('./complete-pipeline-render.wgsl', import.meta.url)),
  ]);
  const [computeShader, renderShader] = await Promise.all([
    compileShader(device, computeCode, 'lesson 06 compute shader'),
    compileShader(device, renderCode, 'lesson 06 render shader'),
  ]);
  const warningCount = computeShader.warningCount + renderShader.warningCount;
  const [resetPipeline, projectionPipeline, sortPipeline, renderPipeline] = await withGpuErrorScopes(
    device,
    async () => Promise.all([
      device.createComputePipelineAsync({
        label: 'lesson 06 reset pipeline',
        layout: 'auto',
        compute: { module: computeShader.module, entryPoint: 'reset_main' },
      }),
      device.createComputePipelineAsync({
        label: 'lesson 06 projection pipeline',
        layout: 'auto',
        compute: { module: computeShader.module, entryPoint: 'project_main' },
      }),
      device.createComputePipelineAsync({
        label: 'lesson 06 bitonic sort pipeline',
        layout: 'auto',
        compute: { module: computeShader.module, entryPoint: 'sort_main' },
      }),
      device.createRenderPipelineAsync({
        label: 'lesson 06 ordered splat pipeline',
        layout: 'auto',
        vertex: { module: renderShader.module, entryPoint: 'vs_main' },
        fragment: {
          module: renderShader.module,
          entryPoint: 'fs_main',
          targets: [{
            format,
            blend: {
              color: { srcFactor: 'src-alpha', dstFactor: 'one-minus-src-alpha' },
              alpha: { srcFactor: 'one', dstFactor: 'one-minus-src-alpha' },
            },
          }],
        },
        primitive: { topology: 'triangle-list' },
      }),
    ]),
  );

  const recordBuffer = createBufferWithData(
    device,
    'lesson 06 validated records',
    encodeRecords(records),
    GPUBufferUsage.STORAGE,
  );
  const projectedBuffer = device.createBuffer({
    label: 'lesson 06 projected records',
    size: records.length * PROJECTED_STRIDE,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });
  const sortBuffer = device.createBuffer({
    label: 'lesson 06 sort entries',
    size: records.length * SORT_ENTRY_STRIDE,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });
  const paramsBuffer = device.createBuffer({
    label: 'lesson 06 frame parameters',
    size: 32,
    usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
  });
  const counterBuffer = device.createBuffer({
    label: 'lesson 06 active-set counters',
    size: COUNTER_BYTES,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });
  const drawArgsBuffer = device.createBuffer({
    label: 'lesson 06 indirect draw arguments',
    size: DRAW_ARGS_BYTES,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.INDIRECT | GPUBufferUsage.COPY_SRC,
  });
  const auditTexture = device.createTexture({
    label: 'lesson 06 one-pixel render audit',
    size: { width: 1, height: 1 },
    format,
    usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC,
  });
  const auditLayout = createAuditLayout(records.length);
  const readbackBuffer = device.createBuffer({
    label: 'lesson 06 compute and rendered-pixel audit',
    size: auditLayout.totalBytes,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });

  const resetGroup = device.createBindGroup({
    label: 'lesson 06 reset bindings',
    layout: resetPipeline.getBindGroupLayout(0),
    entries: [
      { binding: 0, resource: { buffer: recordBuffer } },
      { binding: 2, resource: { buffer: sortBuffer } },
      { binding: 5, resource: { buffer: counterBuffer } },
      { binding: 6, resource: { buffer: drawArgsBuffer } },
    ],
  });
  const projectionGroup = device.createBindGroup({
    label: 'lesson 06 projection bindings',
    layout: projectionPipeline.getBindGroupLayout(0),
    entries: [
      { binding: 0, resource: { buffer: recordBuffer } },
      { binding: 1, resource: { buffer: projectedBuffer } },
      { binding: 2, resource: { buffer: sortBuffer } },
      { binding: 3, resource: { buffer: paramsBuffer } },
      { binding: 5, resource: { buffer: counterBuffer } },
      { binding: 6, resource: { buffer: drawArgsBuffer } },
    ],
  });
  const renderGroup = device.createBindGroup({
    label: 'lesson 06 render bindings',
    layout: renderPipeline.getBindGroupLayout(0),
    entries: [
      { binding: 1, resource: { buffer: projectedBuffer } },
      { binding: 2, resource: { buffer: sortBuffer } },
      { binding: 3, resource: { buffer: paramsBuffer } },
    ],
  });
  const sortStages = createSortStages(device, sortPipeline, sortBuffer, records.length);

  let frameCount = 0;
  let drawing = false;
  let pendingDraw = false;

  async function draw() {
    pendingDraw = true;
    if (drawing) return;
    drawing = true;
    try {
      while (pendingDraw) {
        pendingDraw = false;
        const { width, height } = await gpu.configureCanvas();
        const aspect = width / height;
        const reference = evaluateReference(records, manifest, { time, aspect });
        const wrongOrder = reference.projected.filter((item) => item.valid);
        const correctPixel = compositeAtNdc(
          reference.sorted,
          [0, 0],
          manifest.render.alphaMin * 0.1,
        );
        const wrongPixel = compositeAtNdc(
          wrongOrder,
          [0, 0],
          manifest.render.alphaMin * 0.1,
        );
        const orderEffect = colorDistance(correctPixel.color, wrongPixel.color);
        if (!(orderEffect > 0.01)) {
          throw new Error(`synthetic order witness is too weak: color distance ${orderEffect}`);
        }

        await withGpuErrorScopes(device, async () => {
          device.queue.writeBuffer(paramsBuffer, 0, encodeParams(manifest, time, aspect));
          const encoder = device.createCommandEncoder({ label: 'lesson 06 frame encoder' });
          encodeComputePass(
            encoder,
            'lesson 06 reset pass',
            resetPipeline,
            resetGroup,
            Math.ceil(records.length / WORKGROUP_SIZE),
          );
          encodeComputePass(
            encoder,
            'lesson 06 time, projection and compaction pass',
            projectionPipeline,
            projectionGroup,
            Math.ceil(records.length / WORKGROUP_SIZE),
          );
          for (const [index, stage] of sortStages.entries()) {
            encodeComputePass(
              encoder,
              `lesson 06 bitonic stage ${index}`,
              sortPipeline,
              stage.group,
              Math.ceil(records.length / WORKGROUP_SIZE),
            );
          }

          const pass = encoder.beginRenderPass({
            label: 'lesson 06 canvas render pass',
            colorAttachments: [{
              view: context.getCurrentTexture().createView(),
              clearValue: { r: 0.018, g: 0.018, b: 0.022, a: 1 },
              loadOp: 'clear',
              storeOp: 'store',
            }],
          });
          pass.setPipeline(renderPipeline);
          pass.setBindGroup(0, renderGroup);
          pass.drawIndirect(drawArgsBuffer, 0);
          pass.end();

          const auditPass = encoder.beginRenderPass({
            label: 'lesson 06 one-pixel audit render pass',
            colorAttachments: [{
              view: auditTexture.createView(),
              clearValue: { r: 0, g: 0, b: 0, a: 0 },
              loadOp: 'clear',
              storeOp: 'store',
            }],
          });
          auditPass.setPipeline(renderPipeline);
          auditPass.setBindGroup(0, renderGroup);
          auditPass.drawIndirect(drawArgsBuffer, 0);
          auditPass.end();

          encoder.copyBufferToBuffer(
            projectedBuffer,
            0,
            readbackBuffer,
            auditLayout.projectedOffset,
            auditLayout.projectedBytes,
          );
          encoder.copyBufferToBuffer(
            sortBuffer,
            0,
            readbackBuffer,
            auditLayout.sortOffset,
            auditLayout.sortBytes,
          );
          encoder.copyBufferToBuffer(
            counterBuffer,
            0,
            readbackBuffer,
            auditLayout.counterOffset,
            COUNTER_BYTES,
          );
          encoder.copyBufferToBuffer(
            drawArgsBuffer,
            0,
            readbackBuffer,
            auditLayout.drawArgsOffset,
            DRAW_ARGS_BYTES,
          );
          encoder.copyTextureToBuffer(
            { texture: auditTexture },
            {
              buffer: readbackBuffer,
              offset: auditLayout.pixelOffset,
              bytesPerRow: TEXTURE_BYTES_PER_ROW,
              rowsPerImage: 1,
            },
            { width: 1, height: 1, depthOrArrayLayers: 1 },
          );
          device.queue.submit([encoder.finish()]);
          await device.queue.onSubmittedWorkDone();
        });
        const audited = await readAudit(readbackBuffer, records.length, auditLayout, format);
        const pixelError = assertFrameAudit(audited, reference, correctPixel);
        frameCount += 1;

        const details = {
          adapter: gpu.adapterSummary,
          format,
          width,
          height,
          warningCount,
          frameCount,
          assetSource: source.kind,
          externalManifestUrl: source.manifestUrl,
          recordCount: records.length,
          activeCount: audited.active,
          visibleCount: audited.visible,
          indirectInstanceCount: audited.instanceCount,
          time: round(time),
          sortStageCount: sortStages.length,
          centerOrderWitness: round(orderEffect),
          centerPixelGpu: audited.pixel.color.map(round),
          centerPixelCpu: correctPixel.color.map(round),
          centerPixelMaxError: round(pixelError),
        };
        const sourceAssertions = source.kind === 'procedural'
          ? { proceduralFallbackUsed: true, externalAssetFetchSkipped: true }
          : { externalManifestAndRecordsLoaded: true };
        surface.pass(details, {
          assetInputResolved: true,
          manifestAndRecordsValidated: true,
          ...sourceAssertions,
          shaderCompiled: true,
          gpuActiveAndVisibleCountsMatchCpuReference: true,
          indirectInstanceCountMatchesVisibleCount: true,
          allGpuProjectedFieldsMatchCpuReference: true,
          gpuVisibleOrderMatchesCpuReference: true,
          gpuCenterPixelMatchesCpuReference: true,
          depthOrderChangesCompositedColor: true,
          frameSubmitted: true,
          gpuWorkCompleted: true,
        });
        hud.textContent = [
          'LESSON 06 · PASS',
          `${source.kind} · N ${audited.total} → A ${audited.active} → V ${audited.visible}`,
          `t ${time.toFixed(2)} · compact → sort(${sortStages.length}) → drawIndirect`,
          `pixel error ${pixelError.toFixed(4)} · order ΔRGB ${orderEffect.toFixed(3)}`,
          '←/→ · scrub time    H · hide diagnostics',
        ].join('\n');
      }
    } finally {
      drawing = false;
      if (pendingDraw) void draw().catch(surface.fail);
    }
  }

  addEventListener('keydown', (event) => {
    if (event.code !== 'ArrowLeft' && event.code !== 'ArrowRight') return;
    event.preventDefault();
    const direction = event.code === 'ArrowRight' ? 1 : -1;
    time = clamp(time + direction * 0.025, manifest.time.start, manifest.time.end);
    void draw().catch(surface.fail);
  });
  new ResizeObserver(() => void draw().catch(surface.fail)).observe(canvas);
  await draw();
}

start().catch(surface.fail);

async function fetchText(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Cannot load ${url.pathname}: HTTP ${response.status}`);
  return response.text();
}

async function compileShader(device, code, label) {
  return withGpuErrorScopes(device, async () => {
    const module = device.createShaderModule({ label, code });
    const compilation = await module.getCompilationInfo();
    const errors = compilation.messages.filter((message) => message.type === 'error');
    if (errors.length > 0) {
      throw new Error(errors.map((message) => {
        const location = message.lineNum ? `${message.lineNum}:${message.linePos}` : 'unknown';
        return `${location} ${message.message}`;
      }).join('\n'));
    }
    return {
      module,
      warningCount: compilation.messages.filter((message) => message.type === 'warning').length,
    };
  });
}

function encodeRecords(records) {
  const values = new Float32Array((records.length * RECORD_STRIDE) / 4);
  records.forEach((record, index) => {
    const offset = index * 16;
    values.set([...record.center, record.opacity], offset);
    values.set([...record.velocity, record.timeCenter], offset + 4);
    values.set([...record.scale, record.timeSigma], offset + 8);
    values.set([...record.color, 0], offset + 12);
  });
  return values;
}

function encodeParams(manifest, time, aspect) {
  return new Float32Array([
    aspect,
    time,
    manifest.render.alphaMin,
    manifest.camera.focalY,
    manifest.camera.near,
    manifest.camera.far,
    manifest.camera.minSigmaNdc,
    0,
  ]);
}

function createBufferWithData(device, label, typedArray, usage) {
  const buffer = device.createBuffer({
    label,
    size: typedArray.byteLength,
    usage: usage | GPUBufferUsage.COPY_DST,
  });
  device.queue.writeBuffer(buffer, 0, typedArray);
  return buffer;
}

function createSortStages(device, pipeline, sortBuffer, count) {
  const stages = [];
  for (let k = 2; k <= count; k *= 2) {
    for (let j = k / 2; j >= 1; j /= 2) {
      const buffer = createBufferWithData(
        device,
        `lesson 06 sort stage k=${k} j=${j}`,
        new Uint32Array([k, j, 0, 0]),
        GPUBufferUsage.UNIFORM,
      );
      const group = device.createBindGroup({
        label: `lesson 06 sort bindings k=${k} j=${j}`,
        layout: pipeline.getBindGroupLayout(0),
        entries: [
          { binding: 2, resource: { buffer: sortBuffer } },
          { binding: 4, resource: { buffer } },
        ],
      });
      stages.push({ k, j, buffer, group });
    }
  }
  return stages;
}

function encodeComputePass(encoder, label, pipeline, group, workgroups) {
  const pass = encoder.beginComputePass({ label });
  pass.setPipeline(pipeline);
  pass.setBindGroup(0, group);
  pass.dispatchWorkgroups(workgroups);
  pass.end();
}

async function readAudit(buffer, count, layout, format) {
  await buffer.mapAsync(GPUMapMode.READ);
  const copy = buffer.getMappedRange().slice(0);
  buffer.unmap();
  const projected = [];
  const view = new DataView(copy);
  for (let index = 0; index < count; index += 1) {
    const base = layout.projectedOffset + index * PROJECTED_STRIDE;
    projected.push({
      mean: [view.getFloat32(base, true), view.getFloat32(base + 4, true)],
      extent: [view.getFloat32(base + 8, true), view.getFloat32(base + 12, true)],
      conic: [
        view.getFloat32(base + 16, true),
        view.getFloat32(base + 20, true),
        view.getFloat32(base + 24, true),
      ],
      opacity: view.getFloat32(base + 28, true),
      color: [
        view.getFloat32(base + 32, true),
        view.getFloat32(base + 36, true),
        view.getFloat32(base + 40, true),
      ],
      depth: view.getFloat32(base + 44, true),
      sourceIndex: view.getUint32(base + 48, true),
      valid: view.getUint32(base + 52, true) === 1,
    });
  }
  const order = [];
  for (let index = 0; index < count; index += 1) {
    order.push(view.getUint32(layout.sortOffset + index * SORT_ENTRY_STRIDE + 4, true));
  }
  return {
    projected,
    order,
    total: view.getUint32(layout.counterOffset, true),
    active: view.getUint32(layout.counterOffset + 4, true),
    visible: view.getUint32(layout.counterOffset + 8, true),
    instanceCount: view.getUint32(layout.drawArgsOffset + 4, true),
    pixel: decodePixel(new Uint8Array(copy, layout.pixelOffset, 4), format),
  };
}

function assertFrameAudit(actual, expected, expectedPixel) {
  assertEqual(actual.total, expected.projected.length, 'total counter');
  assertEqual(actual.active, expected.activeCount, 'active counter');
  assertEqual(actual.visible, expected.visibleCount, 'visible counter');
  assertEqual(actual.instanceCount, actual.visible, 'indirect instance count');

  const expectedOrder = expected.sorted
    .filter((item) => item.valid)
    .map((item) => item.sourceIndex);
  const actualOrder = actual.order.slice(0, actual.visible);
  if (
    actualOrder.length !== expectedOrder.length
    || actualOrder.some((value, index) => value !== expectedOrder[index])
  ) {
    throw new Error(`GPU visible order [${actualOrder}] differs from CPU order [${expectedOrder}]`);
  }
  for (const cpu of expected.projected) {
    const gpu = actual.projected[cpu.sourceIndex];
    if (gpu.sourceIndex !== cpu.sourceIndex || gpu.valid !== cpu.valid) {
      throw new Error(`projected record ${cpu.sourceIndex} has a GPU/CPU validity mismatch`);
    }
    for (const [label, gpuValues, cpuValues] of [
      ['mean', gpu.mean, cpu.mean],
      ['extent', gpu.extent, cpu.extent],
      ['conic', gpu.conic, cpu.conic],
      ['opacity', [gpu.opacity], [cpu.opacity]],
      ['color', gpu.color, cpu.color],
      ['depth', [gpu.depth], [cpu.depth]],
    ]) {
      gpuValues.forEach((value, index) => {
        const tolerance = PROJECTION_TOLERANCE * Math.max(1, Math.abs(cpuValues[index]));
        if (!Number.isFinite(value) || Math.abs(value - cpuValues[index]) > tolerance) {
          throw new Error(
            `projected ${label} mismatch at record ${cpu.sourceIndex}: GPU ${value}, CPU ${cpuValues[index]}`,
          );
        }
      });
    }
  }

  const expectedRgba = [...expectedPixel.color, expectedPixel.alpha];
  const actualRgba = [...actual.pixel.color, actual.pixel.alpha];
  const pixelError = Math.max(
    ...actualRgba.map((value, index) => Math.abs(value - expectedRgba[index])),
  );
  if (pixelError > PIXEL_TOLERANCE) {
    throw new Error(
      `GPU center pixel [${actualRgba}] differs from CPU pixel [${expectedRgba}] by ${pixelError}`,
    );
  }
  return pixelError;
}

function assertEqual(actual, expected, label) {
  if (actual !== expected) throw new Error(`${label}: GPU ${actual}, CPU reference ${expected}`);
}

function decodePixel(bytes, format) {
  let channels;
  if (format === 'bgra8unorm') channels = [bytes[2], bytes[1], bytes[0]];
  else if (format === 'rgba8unorm') channels = [bytes[0], bytes[1], bytes[2]];
  else throw new Error(`one-pixel audit does not support canvas format ${format}`);
  return {
    color: channels.map((value) => value / 255),
    alpha: bytes[3] / 255,
    bytes: [...bytes],
  };
}

function createAuditLayout(count) {
  const projectedOffset = 0;
  const projectedBytes = count * PROJECTED_STRIDE;
  const sortOffset = projectedOffset + projectedBytes;
  const sortBytes = count * SORT_ENTRY_STRIDE;
  const counterOffset = sortOffset + sortBytes;
  const drawArgsOffset = counterOffset + COUNTER_BYTES;
  const pixelOffset = alignTo(drawArgsOffset + DRAW_ARGS_BYTES, TEXTURE_BYTES_PER_ROW);
  return {
    projectedOffset,
    projectedBytes,
    sortOffset,
    sortBytes,
    counterOffset,
    drawArgsOffset,
    pixelOffset,
    totalBytes: pixelOffset + TEXTURE_BYTES_PER_ROW,
  };
}

function alignTo(value, alignment) {
  return Math.ceil(value / alignment) * alignment;
}

function clamp(value, minimum, maximum) {
  return Math.max(minimum, Math.min(maximum, value));
}

function round(value) {
  return Math.round(value * 1000) / 1000;
}
