import {
  createGpuContext,
  withGpuErrorScopes,
} from '../infra/gpu.js';
import { createLessonSurface } from '../infra/page.js';

const canvas = document.querySelector('canvas');
const surface = createLessonSurface(0);

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

  surface.progress('compiling WGSL');
  const shaderUrl = new URL('./environment.wgsl', import.meta.url);
  const shaderResponse = await fetch(shaderUrl);
  if (!shaderResponse.ok) {
    throw new Error(`Cannot load ${shaderUrl.pathname}: HTTP ${shaderResponse.status}`);
  }
  const shaderCode = await shaderResponse.text();
  const { module, warningCount } = await withGpuErrorScopes(device, async () => {
    const module = device.createShaderModule({
      label: 'lesson 00 shader',
      code: shaderCode,
    });
    const compilation = await module.getCompilationInfo();
    const errors = compilation.messages.filter((message) => message.type === 'error');
    if (errors.length > 0) {
      const details = errors.map(formatCompilationMessage).join('\n');
      throw new Error(`lesson 00 shader did not compile:\n${details}`);
    }
    return {
      module,
      warningCount: compilation.messages.filter((message) => message.type === 'warning').length,
    };
  });

  surface.progress('creating pipeline');
  const pipeline = await withGpuErrorScopes(
    device,
    async () => device.createRenderPipelineAsync({
      label: 'lesson 00 pipeline',
      layout: 'auto',
      vertex: { module, entryPoint: 'vs_main' },
      fragment: { module, entryPoint: 'fs_main', targets: [{ format }] },
      primitive: { topology: 'triangle-list' },
    }),
  );

  let frameCount = 0;
  let drawingPromise = null;
  let drawRequested = false;

  function requestDraw() {
    drawRequested = true;
    if (!drawingPromise) {
      drawingPromise = (async () => {
        while (drawRequested) {
          drawRequested = false;
          const { width, height, changed } = await gpu.configureCanvas();
          if (width < 1 || height < 1) throw new Error('WebGPU canvas has no drawable pixels.');
          if (!changed && frameCount > 0) continue;
          await withGpuErrorScopes(device, async () => {
            const encoder = device.createCommandEncoder({ label: 'lesson 00 frame encoder' });
            const pass = encoder.beginRenderPass({
              label: 'lesson 00 render pass',
              colorAttachments: [{
                view: context.getCurrentTexture().createView(),
                clearValue: { r: 0.035, g: 0.035, b: 0.035, a: 1 },
                loadOp: 'clear',
                storeOp: 'store',
              }],
            });
            pass.setPipeline(pipeline);
            pass.draw(3);
            pass.end();
            device.queue.submit([encoder.finish()]);
            await device.queue.onSubmittedWorkDone();
          });
          frameCount += 1;

          surface.pass(
            {
              adapter: gpu.adapterSummary,
              format,
              width,
              height,
              warningCount,
              frameCount,
            },
            {
              webGpuAvailable: true,
              adapterCreated: true,
              deviceCreated: true,
              canvasConfigured: true,
              shaderCompiled: true,
              shaderErrorCountIsZero: true,
              pipelineCreated: true,
              frameSubmitted: true,
              gpuWorkCompleted: true,
            },
          );
        }
      })().finally(() => {
        drawingPromise = null;
        // A resize can land between the last loop test and finally(). Keep it.
        if (drawRequested) void requestDraw().catch(surface.fail);
      });
    }
    return drawingPromise;
  }

  await requestDraw();
  const resizeObserver = new ResizeObserver(() => void requestDraw().catch(surface.fail));
  resizeObserver.observe(canvas);
  observeDevicePixelRatio(() => void requestDraw().catch(surface.fail));
}

start().catch(surface.fail);

function formatCompilationMessage(message) {
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
