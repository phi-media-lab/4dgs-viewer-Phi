import { createGpuContext, withGpuErrorScopes } from '../infra/gpu.js';
import { createLessonSurface } from '../infra/page.js';
import {
  DEFAULT_BACKGROUND,
  colorDistance,
  compositeFrontToBack,
  sortFrontToBack,
} from './reference.js';

const RECORD_FLOATS = 12;
const RECORD_BYTES = RECORD_FLOATS * Float32Array.BYTES_PER_ELEMENT;
const VALIDATION_SIZE = 64;

const records = [
  { mean: [-0.16, 0.02], sigma: [0.34, 0.30], color: [0.96, 0.18, 0.12], depth: 0.15, opacity: 0.82 },
  { mean: [0.13, 0.08], sigma: [0.32, 0.35], color: [0.12, 0.82, 0.34], depth: 0.46, opacity: 0.78 },
  { mean: [0.00, -0.15], sigma: [0.37, 0.31], color: [0.16, 0.38, 0.98], depth: 0.78, opacity: 0.86 },
];

const correctOrder = sortFrontToBack(records);
const wrongOrder = [...correctOrder].reverse();

const canvas = document.querySelector('canvas');
const toggle = document.querySelector('#toggle');
const modeOutput = document.querySelector('#mode');
const surface = createLessonSurface(3);

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

  surface.progress('compiling order and blend shader');
  const shaderUrl = new URL('./order-blend.wgsl', import.meta.url);
  const module = await loadShaderModule(device, shaderUrl);
  const resources = await createResources(device, module, format);
  const {
    recordBuffer,
    orderBuffer,
    bindGroup,
    canvasPipeline,
    canvasBackgroundPipeline,
    validationPipeline,
    validationBackgroundPipeline,
  } = resources;

  device.queue.writeBuffer(recordBuffer, 0, packRecords(records));

  const validation = await validateGpuCenterPixel(device, {
    orderBuffer,
    bindGroup,
    gaussianPipeline: validationPipeline,
    backgroundPipeline: validationBackgroundPipeline,
  });

  const correctCenter = compositeFrontToBack(records, correctOrder, validation.sampleNdc);
  const wrongCenter = compositeFrontToBack(records, wrongOrder, validation.sampleNdc);
  const assertions = {
    webGpuAvailable: true,
    shaderCompiled: true,
    recordStrideIs48Bytes: RECORD_BYTES === 48,
    depthOrderIsNearToFar: correctOrder.every(
      (index, position) => position === 0 || records[correctOrder[position - 1]].depth <= records[index].depth,
    ),
    reversedOrderChangesColor: colorDistance(correctCenter.color, wrongCenter.color) > 0.2,
    gpuCenterMatchesCpuFrontToBack: validation.distance <= (2.5 / 255),
  };
  if (!Object.values(assertions).every(Boolean)) {
    throw new Error(`Lesson invariants failed: ${JSON.stringify(assertions)}`);
  }

  let correct = true;
  let drawPromise = null;
  let drawAgain = false;

  function setMode(nextCorrect) {
    correct = nextCorrect;
    const order = correct ? correctOrder : wrongOrder;
    device.queue.writeBuffer(orderBuffer, 0, Uint32Array.from([...order, 0]));
    toggle.textContent = correct ? 'show wrong order' : 'show correct order';
    modeOutput.textContent = correct ? 'NEAR → FAR · CORRECT' : 'FAR → NEAR · WRONG';
    void requestDraw().catch(surface.fail);
  }

  function requestDraw() {
    drawAgain = true;
    if (!drawPromise) {
      drawPromise = (async () => {
        while (drawAgain) {
          drawAgain = false;
          const { width, height } = await gpu.configureCanvas();
          await withGpuErrorScopes(device, async () => {
            const encoder = device.createCommandEncoder({ label: 'lesson 03 frame encoder' });
            encodeScene(
              encoder,
              context.getCurrentTexture().createView(),
              bindGroup,
              canvasPipeline,
              canvasBackgroundPipeline,
            );
            device.queue.submit([encoder.finish()]);
            await device.queue.onSubmittedWorkDone();
          });
          surface.pass(
            {
              adapter: gpu.adapterSummary,
              format,
              width,
              height,
              mode: correct ? 'front-to-back' : 'reversed',
              order: (correct ? correctOrder : wrongOrder).map((index) => records[index].depth),
              gpuCpuCenterError: validation.distance,
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

  toggle.addEventListener('click', () => setMode(!correct));
  addEventListener('keydown', (event) => {
    if (event.code === 'Space') {
      event.preventDefault();
      setMode(!correct);
    }
  });
  new ResizeObserver(() => void requestDraw().catch(surface.fail)).observe(canvas);

  setMode(true);
  await drawPromise;
}

start().catch(surface.fail);

async function loadShaderModule(device, shaderUrl) {
  const response = await fetch(shaderUrl);
  if (!response.ok) throw new Error(`Cannot load ${shaderUrl.pathname}: HTTP ${response.status}`);
  const module = device.createShaderModule({ label: 'lesson 03 shader', code: await response.text() });
  const info = await module.getCompilationInfo();
  const errors = info.messages.filter((message) => message.type === 'error');
  if (errors.length > 0) {
    throw new Error(errors.map((message) => `${message.lineNum}:${message.linePos} ${message.message}`).join('\n'));
  }
  return module;
}

async function createResources(device, module, canvasFormat) {
  return withGpuErrorScopes(device, async () => {
    const recordBuffer = device.createBuffer({
      label: 'lesson 03 Gaussian records',
      size: records.length * RECORD_BYTES,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
    });
    const orderBuffer = device.createBuffer({
      label: 'lesson 03 order',
      size: 4 * Uint32Array.BYTES_PER_ELEMENT,
      usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
    });
    const bindGroupLayout = device.createBindGroupLayout({
      label: 'lesson 03 bind group layout',
      entries: [
        { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: { type: 'read-only-storage' } },
        { binding: 1, visibility: GPUShaderStage.VERTEX, buffer: { type: 'read-only-storage' } },
      ],
    });
    const pipelineLayout = device.createPipelineLayout({
      label: 'lesson 03 pipeline layout',
      bindGroupLayouts: [bindGroupLayout],
    });
    const bindGroup = device.createBindGroup({
      label: 'lesson 03 bind group',
      layout: bindGroupLayout,
      entries: [
        { binding: 0, resource: { buffer: recordBuffer } },
        { binding: 1, resource: { buffer: orderBuffer } },
      ],
    });

    const createGaussianPipeline = (targetFormat, label) => device.createRenderPipelineAsync({
      label,
      layout: pipelineLayout,
      vertex: { module, entryPoint: 'vs_gaussian' },
      fragment: {
        module,
        entryPoint: 'fs_gaussian',
        targets: [{
          format: targetFormat,
          blend: {
            color: { operation: 'add', srcFactor: 'one-minus-dst-alpha', dstFactor: 'one' },
            alpha: { operation: 'add', srcFactor: 'one-minus-dst-alpha', dstFactor: 'one' },
          },
        }],
      },
      primitive: { topology: 'triangle-list' },
    });
    const createBackgroundPipeline = (targetFormat, label) => device.createRenderPipelineAsync({
      label,
      layout: 'auto',
      vertex: { module, entryPoint: 'vs_background' },
      fragment: {
        module,
        entryPoint: 'fs_background',
        targets: [{
          format: targetFormat,
          blend: {
            color: { operation: 'add', srcFactor: 'one-minus-dst-alpha', dstFactor: 'one' },
            alpha: { operation: 'add', srcFactor: 'one-minus-dst-alpha', dstFactor: 'one' },
          },
        }],
      },
      primitive: { topology: 'triangle-list' },
    });

    const [canvasPipeline, canvasBackgroundPipeline, validationPipeline, validationBackgroundPipeline] = await Promise.all([
      createGaussianPipeline(canvasFormat, 'lesson 03 canvas Gaussian pipeline'),
      createBackgroundPipeline(canvasFormat, 'lesson 03 canvas background pipeline'),
      createGaussianPipeline('rgba8unorm', 'lesson 03 validation Gaussian pipeline'),
      createBackgroundPipeline('rgba8unorm', 'lesson 03 validation background pipeline'),
    ]);
    return {
      recordBuffer,
      orderBuffer,
      bindGroup,
      canvasPipeline,
      canvasBackgroundPipeline,
      validationPipeline,
      validationBackgroundPipeline,
    };
  });
}

function packRecords(source) {
  const packed = new Float32Array(source.length * RECORD_FLOATS);
  source.forEach((record, index) => {
    packed.set([
      ...record.mean,
      ...record.sigma,
      ...record.color, 1,
      record.depth,
      record.opacity,
      0,
      0,
    ], index * RECORD_FLOATS);
  });
  return packed;
}

function encodeScene(encoder, view, bindGroup, gaussianPipeline, backgroundPipeline) {
  const pass = encoder.beginRenderPass({
    label: 'lesson 03 render pass',
    colorAttachments: [{
      view,
      clearValue: { r: 0, g: 0, b: 0, a: 0 },
      loadOp: 'clear',
      storeOp: 'store',
    }],
  });
  pass.setPipeline(gaussianPipeline);
  pass.setBindGroup(0, bindGroup);
  pass.draw(6, records.length);
  pass.setPipeline(backgroundPipeline);
  pass.draw(3);
  pass.end();
}

async function validateGpuCenterPixel(device, pipelines) {
  device.queue.writeBuffer(pipelines.orderBuffer, 0, Uint32Array.from([...correctOrder, 0]));
  const texture = device.createTexture({
    label: 'lesson 03 validation target',
    size: [VALIDATION_SIZE, VALIDATION_SIZE],
    format: 'rgba8unorm',
    usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC,
  });
  const readback = device.createBuffer({
    label: 'lesson 03 validation readback',
    size: 256,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });
  const sampleX = VALIDATION_SIZE / 2;
  const sampleY = VALIDATION_SIZE / 2;
  const sampleNdc = [
    ((sampleX + 0.5) / VALIDATION_SIZE) * 2 - 1,
    1 - ((sampleY + 0.5) / VALIDATION_SIZE) * 2,
  ];
  const expected = compositeFrontToBack(records, correctOrder, sampleNdc, DEFAULT_BACKGROUND).color;

  await withGpuErrorScopes(device, async () => {
    const encoder = device.createCommandEncoder({ label: 'lesson 03 validation encoder' });
    encodeScene(
      encoder,
      texture.createView(),
      pipelines.bindGroup,
      pipelines.gaussianPipeline,
      pipelines.backgroundPipeline,
    );
    encoder.copyTextureToBuffer(
      { texture, origin: [sampleX, sampleY, 0] },
      { buffer: readback, bytesPerRow: 256, rowsPerImage: 1 },
      [1, 1, 1],
    );
    device.queue.submit([encoder.finish()]);
    await device.queue.onSubmittedWorkDone();
  });
  await readback.mapAsync(GPUMapMode.READ);
  const bytes = new Uint8Array(readback.getMappedRange());
  const actual = [bytes[0], bytes[1], bytes[2]].map((value) => value / 255);
  const distance = colorDistance(actual, expected);
  readback.unmap();
  readback.destroy();
  texture.destroy();
  return { actual, expected, distance, sampleNdc };
}
