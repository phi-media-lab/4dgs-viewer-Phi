import {
  createGpuContext,
  withGpuErrorScopes,
} from '../infra/gpu.js';
import { createLessonSurface } from '../infra/page.js';
import {
  createProjectionScene,
  projectGaussian,
  projectedValues,
} from './reference.js';

const CASE_COUNT = 4;
const CASE_STRIDE_FLOATS = 20;
const OUTPUT_STRIDE_FLOATS = 8;
const AGREEMENT_ABSOLUTE_TOLERANCE = 0.0005;
const AGREEMENT_RELATIVE_TOLERANCE = 2e-6;

const canvas = document.querySelector('canvas');
const hud = document.querySelector('#hud');
const surface = createLessonSurface(2);
let focalScale = 0.72;

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

  surface.progress('compiling projection equations');
  const shaderUrl = new URL('./projection.wgsl', import.meta.url);
  const shaderCode = await loadText(shaderUrl);
  const { module, warningCount } = await withGpuErrorScopes(device, async () => {
    const module = device.createShaderModule({
      label: 'lesson 02 projection shader',
      code: shaderCode,
    });
    const compilation = await module.getCompilationInfo();
    const errors = compilation.messages.filter((message) => message.type === 'error');
    if (errors.length > 0) {
      throw new Error(`lesson 02 shader did not compile:\n${errors.map(formatMessage).join('\n')}`);
    }
    return {
      module,
      warningCount: compilation.messages.filter((message) => message.type === 'warning').length,
    };
  });

  surface.progress('creating projection pipelines');
  const renderPipeline = await withGpuErrorScopes(
    device,
    async () => device.createRenderPipelineAsync({
      label: 'lesson 02 projected splat pipeline',
      layout: 'auto',
      vertex: { module, entryPoint: 'vs_main' },
      fragment: {
        module,
        entryPoint: 'fs_main',
        targets: [{
          format,
          blend: {
            color: { srcFactor: 'one', dstFactor: 'one-minus-src-alpha' },
            alpha: { srcFactor: 'one', dstFactor: 'one-minus-src-alpha' },
          },
        }],
      },
      primitive: { topology: 'triangle-list' },
    }),
  );
  const verificationPipeline = await withGpuErrorScopes(
    device,
    async () => device.createComputePipelineAsync({
      label: 'lesson 02 projection verification pipeline',
      layout: 'auto',
      compute: { module, entryPoint: 'verify_projection' },
    }),
  );

  const cameraBuffer = device.createBuffer({
    label: 'lesson 02 camera uniform',
    size: 32,
    usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
  });
  const gaussianBuffer = device.createBuffer({
    label: 'lesson 02 procedural Gaussian records',
    size: CASE_COUNT * CASE_STRIDE_FLOATS * Float32Array.BYTES_PER_ELEMENT,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
  });
  const outputByteLength = CASE_COUNT * OUTPUT_STRIDE_FLOATS * Float32Array.BYTES_PER_ELEMENT;
  const verificationBuffer = device.createBuffer({
    label: 'lesson 02 WGSL projection values',
    size: outputByteLength,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });
  const readbackBuffer = device.createBuffer({
    label: 'lesson 02 projection readback',
    size: outputByteLength,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });
  const renderBindGroup = device.createBindGroup({
    label: 'lesson 02 render bind group',
    layout: renderPipeline.getBindGroupLayout(0),
    entries: [
      { binding: 0, resource: { buffer: cameraBuffer } },
      { binding: 1, resource: { buffer: gaussianBuffer } },
    ],
  });
  const verificationBindGroup = device.createBindGroup({
    label: 'lesson 02 verification bind group',
    layout: verificationPipeline.getBindGroupLayout(0),
    entries: [
      { binding: 0, resource: { buffer: cameraBuffer } },
      { binding: 1, resource: { buffer: gaussianBuffer } },
      { binding: 2, resource: { buffer: verificationBuffer } },
    ],
  });

  let frameCount = 0;
  let drawingPromise = null;
  let drawRequested = false;

  async function requestDraw() {
    drawRequested = true;
    if (!drawingPromise) {
      drawingPromise = (async () => {
        while (drawRequested) {
          drawRequested = false;
          const { width, height } = await gpu.configureCanvas();
          const scene = createProjectionScene(width, height, focalScale);
          const cameraData = packCamera(width, height, scene.camera);
          const cameraFloats = new Float32Array(
            cameraData.buffer,
            cameraData.byteOffset,
            7,
          );
          const gaussianData = packCases(scene.cases);
          device.queue.writeBuffer(cameraBuffer, 0, cameraData);
          device.queue.writeBuffer(gaussianBuffer, 0, gaussianData);

          await withGpuErrorScopes(device, async () => {
            const encoder = device.createCommandEncoder({ label: 'lesson 02 frame encoder' });
            const computePass = encoder.beginComputePass({ label: 'lesson 02 CPU/WGSL check' });
            computePass.setPipeline(verificationPipeline);
            computePass.setBindGroup(0, verificationBindGroup);
            computePass.dispatchWorkgroups(1);
            computePass.end();

            const renderPass = encoder.beginRenderPass({
              label: 'lesson 02 render pass',
              colorAttachments: [{
                view: context.getCurrentTexture().createView(),
                clearValue: { r: 0.025, g: 0.025, b: 0.028, a: 1 },
                loadOp: 'clear',
                storeOp: 'store',
              }],
            });
            renderPass.setPipeline(renderPipeline);
            renderPass.setBindGroup(0, renderBindGroup);
            renderPass.draw(6, CASE_COUNT);
            renderPass.end();
            encoder.copyBufferToBuffer(
              verificationBuffer,
              0,
              readbackBuffer,
              0,
              outputByteLength,
            );
            device.queue.submit([encoder.finish()]);
            await device.queue.onSubmittedWorkDone();
          });

          await readbackBuffer.mapAsync(GPUMapMode.READ);
          const gpuValues = new Float32Array(readbackBuffer.getMappedRange()).slice();
          readbackBuffer.unmap();
          const comparison = compareProjectionResults(scene, cameraData, gaussianData, gpuValues);
          if (!Number.isFinite(comparison.maxToleranceRatio)
              || comparison.maxToleranceRatio > 1) {
            throw new Error(
              `CPU/WGSL projection mismatch: worst error is `
              + `${comparison.maxToleranceRatio.toFixed(3)}× the abs+relative tolerance`,
            );
          }

          const nearTrace = trace2D(comparison.cases[0].projectedCovariance);
          const farTrace = trace2D(comparison.cases[1].projectedCovariance);
          const rotationCrossTerm = Math.abs(comparison.cases[2].projectedCovariance[1]);
          if (!(nearTrace > farTrace) || !(rotationCrossTerm > 1)) {
            throw new Error('Projection cases do not expose the expected depth and rotation effects.');
          }

          frameCount += 1;
          surface.pass(
            {
              adapter: gpu.adapterSummary,
              format,
              width,
              height,
              warningCount,
              frameCount,
              focalPixels: cameraFloats[2],
              agreementAbsoluteTolerance: AGREEMENT_ABSOLUTE_TOLERANCE,
              agreementRelativeTolerance: AGREEMENT_RELATIVE_TOLERANCE,
              maxAbsoluteError: comparison.maxAbsoluteError,
              maxRelativeError: comparison.maxRelativeError,
              maxToleranceRatio: comparison.maxToleranceRatio,
              cases: comparison.cases,
            },
            {
              webGpuAvailable: true,
              shaderCompiled: true,
              renderPipelineCreated: true,
              verificationPipelineCreated: true,
              fourProjectionCasesRendered: true,
              nearFootprintLargerThanFar: true,
              rotatedCovarianceHasCrossTerm: true,
              cpuGpuAgreement: true,
              frameSubmitted: true,
              gpuWorkCompleted: true,
            },
          );
          hud.textContent += [
            `\nf ${cameraFloats[2].toFixed(1)} px · CPU/WGSL Δ ${comparison.maxAbsoluteError.toExponential(2)}`,
            ...comparison.cases.map((item) => (
              `${item.label.padEnd(23)} z ${item.center[2].toFixed(1)} · `
              + `σ ${Math.sqrt(item.projectedCovariance[0]).toFixed(1)}, `
              + `${Math.sqrt(item.projectedCovariance[2]).toFixed(1)} px`
            )),
            '[ / ] focal length · R reset · H diagnostics',
          ].join('\n');
        }
      })().finally(() => {
        drawingPromise = null;
        if (drawRequested) void requestDraw().catch(surface.fail);
      });
    }
    return drawingPromise;
  }

  addEventListener('keydown', (event) => {
    if (event.code === 'BracketLeft') focalScale = Math.max(0.4, focalScale - 0.04);
    else if (event.code === 'BracketRight') focalScale = Math.min(1.2, focalScale + 0.04);
    else if (event.code === 'KeyR') focalScale = 0.72;
    else return;
    event.preventDefault();
    void requestDraw().catch(surface.fail);
  });
  await requestDraw();
  new ResizeObserver(() => void requestDraw().catch(surface.fail)).observe(canvas);
  observeDevicePixelRatio(() => void requestDraw().catch(surface.fail));
}

start().catch(surface.fail);

function packCamera(width, height, camera) {
  const buffer = new ArrayBuffer(32);
  const view = new DataView(buffer);
  const values = [
    width, height,
    camera.focal[0], camera.focal[1],
    camera.principal[0], camera.principal[1],
    camera.minimumVariance,
  ];
  values.forEach((value, index) => view.setFloat32(index * 4, value, true));
  view.setUint32(28, CASE_COUNT, true);
  return new Uint8Array(buffer);
}

function packCases(cases) {
  const values = new Float32Array(CASE_COUNT * CASE_STRIDE_FLOATS);
  cases.forEach((item, index) => {
    const offset = index * CASE_STRIDE_FLOATS;
    values.set([...item.center, item.opacity], offset);
    values.set([...item.covariance.slice(0, 3), 0], offset + 4);
    values.set([...item.covariance.slice(3, 6), 0], offset + 8);
    values.set([...item.covariance.slice(6, 9), 0], offset + 12);
    values.set([...item.color, 0], offset + 16);
  });
  return values;
}

function compareProjectionResults(scene, packedCameraBytes, packedCases, gpuValues) {
  const cameraFloats = new Float32Array(
    packedCameraBytes.buffer,
    packedCameraBytes.byteOffset,
    7,
  );
  const camera = {
    focal: [cameraFloats[2], cameraFloats[3]],
    principal: [cameraFloats[4], cameraFloats[5]],
    minimumVariance: cameraFloats[6],
  };
  let maxAbsoluteError = 0;
  let maxRelativeError = 0;
  let maxToleranceRatio = 0;
  const cases = scene.cases.map((item, index) => {
    const inputOffset = index * CASE_STRIDE_FLOATS;
    const outputOffset = index * OUTPUT_STRIDE_FLOATS;
    const center = Array.from(packedCases.slice(inputOffset, inputOffset + 3));
    const covariance = [
      packedCases[inputOffset + 4], packedCases[inputOffset + 5], packedCases[inputOffset + 6],
      packedCases[inputOffset + 8], packedCases[inputOffset + 9], packedCases[inputOffset + 10],
      packedCases[inputOffset + 12], packedCases[inputOffset + 13], packedCases[inputOffset + 14],
    ];
    const projection = projectGaussian(center, covariance, camera);
    const cpuValues = projectedValues(projection);
    const shaderValues = Array.from(gpuValues.slice(outputOffset, outputOffset + OUTPUT_STRIDE_FLOATS));
    const errors = cpuValues.map((value, valueIndex) => Math.abs(value - shaderValues[valueIndex]));
    const relativeErrors = errors.map((error, valueIndex) => (
      error / Math.max(1, Math.abs(cpuValues[valueIndex]))
    ));
    const toleranceRatios = errors.map((error, valueIndex) => {
      const tolerance = AGREEMENT_ABSOLUTE_TOLERANCE
        + AGREEMENT_RELATIVE_TOLERANCE * Math.abs(cpuValues[valueIndex]);
      return error / tolerance;
    });
    maxAbsoluteError = Math.max(maxAbsoluteError, ...errors);
    maxRelativeError = Math.max(maxRelativeError, ...relativeErrors);
    maxToleranceRatio = Math.max(maxToleranceRatio, ...toleranceRatios);
    return {
      label: item.label,
      center,
      projectedMeanPx: projection.mean,
      projectedCovariance: projection.covariance,
      conic: projection.conic,
      jacobian: projection.jacobian,
      gpuValues: shaderValues,
      maxAbsoluteError: Math.max(...errors),
    };
  });
  return { cases, maxAbsoluteError, maxRelativeError, maxToleranceRatio };
}

function trace2D(covariance) {
  return covariance[0] + covariance[2];
}

async function loadText(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Cannot load ${url.pathname}: HTTP ${response.status}`);
  return response.text();
}

function formatMessage(message) {
  const location = message.lineNum ? `${message.lineNum}:${message.linePos}` : 'unknown';
  return `${location} ${message.message}`;
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
