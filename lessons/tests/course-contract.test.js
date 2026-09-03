import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import { extname } from 'node:path';
import { test } from 'node:test';

const lessons = [
  ['00', '00-environment'],
  ['01', '01-one-gaussian'],
  ['02', '02-projection'],
  ['03', '03-order-blend'],
  ['04', '04-explicit-time'],
  ['05', '05-active-set'],
  ['06', '06-complete-pipeline'],
];

const forbiddenAssetExtensions = new Set([
  '.bin', '.ckpt', '.jpeg', '.jpg', '.mov', '.mp4', '.npy', '.npz', '.ply',
  '.png', '.pt', '.pth', '.rgba8', '.safetensors', '.splat', '.webm',
]);

async function read(relative) {
  return readFile(new URL(`../${relative}`, import.meta.url), 'utf8');
}

async function lessonFiles(directory, relative = '') {
  const base = new URL(`../${directory}/${relative ? `${relative}/` : ''}`, import.meta.url);
  const entries = await readdir(base, { withFileTypes: true });
  const nested = await Promise.all(entries.map(async (entry) => {
    const path = relative ? `${relative}/${entry.name}` : entry.name;
    return entry.isDirectory() ? lessonFiles(directory, path) : [path];
  }));
  return nested.flat();
}

test('course contains seven directly runnable lessons', async () => {
  const catalog = await read('index.html');
  const vite = await read('vite.config.js');

  for (const [number, directory] of lessons) {
    const files = await lessonFiles(directory);
    for (const required of ['index.html', 'main.js', 'LESSON.md']) {
      assert.ok(files.includes(required), `${directory}/${required} is missing`);
    }
    assert.ok(files.some((name) => name.endsWith('.wgsl')), `${directory} has no WGSL`);
    assert.match(catalog, new RegExp(`href=["']\\./${directory}/`));
    assert.match(vite, new RegExp(`lesson${number}:[^\n]+${directory}/index\\.html`));
  }
});

test('every lesson owns its GPU command chain and result contract', async () => {
  for (const [number, directory] of lessons) {
    const main = await read(`${directory}/main.js`);
    assert.match(main, /\.\.\/infra\/gpu\.js/);
    assert.match(main, /\.\.\/infra\/page\.js/);
    assert.match(main, /device\.createShaderModule/);
    assert.match(main, /device\.createRenderPipeline(?:Async)?/);
    assert.match(main, /device\.createCommandEncoder/);
    assert.match(main, /device\.queue\.submit/);
    assert.match(main, /surface\.pass/);
    assert.match(main, new RegExp(`createLessonSurface\\(${Number(number)}\\)`));
    assert.doesNotMatch(main, /shared\/renderer|client-runtime|startLesson/);
  }
});

test('browser resources are relative and contain no bundled model assets', async () => {
  for (const [, directory] of lessons) {
    const files = await lessonFiles(directory);
    for (const name of files) {
      assert.ok(
        !forbiddenAssetExtensions.has(extname(name).toLowerCase()),
        `${directory}/${name} is a bundled model or media asset`,
      );
      if (!['.html', '.js', '.css', '.wgsl', '.md'].includes(extname(name))) continue;
      const source = await read(`${directory}/${name}`);
      assert.doesNotMatch(source, /(?:src|href)=["']\/(?!\/)/, `${directory}/${name} has a root-absolute URL`);
      assert.doesNotMatch(source, /from\s+["']\/(?!\/)/, `${directory}/${name} has a root-absolute import`);
      if (extname(name) !== '.md') {
        assert.doesNotMatch(source, /freetimegs|checkpoint|assets\/development/i);
      }
    }
  }
});

test('lesson documents expose the complete learning and verification loop', async () => {
  const requiredSections = [
    /## Learning goal/i,
    /## Prerequisites/i,
    /## Open (?:these )?files/i,
    /## Run and interact/i,
    /## Verifiable assertions/i,
    /## Modification experiment/i,
    /## Expected failure experiment/i,
    /## Common failures/i,
    /## .*Lesson 0[1-6]|## Next step/i,
  ];

  for (const [, directory] of lessons) {
    const document = await read(`${directory}/LESSON.md`);
    assert.match(document, /\$\$[\s\S]+\$\$/, `${directory}/LESSON.md has no display equation`);
    for (const section of requiredSections) {
      assert.match(document, section, `${directory}/LESSON.md lacks ${section}`);
    }
  }
});

test('lesson mathematics stays compatible with GitHub GFM', async () => {
  for (const [, directory] of lessons) {
    const document = await read(`${directory}/LESSON.md`);
    assert.doesNotMatch(
      document,
      /\\operatorname\b/,
      `${directory}/LESSON.md uses \\operatorname, which GitHub rejects`,
    );
    for (const [, equation] of document.matchAll(/\$\$([\s\S]*?)\$\$/g)) {
      assert.doesNotMatch(
        equation,
        /^[+*-] /m,
        `${directory}/LESSON.md starts a math line with a GFM list marker`,
      );
    }
  }
});
