import { createGpuContext, withGpuErrorScopes } from '../infra/gpu.js';
import { createLessonSurface } from '../infra/page.js';
import {
  PRIMITIVE_BYTES,
  REFERENCE_TIMES,
  checkCpuInvariants,
  evaluateScene,
  packPrimitives,
} from './reference.js';

const FLOATS_PER_EVALUATION = 4;

// Records are already near-to-far. Static and moving behavior is selected by
// the explicit `moving` field; every record still has the same 64-byte schema.
const primitives = [
  { mean: [-0.58, 0.34], velocity: [0, 0], color: [0.34, 0.50, 0.92], timeCenter: 0.5, duration: 1, opacity: 0.58, moving: 0, scale: [0.22, 0.18], depth: 0.12 },
  { mean: [0.48, 0.37], velocity: [0, 0], color: [0.52, 0.38, 0.88], timeCenter: 0.5, duration: 1, opacity: 0.54, moving: 0, scale: [0.24, 0.17], depth: 0.18 },
  { mean: [-0.18, -0.36], velocity: [0, 0], color: [0.22, 0.65, 0.72], timeCenter: 0.5, duration: 1, opacity: 0.50, moving: 0, scale: [0.26, 0.17], depth: 0.24 },
  { mean: [0.46, -0.34], velocity: [0, 0], color: [0.26, 0.57, 0.84], timeCenter: 0.5, duration: 1, opacity: 0.56, moving: 0, scale: [0.21, 0.20], depth: 0.30 },
  { mean: [-0.28, 0.02], velocity: [1.18, 0.24], color: [1.00, 0.34, 0.10], timeCenter: 0.5, duration: 0.27, opacity: 0.91, moving: 1, scale: [0.20, 0.16], depth: 0.38 },
  { mean: [0.31, -0.04], velocity: [-1.08, 0.30], color: [0.08, 0.90, 0.62], timeCenter: 0.5, duration: 0.24, opacity: 0.88, moving: 1, scale: [0.18, 0.19], depth: 0.44 },
  { mean: [0.02, 0.24], velocity: [0.14, -0.94], color: [0.98, 0.22, 0.56], timeCenter: 0.5, duration: 0.30, opacity: 0.86, moving: 1, scale: [0.17, 0.20], depth: 0.50 },
];

const staticCount = primitives.filter((primitive) => primitive.moving < 0.5).length;
const movingCount = primitives.length - staticCount;
const canvas = document.querySelector('canvas');
const playButton = document.querySelector('#play');
const timeSlider = document.querySelector('#time');
const timeOutput = document.querySelector('#time-value');
const surface = createLessonSurface(4);

async function start() {
  surface.progress('requesting adapter');
  const gpu = await createGpuContext(canvas);
  const { device, context, format } = gpu;

  device.addEventListener('uncapturederror', (event) => {
    surface.fail(new Error(`Uncaptured WebGPU error: ${event.error.message}`));
  });
  device.lost.then((info) => {
    surface.fail(new Error(`WebGPU device lost (${info.reason}): ${info.message}`));
  });

  surface.progress('compiling explicit-time shader');
  const shaderUrl = new URL('./explicit-time.wgsl', import.meta.url);
  const module = await loadShaderModule(device, shaderUrl);
  const resources = await createResources(device, module, format);
  device.queue.writeBuffer(resources.primitiveBuffer, 0, packPrimitives(primitives));

  surface.progress('comparing CPU and WGSL evaluation');
  const gpuValidation = await validateGpuEvaluation(device, resources);
  const assertions = {
    webGpuAvailable: true,
    shaderCompiled: true,
    ...checkCpuInvariants(primitives),
    cpuAndWgslEvaluationsMatch: gpuValidation.maxError < 1e-4,
    staticAndMovingRecordsPresent: staticCount > 0 && movingCount > 0,
  };
  if (!Object.values(assertions).every(Boolean)) {
    throw new Error(`Lesson invariants failed: ${JSON.stringify(assertions)}`);
  }

  let time = Number(timeSlider.value);
  let playing = true;
  let lastTick;
  let animationFrame;
  let drawPromise = null;
  let drawAgain = false;

  function updateControls() {
    timeSlider.value = time.toFixed(3);
    timeOutput.value = time.toFixed(3);
    playButton.textContent = playing ? 'pause' : 'play';
  }

  function setPlaying(nextPlaying) {
    playing = nextPlaying;
    lastTick = undefined;
    if (animationFrame !== undefined) {
      cancelAnimationFrame(animationFrame);
      animationFrame = undefined;
    }
    updateControls();
    if (playing) animationFrame = requestAnimationFrame(tick);
  }

  function requestDraw() {
    drawAgain = true;
    if (!drawPromise) {
      drawPromise = (async () => {
        while (drawAgain) {
          drawAgain = false;
          const { width, height } = await gpu.configureCanvas();
          device.queue.writeBuffer(resources.timeBuffer, 0, new Float32Array([time, 0, 0, 0]));
          await withGpuErrorScopes(device, async () => {
            const encoder = device.createCommandEncoder({ label: 'lesson 04 frame encoder' });
            const pass = encoder.beginRenderPass({
              label: 'lesson 04 render pass',
              colorAttachments: [{
                view: context.getCurrentTexture().createView(),
                clearValue: { r: 0, g: 0, b: 0, a: 0 },
                loadOp: 'clear',
                storeOp: 'store',
              }],
            });
            pass.setPipeline(resources.renderPipeline);
            pass.setBindGroup(0, resources.renderBindGroup);
            pass.draw(6, primitives.length);
            pass.setPipeline(resources.backgroundPipeline);
            pass.draw(3);
            pass.end();
            device.queue.submit([encoder.finish()]);
            await device.queue.onSubmittedWorkDone();
          });
          surface.pass(
            {
              adapter: gpu.adapterSummary,
              format,
              width,
              height,
              time,
              playing,
              staticCount,
              movingCount,
              recordBytes: PRIMITIVE_BYTES,
              maxCpuWgslError: gpuValidation.maxError,
            },
            assertions,
          );
        }
      })().finally(() => {
        drawPromise = null;
        if (drawAgain) void requestDraw().catch(surface.fail);
      });
    }
    return drawPromise;
  }

  function tick(timestamp) {
    animationFrame = undefined;
    if (!playing) return;
    if (lastTick !== undefined) time = (time + (timestamp - lastTick) / 5000) % 1;
    lastTick = timestamp;
    updateControls();
    void requestDraw().catch(surface.fail);
    animationFrame = requestAnimationFrame(tick);
  }

  playButton.addEventListener('click', () => setPlaying(!playing));
  timeSlider.addEventListener('input', () => {
    setPlaying(false);
    time = Number(timeSlider.value);
    updateControls();
    void requestDraw().catch(surface.fail);
  });
  addEventListener('keydown', (event) => {
    if (event.code === 'Space') {
      event.preventDefault();
      setPlaying(!playing);
    }
    if (event.code === 'ArrowLeft' || event.code === 'ArrowRight') {
      event.preventDefault();
      setPlaying(false);
      time = Math.min(1, Math.max(0, time + (event.code === 'ArrowLeft' ? -0.02 : 0.02)));
      updateControls();
      void requestDraw().catch(surface.fail);
    }
  });
  new ResizeObserver(() => void requestDraw().catch(surface.fail)).observe(canvas);

  updateControls();
  await requestDraw();
  setPlaying(true);
}

start().catch(surface.fail);

async function loadShaderModule(device, shaderUrl) {
  const response = await fetch(shaderUrl);
  if (!response.ok) throw new Error(`Cannot load ${shaderUrl.pathname}: HTTP ${response.status}`);
  const module = device.createShaderModule({ label: 'lesson 04 shader', code: await response.text() });
  const info = await module.getCompilationInfo();
  const errors = info.messages.filter((message) => message.type === 'error');
  if (errors.length > 0) {
    throw new Error(errors.map((message) => `${message.lineNum}:${message.linePos} ${message.message}`).join('\n'));
  }
  return module;
}

async function createResources(device, module, canvasFormat) {
  return withGpuErrorScopes(device, async () => {
    const primitiveBuffer = device.createBuffer({
      label: 'lesson 04 primitive records',
      size: primitives.length * PRIMITIVE_BYTES,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
    });
    const timeBuffer = device.createBuffer({
      label: 'lesson 04 current time',
      // TimeState is 32 bytes: vec3<f32> starts at a 16-byte aligned offset.
      size: 32,
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    });
    const validationTimesBuffer = device.createBuffer({
      label: 'lesson 04 validation times',
      size: REFERENCE_TIMES.length * Float32Array.BYTES_PER_ELEMENT,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
    });
    const validationBytes = primitives.length * REFERENCE_TIMES.length
      * FLOATS_PER_EVALUATION * Float32Array.BYTES_PER_ELEMENT;
    const validationOutputBuffer = device.createBuffer({
      label: 'lesson 04 validation output',
      size: validationBytes,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
    });

    const renderBindGroupLayout = device.createBindGroupLayout({
      label: 'lesson 04 render bind group layout',
      entries: [
        { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: { type: 'read-only-storage' } },
        { binding: 1, visibility: GPUShaderStage.VERTEX, buffer: { type: 'uniform' } },
      ],
    });
    const renderBindGroup = device.createBindGroup({
      label: 'lesson 04 render bind group',
      layout: renderBindGroupLayout,
      entries: [
        { binding: 0, resource: { buffer: primitiveBuffer } },
        { binding: 1, resource: { buffer: timeBuffer } },
      ],
    });
    const renderPipeline = await device.createRenderPipelineAsync({
      label: 'lesson 04 primitive pipeline',
      layout: device.createPipelineLayout({ bindGroupLayouts: [renderBindGroupLayout] }),
      vertex: { module, entryPoint: 'vs_primitive' },
      fragment: {
        module,
        entryPoint: 'fs_primitive',
        targets: [{
          format: canvasFormat,
          blend: {
            color: { operation: 'add', srcFactor: 'one-minus-dst-alpha', dstFactor: 'one' },
            alpha: { operation: 'add', srcFactor: 'one-minus-dst-alpha', dstFactor: 'one' },
          },
        }],
      },
      primitive: { topology: 'triangle-list' },
    });
    const backgroundPipeline = await device.createRenderPipelineAsync({
      label: 'lesson 04 background pipeline',
      layout: 'auto',
      vertex: { module, entryPoint: 'vs_background' },
      fragment: {
        module,
        entryPoint: 'fs_background',
        targets: [{
          format: canvasFormat,
          blend: {
            color: { operation: 'add', srcFactor: 'one-minus-dst-alpha', dstFactor: 'one' },
            alpha: { operation: 'add', srcFactor: 'one-minus-dst-alpha', dstFactor: 'one' },
          },
        }],
      },
      primitive: { topology: 'triangle-list' },
    });

    const computeBindGroupLayout = device.createBindGroupLayout({
      label: 'lesson 04 validation bind group layout',
      entries: [
        { binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'read-only-storage' } },
        { binding: 2, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'read-only-storage' } },
        { binding: 3, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'storage' } },
      ],
    });
    const computeBindGroup = device.createBindGroup({
      label: 'lesson 04 validation bind group',
      layout: computeBindGroupLayout,
      entries: [
        { binding: 0, resource: { buffer: primitiveBuffer } },
        { binding: 2, resource: { buffer: validationTimesBuffer } },
        { binding: 3, resource: { buffer: validationOutputBuffer } },
      ],
    });
    const computePipeline = await device.createComputePipelineAsync({
      label: 'lesson 04 validation compute pipeline',
      layout: device.createPipelineLayout({ bindGroupLayouts: [computeBindGroupLayout] }),
      compute: { module, entryPoint: 'evaluate_for_validation' },
    });

    device.queue.writeBuffer(validationTimesBuffer, 0, new Float32Array(REFERENCE_TIMES));
    return {
      primitiveBuffer,
      timeBuffer,
      renderPipeline,
      backgroundPipeline,
      renderBindGroup,
      validationOutputBuffer,
      validationBytes,
      computePipeline,
      computeBindGroup,
    };
  });
}

async function validateGpuEvaluation(device, resources) {
  const readback = device.createBuffer({
    label: 'lesson 04 validation readback',
    size: resources.validationBytes,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });
  await withGpuErrorScopes(device, async () => {
    const encoder = device.createCommandEncoder({ label: 'lesson 04 validation encoder' });
    const pass = encoder.beginComputePass({ label: 'lesson 04 validation pass' });
    pass.setPipeline(resources.computePipeline);
    pass.setBindGroup(0, resources.computeBindGroup);
    const evaluationCount = primitives.length * REFERENCE_TIMES.length;
    pass.dispatchWorkgroups(Math.ceil(evaluationCount / 64));
    pass.end();
    encoder.copyBufferToBuffer(
      resources.validationOutputBuffer,
      0,
      readback,
      0,
      resources.validationBytes,
    );
    device.queue.submit([encoder.finish()]);
    await device.queue.onSubmittedWorkDone();
  });
  await readback.mapAsync(GPUMapMode.READ);
  const actual = new Float32Array(readback.getMappedRange());
  const expected = evaluateScene(primitives).flatMap((evaluation) => [
    ...evaluation.mean,
    evaluation.gate,
    evaluation.opacity,
  ]);
  let maxError = 0;
  for (let index = 0; index < expected.length; index += 1) {
    maxError = Math.max(maxError, Math.abs(actual[index] - expected[index]));
  }
  readback.unmap();
  readback.destroy();
  return { maxError };
}
