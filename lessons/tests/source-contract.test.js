import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';

const sources = [
  'index.html',
  '00-environment/index.html',
  '00-environment/main.js',
  'infra/gpu.js',
  'infra/page.js',
  'infra/style.css',
];

async function read(path) {
  return readFile(new URL(`../${path}`, import.meta.url), 'utf8');
}

test('all browser resources are base-path safe', async () => {
  for (const path of sources) {
    const source = await read(path);
    assert.doesNotMatch(source, /(?:src|href)=["']\/(?!\/)/, `${path} has a root-absolute HTML URL`);
    assert.doesNotMatch(source, /from\s+["']\/(?!\/)/, `${path} has a root-absolute JS import`);
  }
});

test('lesson 00 imports only the two audited infrastructure modules', async () => {
  const lesson = await read('00-environment/main.js');
  const imports = [...lesson.matchAll(/\bfrom\s+['"]([^'"]+)['"]/g)]
    .map((match) => match[1])
    .sort();
  assert.deepEqual(imports, ['../infra/gpu.js', '../infra/page.js']);

  for (const path of ['infra/gpu.js', 'infra/page.js']) {
    const source = await read(path);
    assert.doesNotMatch(source, /\b(?:import|export)\b[^'";]*\bfrom\s+['"]/s);
    assert.doesNotMatch(source, /\bimport\s*\(/);
  }
});

test('lesson 00 owns the shader, pipeline, encoding, and submission chain', async () => {
  const lesson = await read('00-environment/main.js');
  const gpu = await read('infra/gpu.js');
  for (const operation of [
    'device.createShaderModule',
    'device.createRenderPipelineAsync',
    'device.createCommandEncoder',
    'device.queue.submit',
    'device.queue.onSubmittedWorkDone',
  ]) {
    assert.match(lesson, new RegExp(operation.replaceAll('.', '\\.')));
    assert.doesNotMatch(gpu, new RegExp(operation.replaceAll('.', '\\.')));
  }
});

test('lesson exposes a deterministic browser result contract', async () => {
  const page = await read('infra/page.js');
  const lesson = await read('00-environment/main.js');
  const gpu = await read('infra/gpu.js');
  assert.match(page, /__LESSON_RESULT__/);
  assert.match(page, /status:\s*'PASS'/);
  assert.match(lesson, /addEventListener\('uncapturederror'/);
  assert.match(lesson, /current !== observed/);
  assert.match(gpu, /pushErrorScope\('internal'\)/);
  assert.match(gpu, /pushErrorScope\('out-of-memory'\)/);
  assert.match(gpu, /pushErrorScope\('validation'\)/);
  for (const assertion of [
    'webGpuAvailable',
    'adapterCreated',
    'deviceCreated',
    'canvasConfigured',
    'shaderCompiled',
    'pipelineCreated',
    'frameSubmitted',
    'gpuWorkCompleted',
  ]) {
    assert.match(lesson, new RegExp(`${assertion}:\\s*true`));
  }
});

test('WGSL contains exactly the lesson 00 render entry points', async () => {
  const shader = await read('00-environment/environment.wgsl');
  assert.match(shader, /@vertex\s+fn\s+vs_main/);
  assert.match(shader, /@fragment\s+fn\s+fs_main/);
  assert.equal((shader.match(/@(vertex|fragment|compute)\b/g) ?? []).length, 2);
  assert.doesNotMatch(shader, /@compute\b/);
});

test('lesson 04 time uniform preserves its 32-byte CPU/WGSL contract', async () => {
  const main = await read('04-explicit-time/main.js');
  const shader = await read('04-explicit-time/explicit-time.wgsl');

  const bufferDeclaration = main.match(
    /const timeBuffer = device\.createBuffer\(\{([\s\S]*?)\n    \}\);/,
  );
  assert.ok(bufferDeclaration, 'Lesson 04 time buffer declaration is missing');
  assert.match(bufferDeclaration[1], /label: 'lesson 04 current time'/);
  assert.match(bufferDeclaration[1], /size: 32/);
  assert.match(
    bufferDeclaration[1],
    /usage: GPUBufferUsage\.UNIFORM \| GPUBufferUsage\.COPY_DST/,
  );
  assert.match(
    main,
    /writeBuffer\(resources\.timeBuffer, 0, new Float32Array\(\[time, 0, 0, 0\]\)\)/,
  );
  assert.match(
    shader,
    /struct TimeState \{\s*time: f32,\s*_padding: vec3<f32>,\s*\};/s,
  );
  assert.match(shader, /@binding\(1\) var<uniform> time_state: TimeState;/);
});
