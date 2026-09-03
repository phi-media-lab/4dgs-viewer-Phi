import {
  createGpuContext,
  withGpuErrorScopes,
} from '../infra/gpu.js';
import { createLessonSurface } from '../infra/page.js';
import {
  ALPHA_MIN,
  INITIAL_TIME,
  classifyRecords,
  createProceduralRecords,
} from './reference.js';

const RECORD_STRIDE = 48;
const COUNTER_BYTES = 16;
const DRAW_ARGS_BYTES = 16;
const INDEX_BYTES = 4;
const WORKGROUP_SIZE = 64;

const canvas = document.querySelector('canvas');
const hud = document.querySelector('#hud');
const surface = createLessonSurface(5);

async function start() {
  surface.progress('creating procedural records');
  const records = createProceduralRecords();
  let time = INITIAL_TIME;

  const gpu = await createGpuContext(canvas);
  const { device, context, format } = gpu;
  device.addEventListener('uncapturederror', (event) => {
    surface.fail(new Error(`Uncaptured WebGPU error: ${event.error.message}`));
  });
  device.lost.then((info) => {
    surface.fail(new Error(`WebGPU device lost (${info.reason}): ${info.message}`));
  });

  surface.progress('compiling lesson-owned compute and render stages');
  const [computeCode, renderCode] = await Promise.all([
    fetchText(new URL('./active-set.wgsl', import.meta.url)),
    fetchText(new URL('./active-set-render.wgsl', import.meta.url)),
  ]);
  const [computeShader, renderShader] = await Promise.all([
    compileShader(device, computeCode, 'lesson 05 compute shader'),
    compileShader(device, renderCode, 'lesson 05 render shader'),
  ]);
  const warningCount = computeShader.warningCount + renderShader.warningCount;

  const [resetPipeline, activePipeline, visiblePipeline, renderPipeline] = await withGpuErrorScopes(
    device,
    async () => Promise.all([
      device.createComputePipelineAsync({
        label: 'lesson 05 reset pipeline',
        layout: 'auto',
        compute: { module: computeShader.module, entryPoint: 'reset_main' },
      }),
      device.createComputePipelineAsync({
        label: 'lesson 05 active compaction pipeline',
        layout: 'auto',
        compute: { module: computeShader.module, entryPoint: 'active_main' },
      }),
      device.createComputePipelineAsync({
        label: 'lesson 05 visible compaction pipeline',
        layout: 'auto',
        compute: { module: computeShader.module, entryPoint: 'visible_main' },
      }),
      device.createRenderPipelineAsync({
        label: 'lesson 05 order-independent additive splat pipeline',
        layout: 'auto',
        vertex: { module: renderShader.module, entryPoint: 'vs_main' },
        fragment: {
          module: renderShader.module,
          entryPoint: 'fs_main',
          targets: [{
            format,
            blend: {
              color: { operation: 'add', srcFactor: 'one', dstFactor: 'one' },
              alpha: { operation: 'add', srcFactor: 'zero', dstFactor: 'one' },
            },
          }],
        },
        primitive: { topology: 'triangle-list' },
      }),
    ]),
  );

  const recordBuffer = createBufferWithData(
    device,
    'lesson 05 records',
    encodeRecords(records),
    GPUBufferUsage.STORAGE,
  );
  const counterBuffer = device.createBuffer({
    label: 'lesson 05 counters',
    size: COUNTER_BYTES,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });
  const activeBuffer = device.createBuffer({
    label: 'lesson 05 active indices',
    size: records.length * INDEX_BYTES,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });
  const visibleBuffer = device.createBuffer({
    label: 'lesson 05 visible indices',
    size: records.length * INDEX_BYTES,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });
  const drawArgsBuffer = device.createBuffer({
    label: 'lesson 05 indirect draw arguments',
    size: DRAW_ARGS_BYTES,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.INDIRECT | GPUBufferUsage.COPY_SRC,
  });
  const paramsBuffer = device.createBuffer({
    label: 'lesson 05 parameters',
    size: 16,
    usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
  });
  const readbackBytes = COUNTER_BYTES + DRAW_ARGS_BYTES + records.length * INDEX_BYTES * 2;
  const readbackBuffer = device.createBuffer({
    label: 'lesson 05 audit readback',
    size: readbackBytes,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });

  const bindings = {
    0: { buffer: recordBuffer },
    1: { buffer: counterBuffer },
    2: { buffer: activeBuffer },
    3: { buffer: visibleBuffer },
    4: { buffer: drawArgsBuffer },
    5: { buffer: paramsBuffer },
  };
  const resetGroup = bindGroup(device, resetPipeline, 'lesson 05 reset bindings', bindings, [0, 1, 4]);
  const activeGroup = bindGroup(device, activePipeline, 'lesson 05 active bindings', bindings, [0, 1, 2, 5]);
  const visibleGroup = bindGroup(device, visiblePipeline, 'lesson 05 visible bindings', bindings, [0, 1, 2, 3, 4]);
  const renderGroup = bindGroup(device, renderPipeline, 'lesson 05 render bindings', bindings, [0, 3, 5]);

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
        const expected = classifyRecords(records, time, ALPHA_MIN);
        device.queue.writeBuffer(paramsBuffer, 0, new Float32Array([time, ALPHA_MIN, 0, 0]));

        await withGpuErrorScopes(device, async () => {
          const encoder = device.createCommandEncoder({ label: 'lesson 05 frame encoder' });
          encodeComputePass(encoder, 'lesson 05 reset pass', resetPipeline, resetGroup, 1);
          encodeComputePass(
            encoder,
            'lesson 05 total to active pass',
            activePipeline,
            activeGroup,
            Math.ceil(records.length / WORKGROUP_SIZE),
          );
          encodeComputePass(
            encoder,
            'lesson 05 active to visible pass',
            visiblePipeline,
            visibleGroup,
            Math.ceil(records.length / WORKGROUP_SIZE),
          );

          const pass = encoder.beginRenderPass({
            label: 'lesson 05 order-independent display pass',
            colorAttachments: [{
              view: context.getCurrentTexture().createView(),
              clearValue: { r: 0.025, g: 0.025, b: 0.025, a: 1 },
              loadOp: 'clear',
              storeOp: 'store',
            }],
          });
          pass.setPipeline(renderPipeline);
          pass.setBindGroup(0, renderGroup);
          pass.drawIndirect(drawArgsBuffer, 0);
          pass.end();

          let offset = 0;
          encoder.copyBufferToBuffer(counterBuffer, 0, readbackBuffer, offset, COUNTER_BYTES);
          offset += COUNTER_BYTES;
          encoder.copyBufferToBuffer(drawArgsBuffer, 0, readbackBuffer, offset, DRAW_ARGS_BYTES);
          offset += DRAW_ARGS_BYTES;
          encoder.copyBufferToBuffer(activeBuffer, 0, readbackBuffer, offset, records.length * INDEX_BYTES);
          offset += records.length * INDEX_BYTES;
          encoder.copyBufferToBuffer(visibleBuffer, 0, readbackBuffer, offset, records.length * INDEX_BYTES);

          device.queue.submit([encoder.finish()]);
          await device.queue.onSubmittedWorkDone();
        });
        const audited = await readAudit(readbackBuffer, records.length);
        assertAudit(audited, expected);
        frameCount += 1;

        const details = {
          adapter: gpu.adapterSummary,
          format,
          width,
          height,
          warningCount,
          frameCount,
          time: round(time),
          totalCount: audited.total,
          activeCount: audited.active,
          visibleCount: audited.visible,
          indirectInstanceCount: audited.instanceCount,
          displayBlend: 'premultiplied-additive',
        };
        surface.pass(details, {
          shaderCompiled: true,
          computePipelinesCreated: true,
          renderPipelineCreated: true,
          gpuCountersMatchCpuReference: true,
          compactedIndexSetsMatchCpuReference: true,
          indirectInstanceCountMatchesVisibleCount: true,
          displayIsOrderIndependent: true,
          frameSubmitted: true,
          gpuWorkCompleted: true,
        });
        hud.textContent = [
          'LESSON 05 · PASS',
          `N ${audited.total}  →  A ${audited.active}  →  V ${audited.visible}`,
          `t ${time.toFixed(2)} · indirect ${audited.instanceCount} · additive preview`,
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
    time = clamp(time + (event.code === 'ArrowRight' ? 0.05 : -0.05), 0, 1);
    void draw().catch(surface.fail);
  });
  new ResizeObserver(() => void draw().catch(surface.fail)).observe(canvas);
  observeDevicePixelRatio(() => void draw().catch(surface.fail));
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
    values.set([...record.center, ...record.sigma], index * 12);
    values.set([record.timeCenter, record.timeSigma, record.opacity, 0], index * 12 + 4);
    values.set([...record.color, 0], index * 12 + 8);
  });
  return values;
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

function bindGroup(device, pipeline, label, bindings, selected) {
  return device.createBindGroup({
    label,
    layout: pipeline.getBindGroupLayout(0),
    entries: selected.map((binding) => ({ binding, resource: bindings[binding] })),
  });
}

function encodeComputePass(encoder, label, pipeline, group, workgroups) {
  const pass = encoder.beginComputePass({ label });
  pass.setPipeline(pipeline);
  pass.setBindGroup(0, group);
  pass.dispatchWorkgroups(workgroups);
  pass.end();
}

async function readAudit(buffer, recordCount) {
  await buffer.mapAsync(GPUMapMode.READ);
  const copy = buffer.getMappedRange().slice(0);
  buffer.unmap();
  const words = new Uint32Array(copy);
  const total = words[0];
  const active = words[1];
  const visible = words[2];
  const instanceCount = words[5];
  const activeStart = (COUNTER_BYTES + DRAW_ARGS_BYTES) / INDEX_BYTES;
  const visibleStart = activeStart + recordCount;
  return {
    total,
    active,
    visible,
    instanceCount,
    activeIndices: [...words.slice(activeStart, activeStart + active)],
    visibleIndices: [...words.slice(visibleStart, visibleStart + visible)],
  };
}

function assertAudit(actual, expected) {
  assertEqual(actual.total, expected.total, 'total counter');
  assertEqual(actual.active, expected.activeIndices.length, 'active counter');
  assertEqual(actual.visible, expected.visibleIndices.length, 'visible counter');
  assertEqual(actual.instanceCount, actual.visible, 'indirect instance count');
  assertArrayEqual(sorted(actual.activeIndices), expected.activeIndices, 'active compacted set');
  assertArrayEqual(sorted(actual.visibleIndices), expected.visibleIndices, 'visible compacted set');
}

function assertEqual(actual, expected, label) {
  if (actual !== expected) throw new Error(`${label}: GPU ${actual}, CPU reference ${expected}`);
}

function assertArrayEqual(actual, expected, label) {
  if (actual.length !== expected.length || actual.some((value, index) => value !== expected[index])) {
    throw new Error(`${label}: GPU [${actual}], CPU reference [${expected}]`);
  }
}

function sorted(values) {
  return [...values].sort((left, right) => left - right);
}

function clamp(value, minimum, maximum) {
  return Math.max(minimum, Math.min(maximum, value));
}

function round(value) {
  return Math.round(value * 1000) / 1000;
}

function observeDevicePixelRatio(onChange) {
  let observed = Math.max(0.01, globalThis.devicePixelRatio || 1);
  function sample() {
    const current = Math.max(0.01, globalThis.devicePixelRatio || 1);
    if (current !== observed) {
      observed = current;
      onChange();
    }
    requestAnimationFrame(sample);
  }
  requestAnimationFrame(sample);
}
