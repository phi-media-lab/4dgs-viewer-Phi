import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  covariance2D,
  gaussianWeight,
} from '../01-one-gaussian/reference.js';
import {
  covarianceFromScaleRotation,
  projectGaussian,
} from '../02-projection/reference.js';
import {
  compositeFrontToBack,
  sortFrontToBack,
} from '../03-order-blend/reference.js';
import {
  checkCpuInvariants,
  evaluatePrimitive,
} from '../04-explicit-time/reference.js';
import {
  ALPHA_MIN,
  INITIAL_TIME,
  classifyRecords,
  createProceduralRecords,
} from '../05-active-set/reference.js';
import {
  ASSET_INPUT,
  createProceduralAsset,
  loadLessonAsset,
  validateAssetEnvelope,
} from '../06-complete-pipeline/asset-contract.js';
import {
  colorDistance,
  compositeAtNdc,
  evaluateReference,
} from '../06-complete-pipeline/reference.js';

const EPSILON = 1e-10;

test('lesson 01 Gaussian and covariance invariants are analytic', () => {
  assert.equal(gaussianWeight([0, 0], 0.75), 0.75);
  assert.ok(Math.abs(gaussianWeight([1, 0]) - Math.exp(-0.5)) < EPSILON);

  const unrotated = covariance2D([2, 1], 0);
  const quarterTurn = covariance2D([2, 1], Math.PI / 2);
  assert.deepEqual(unrotated, [4, 0, 1]);
  assert.ok(Math.abs(quarterTurn[0] - 1) < EPSILON);
  assert.ok(Math.abs(quarterTurn[1]) < EPSILON);
  assert.ok(Math.abs(quarterTurn[2] - 4) < EPSILON);
});

test('lesson 02 perspective projection shrinks covariance with depth', () => {
  const covariance = covarianceFromScaleRotation([0.2, 0.1, 0.3]);
  const camera = { focal: [1, 1], principal: [0, 0], minimumVariance: 0 };
  const near = projectGaussian([0, 0, 2], covariance, camera);
  const far = projectGaussian([0, 0, 4], covariance, camera);

  assert.ok(Math.abs(near.covariance[0] / far.covariance[0] - 4) < EPSILON);
  assert.ok(Math.abs(near.covariance[2] / far.covariance[2] - 4) < EPSILON);
  assert.ok(near.determinant > 0 && far.determinant > 0);
  assert.ok(near.conic.every(Number.isFinite));
});

test('lesson 03 order changes non-commutative alpha compositing', () => {
  const records = [
    { mean: [0, 0], sigma: [1, 1], opacity: 0.5, color: [1, 0, 0], depth: 1 },
    { mean: [0, 0], sigma: [1, 1], opacity: 0.5, color: [0, 0, 1], depth: 2 },
  ];
  const order = sortFrontToBack(records);
  assert.deepEqual(order, [0, 1]);

  const correct = compositeFrontToBack(records, order, [0, 0], [0, 0, 0]);
  const reversed = compositeFrontToBack(records, [...order].reverse(), [0, 0], [0, 0, 0]);
  assert.deepEqual(correct.color, [0.5, 0, 0.25]);
  assert.deepEqual(reversed.color, [0.25, 0, 0.5]);
  assert.equal(correct.transmittance, 0.25);
});

test('lesson 04 static and moving primitives obey distinct time rules', () => {
  const staticPrimitive = {
    mean: [0.2, -0.1], velocity: [3, 4], color: [1, 1, 1],
    timeCenter: 0.5, duration: 0.2, opacity: 0.8, moving: 0,
    scale: [0.1, 0.1], depth: 1,
  };
  const movingPrimitive = { ...staticPrimitive, moving: 1 };
  assert.deepEqual(evaluatePrimitive(staticPrimitive, 0.8).mean, staticPrimitive.mean);
  assert.equal(evaluatePrimitive(staticPrimitive, 0.8).gate, 1);
  assert.notDeepEqual(evaluatePrimitive(movingPrimitive, 0.8).mean, movingPrimitive.mean);
  assert.ok(evaluatePrimitive(movingPrimitive, 0.8).gate < 1);
  assert.ok(Object.values(checkCpuInvariants([staticPrimitive, movingPrimitive])).every(Boolean));
});

test('lesson 05 procedural active and visible sets are deterministic', () => {
  const records = createProceduralRecords();
  const result = classifyRecords(records, INITIAL_TIME, ALPHA_MIN);
  assert.equal(records.length, 64);
  assert.equal(result.total, 64);
  assert.equal(result.activeIndices.length, 27);
  assert.equal(result.visibleIndices.length, 20);
  assert.ok(result.visibleIndices.every((index) => result.activeIndices.includes(index)));
});

test('lesson 06 keeps the external asset slot empty and uses no fetch for fallback', async () => {
  assert.equal(ASSET_INPUT.manifestUrl, null);
  let fetchTouched = false;
  const asset = await loadLessonAsset(ASSET_INPUT, async () => {
    fetchTouched = true;
    throw new Error('procedural fallback must not fetch');
  });
  assert.equal(fetchTouched, false);
  assert.equal(asset.source.kind, 'procedural');
  assert.equal(asset.records.length, 32);
});

test('lesson 06 reference exposes a visible depth-order witness', () => {
  const asset = createProceduralAsset();
  const reference = evaluateReference(asset.records, asset.manifest, {
    time: asset.manifest.time.initial,
    aspect: 1.5,
  });
  const visible = reference.sorted.filter((item) => item.valid);
  assert.equal(reference.visibleCount, 30);
  assert.ok(visible.every((item, index) => index === 0 || visible[index - 1].depth >= item.depth));

  const wrong = reference.projected.filter((item) => item.valid);
  const correctPixel = compositeAtNdc(reference.sorted, [0, 0], 0);
  const wrongPixel = compositeAtNdc(wrong, [0, 0], 0);
  assert.ok(colorDistance(correctPixel.color, wrongPixel.color) > 0.01);
});

test('lesson 06 rejects a manifest/record count mismatch', () => {
  const asset = createProceduralAsset();
  asset.records.pop();
  assert.throws(() => validateAssetEnvelope(asset), /declares 32 records, received 31/);
});

test('lesson 06 rejects an absolute external records URI', () => {
  const asset = createProceduralAsset();
  asset.manifest.records.uri = 'https://example.invalid/records.json';
  asset.source = {
    kind: 'external',
    manifestUrl: 'https://example.invalid/manifest.json',
    recordUrl: 'https://example.invalid/records.json',
  };
  assert.throws(() => validateAssetEnvelope(asset), /records\.uri must be a relative URL/);
});

test('lesson 06 rejects finite JavaScript numbers that overflow f32', () => {
  const asset = createProceduralAsset();
  asset.records[0].center[0] = Number.MAX_VALUE;
  assert.throws(
    () => validateAssetEnvelope(asset),
    /must contain 3 values representable as finite f32/,
  );
});
