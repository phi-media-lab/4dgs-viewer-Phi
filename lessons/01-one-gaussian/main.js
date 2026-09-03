import {
  createGpuContext,
  withGpuErrorScopes,
} from '../infra/gpu.js';
import { createLessonSurface } from '../infra/page.js';
import {
  covariance2D,
  GAUSSIAN_SAMPLE,
  gaussianWeight,
} from './reference.js';

const canvas = document.querySelector('canvas');
const hud = document.querySelector('#hud');
const surface = createLessonSurface(1);

const initialState = Object.freeze({
  center: [0.5, 0.5],
  radius: 0.145,
  anisotropy: 0.46,
  rotation: -0.45,
  opacity: 0.92,
});
const state = structuredClone(initialState);

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

  surface.progress('compiling analytic footprint');
  const shaderUrl = new URL('./one-gaussian.wgsl', import.meta.url);
  const shaderCode = await loadText(shaderUrl);
  const { module, warningCount } = await withGpuErrorScopes(device, async () => {
    const module = device.createShaderModule({
      label: 'lesson 01 one-Gaussian shader',
      code: shaderCode,
    });
    const compilation = await module.getCompilationInfo();
    const errors = compilation.messages.filter((message) => message.type === 'error');
    if (errors.length > 0) {
      throw new Error(`lesson 01 shader did not compile:\n${errors.map(formatMessage).join('\n')}`);
    }
    return {
      module,
      warningCount: compilation.messages.filter((message) => message.type === 'warning').length,
    };
  });

  surface.progress('creating render and verification pipelines');
  const renderPipeline = await withGpuErrorScopes(
    device,
    async () => device.createRenderPipelineAsync({
      label: 'lesson 01 analytic splat pipeline',
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
      label: 'lesson 01 verification pipeline',
      layout: 'auto',
      compute: { module, entryPoint: 'verify_gaussian' },
    }),
  );

  const uniformBuffer = device.createBuffer({
    label: 'lesson 01 Gaussian2D uniform',
    size: 48,
    usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
  });
  const verificationBuffer = device.createBuffer({
    label: 'lesson 01 verification output',
    size: 16,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC,
  });
  const readbackBuffer = device.createBuffer({
    label: 'lesson 01 verification readback',
    size: 16,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ,
  });
  const renderBindGroup = device.createBindGroup({
    label: 'lesson 01 render bind group',
    layout: renderPipeline.getBindGroupLayout(0),
    entries: [{ binding: 0, resource: { buffer: uniformBuffer } }],
  });
  const verificationBindGroup = device.createBindGroup({
    label: 'lesson 01 verification bind group',
    layout: verificationPipeline.getBindGroupLayout(0),
    entries: [
      { binding: 0, resource: { buffer: uniformBuffer } },
      { binding: 1, resource: { buffer: verificationBuffer } },
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
          const shortSide = Math.min(width, height);
          const centerPx = [state.center[0] * width, state.center[1] * height];
          const sigmaPx = [
            state.radius * shortSide,
            state.radius * state.anisotropy * shortSide,
          ];
          const uniform = new Float32Array([
            width, height,
            centerPx[0], centerPx[1],
            sigmaPx[0], sigmaPx[1],
            state.rotation, state.opacity,
            GAUSSIAN_SAMPLE[0], GAUSSIAN_SAMPLE[1],
            0, 0,
          ]);
          device.queue.writeBuffer(uniformBuffer, 0, uniform);

          await withGpuErrorScopes(device, async () => {
            const encoder = device.createCommandEncoder({ label: 'lesson 01 frame encoder' });
            const computePass = encoder.beginComputePass({ label: 'lesson 01 CPU/WGSL check' });
            computePass.setPipeline(verificationPipeline);
            computePass.setBindGroup(0, verificationBindGroup);
            computePass.dispatchWorkgroups(1);
            computePass.end();

            const renderPass = encoder.beginRenderPass({
              label: 'lesson 01 render pass',
              colorAttachments: [{
                view: context.getCurrentTexture().createView(),
                clearValue: { r: 0.025, g: 0.025, b: 0.028, a: 1 },
                loadOp: 'clear',
                storeOp: 'store',
              }],
            });
            renderPass.setPipeline(renderPipeline);
            renderPass.setBindGroup(0, renderBindGroup);
            renderPass.draw(6);
            renderPass.end();
            encoder.copyBufferToBuffer(verificationBuffer, 0, readbackBuffer, 0, 16);
            device.queue.submit([encoder.finish()]);
            await device.queue.onSubmittedWorkDone();
          });

          await readbackBuffer.mapAsync(GPUMapMode.READ);
          const verification = new Float32Array(readbackBuffer.getMappedRange()).slice();
          readbackBuffer.unmap();
          const cpuAlpha = gaussianWeight(GAUSSIAN_SAMPLE, state.opacity);
          const gpuAlpha = verification[0];
          const absoluteError = Math.abs(cpuAlpha - gpuAlpha);
          if (!Number.isFinite(gpuAlpha) || absoluteError > 1e-6) {
            throw new Error(`CPU/WGSL Gaussian mismatch: |${cpuAlpha} - ${gpuAlpha}| = ${absoluteError}`);
          }

          frameCount += 1;
          const covariance = covariance2D(sigmaPx, state.rotation);
          surface.pass(
            {
              adapter: gpu.adapterSummary,
              format,
              width,
              height,
              warningCount,
              frameCount,
              centerPx,
              sigmaPx,
              rotationRadians: state.rotation,
              opacity: state.opacity,
              covariance,
              verificationSample: GAUSSIAN_SAMPLE,
              cpuSampleAlpha: cpuAlpha,
              gpuSampleAlpha: gpuAlpha,
              absoluteError,
            },
            {
              webGpuAvailable: true,
              shaderCompiled: true,
              renderPipelineCreated: true,
              verificationPipelineCreated: true,
              analyticFootprintRendered: true,
              cpuGpuAgreement: true,
              frameSubmitted: true,
              gpuWorkCompleted: true,
            },
          );
          hud.textContent += [
            `\nμ ${formatPair(centerPx)} px · σ ${formatPair(sigmaPx)} px`,
            `CPU/WGSL Δ ${absoluteError.toExponential(2)}`,
            'drag · wheel size · A/D rotate · W/S anisotropy · R reset',
          ].join('\n');
        }
      })().finally(() => {
        drawingPromise = null;
        if (drawRequested) void requestDraw().catch(surface.fail);
      });
    }
    return drawingPromise;
  }

  installControls(canvas, state, () => void requestDraw().catch(surface.fail));
  await requestDraw();
  new ResizeObserver(() => void requestDraw().catch(surface.fail)).observe(canvas);
  observeDevicePixelRatio(() => void requestDraw().catch(surface.fail));
}

start().catch(surface.fail);

function installControls(target, mutableState, redraw) {
  target.addEventListener('pointerdown', (event) => {
    target.setPointerCapture(event.pointerId);
    moveCenter(event);
  });
  target.addEventListener('pointermove', (event) => {
    if (target.hasPointerCapture(event.pointerId)) moveCenter(event);
  });
  target.addEventListener('wheel', (event) => {
    event.preventDefault();
    mutableState.radius = clamp(mutableState.radius * Math.exp(-event.deltaY * 0.001), 0.035, 0.32);
    redraw();
  }, { passive: false });
  addEventListener('keydown', (event) => {
    const actions = {
      KeyA: () => { mutableState.rotation -= 0.08; },
      KeyD: () => { mutableState.rotation += 0.08; },
      KeyW: () => { mutableState.anisotropy = clamp(mutableState.anisotropy + 0.05, 0.12, 1); },
      KeyS: () => { mutableState.anisotropy = clamp(mutableState.anisotropy - 0.05, 0.12, 1); },
      KeyR: () => Object.assign(mutableState, structuredClone(initialState)),
    };
    const action = actions[event.code];
    if (action) {
      event.preventDefault();
      action();
      redraw();
    }
  });

  function moveCenter(event) {
    const bounds = target.getBoundingClientRect();
    mutableState.center[0] = clamp((event.clientX - bounds.left) / bounds.width, 0, 1);
    mutableState.center[1] = clamp((event.clientY - bounds.top) / bounds.height, 0, 1);
    redraw();
  }
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

function formatPair(values) {
  return values.map((value) => value.toFixed(1)).join(', ');
}

function clamp(value, minimum, maximum) {
  return Math.min(maximum, Math.max(minimum, value));
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
