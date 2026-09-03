const MAX_DEVICE_PIXEL_RATIO = 2;

export async function createGpuContext(canvas) {
  if (!navigator.gpu) {
    throw new Error(
      'WebGPU is unavailable. Open this page in a current WebGPU-capable browser ' +
      'with hardware acceleration enabled.',
    );
  }

  const adapter = await navigator.gpu.requestAdapter({
    powerPreference: 'high-performance',
  });
  if (!adapter) throw new Error('navigator.gpu.requestAdapter() returned no adapter.');

  const device = await adapter.requestDevice();
  const context = canvas.getContext('webgpu');
  if (!context) throw new Error('canvas.getContext("webgpu") failed.');

  const format = navigator.gpu.getPreferredCanvasFormat();
  let configuredWidth = 0;
  let configuredHeight = 0;

  async function configureCanvas() {
    const ratio = Math.min(globalThis.devicePixelRatio || 1, MAX_DEVICE_PIXEL_RATIO);
    const limit = device.limits.maxTextureDimension2D;
    const width = Math.min(limit, Math.max(1, Math.round(canvas.clientWidth * ratio)));
    const height = Math.min(limit, Math.max(1, Math.round(canvas.clientHeight * ratio)));

    const changed = width !== configuredWidth || height !== configuredHeight;
    if (changed) {
      canvas.width = width;
      canvas.height = height;
      await withGpuErrorScopes(device, async () => {
        context.configure({ device, format, alphaMode: 'opaque' });
      });
      configuredWidth = width;
      configuredHeight = height;
    }

    return { width, height, changed };
  }

  return {
    adapter,
    device,
    context,
    format,
    configureCanvas,
    adapterSummary: describeAdapter(adapter),
  };
}

export async function withGpuErrorScopes(device, operation) {
  device.pushErrorScope('internal');
  device.pushErrorScope('out-of-memory');
  device.pushErrorScope('validation');

  let value;
  let operationError;
  try {
    value = await operation();
  } catch (error) {
    operationError = error;
  }

  const validationError = await device.popErrorScope();
  const outOfMemoryError = await device.popErrorScope();
  const internalError = await device.popErrorScope();
  if (operationError) throw operationError;
  if (validationError) throw new Error(`WebGPU validation error: ${validationError.message}`);
  if (outOfMemoryError) throw new Error(`WebGPU out of memory: ${outOfMemoryError.message}`);
  if (internalError) throw new Error(`WebGPU internal error: ${internalError.message}`);
  return value;
}

function describeAdapter(adapter) {
  const info = adapter.info;
  if (!info) return 'WebGPU adapter';
  return info.description || [info.vendor, info.architecture].filter(Boolean).join(' · ') || 'WebGPU adapter';
}
