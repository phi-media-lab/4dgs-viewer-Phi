import assert from 'node:assert/strict';
import { test } from 'node:test';

import { createLessonSurface } from '../infra/page.js';

test('surface publishes PASS only when every assertion is exactly true', () => {
  const fake = installFakeDom();
  try {
    const surface = createLessonSurface(3);
    const details = { adapter: 'test adapter', width: 64, height: 64, format: 'rgba8unorm' };

    assert.throws(
      () => surface.pass(details, { shaderCompiled: true, pixelMatches: false }),
      /Cannot publish PASS: pixelMatches=false/,
    );

    const result = globalThis.__LESSON_RESULT__;
    assert.equal(result.status, 'FAIL');
    assert.deepEqual(result.details, details);
    assert.deepEqual(result.assertions, { shaderCompiled: true, pixelMatches: false });
    assert.equal(fake.documentElement.dataset.lessonStatus, 'FAIL');
    assert.deepEqual(JSON.parse(fake.resultElement.textContent), result);
    assert.ok(fake.resultHistory.every((entry) => JSON.parse(entry).status !== 'PASS'));
    assert.match(fake.error.textContent, /pixelMatches=false/);
  } finally {
    fake.restore();
  }
});

test('surface preserves a structured DOM result for a valid PASS', () => {
  const fake = installFakeDom();
  try {
    const surface = createLessonSurface(4);
    const details = { adapter: 'test adapter', width: 80, height: 45, format: 'bgra8unorm' };
    surface.pass(details, { shaderCompiled: true, cpuGpuAgreement: true });

    const result = JSON.parse(fake.resultElement.textContent);
    assert.equal(result.lesson, 4);
    assert.equal(result.status, 'PASS');
    assert.deepEqual(result.details, details);
    assert.deepEqual(result.assertions, { shaderCompiled: true, cpuGpuAgreement: true });
    assert.equal(fake.documentElement.dataset.lessonStatus, 'PASS');
  } finally {
    fake.restore();
  }
});

test('surface rejects truthy and empty assertion maps', () => {
  for (const [assertions, expected] of [
    [{ truthyButNotBoolean: 1 }, /truthyButNotBoolean=1/],
    [{}, /assertions=<empty>/],
  ]) {
    const fake = installFakeDom();
    try {
      const surface = createLessonSurface(6);
      assert.throws(
        () => surface.pass(
          { adapter: 'test adapter', width: 1, height: 1, format: 'rgba8unorm' },
          assertions,
        ),
        expected,
      );
      assert.equal(globalThis.__LESSON_RESULT__.status, 'FAIL');
      assert.equal(fake.documentElement.dataset.lessonStatus, 'FAIL');
    } finally {
      fake.restore();
    }
  }
});

function installFakeDom() {
  const originals = new Map([
    ['document', globalThis.document],
    ['addEventListener', globalThis.addEventListener],
    ['__LESSON_RESULT__', globalThis.__LESSON_RESULT__],
  ]);
  const originalConsoleError = console.error;
  const hud = { textContent: '' };
  const error = { textContent: '' };
  const documentElement = { dataset: {} };
  const resultHistory = [];
  let resultText = '';
  const resultElement = {
    id: '',
    type: '',
    get textContent() { return resultText; },
    set textContent(value) {
      resultText = value;
      resultHistory.push(value);
    },
  };

  globalThis.document = {
    documentElement,
    head: { append() {} },
    querySelector(selector) {
      if (selector === '#hud') return hud;
      if (selector === '#error') return error;
      return null;
    },
    createElement(tag) {
      assert.equal(tag, 'script');
      return resultElement;
    },
  };
  globalThis.addEventListener = () => {};
  console.error = () => {};

  return {
    documentElement,
    error,
    resultElement,
    resultHistory,
    restore() {
      console.error = originalConsoleError;
      for (const [name, value] of originals) {
        if (value === undefined) delete globalThis[name];
        else globalThis[name] = value;
      }
    },
  };
}
