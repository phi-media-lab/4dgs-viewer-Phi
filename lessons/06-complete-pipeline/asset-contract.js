export const MANIFEST_SCHEMA = 'phi.4dgs.lesson-manifest.v1';
export const RECORDS_SCHEMA = 'phi.4dgs.lesson-records.v1';
export const RECORD_ENCODING = 'json-array-f32-v1';

// Populate this one slot with a manifest URL when an external teaching asset
// is deliberately supplied. The checked-in lesson leaves it empty.
export const ASSET_INPUT = Object.freeze({ manifestUrl: null });

export async function loadLessonAsset(input = ASSET_INPUT, fetchImpl = globalThis.fetch) {
  if (input.manifestUrl === null) {
    return validateAssetEnvelope(createProceduralAsset());
  }
  if (typeof input.manifestUrl !== 'string' || input.manifestUrl.length === 0) {
    throw new Error('asset input manifestUrl must be null or a non-empty URL string');
  }
  if (typeof fetchImpl !== 'function') throw new Error('fetch is unavailable for external asset input');

  const manifestUrl = new URL(input.manifestUrl, import.meta.url);
  const manifest = await fetchJson(fetchImpl, manifestUrl, 'asset manifest');
  validateManifest(manifest, { external: true });
  const recordsUrl = new URL(manifest.records.uri, manifestUrl);
  const recordDocument = await fetchJson(fetchImpl, recordsUrl, 'record document');
  exactKeys(recordDocument, ['schema', 'records'], 'record document');
  if (recordDocument.schema !== RECORDS_SCHEMA) {
    throw new Error(`record document schema must be ${RECORDS_SCHEMA}`);
  }
  return validateAssetEnvelope({
    manifest,
    records: recordDocument.records,
    source: {
      kind: 'external',
      manifestUrl: manifestUrl.href,
      recordUrl: recordsUrl.href,
    },
  });
}

export function validateAssetEnvelope(envelope) {
  exactKeys(envelope, ['manifest', 'records', 'source'], 'asset envelope');
  validateManifest(envelope.manifest, { external: envelope.source.kind === 'external' });
  exactKeys(envelope.source, ['kind', 'manifestUrl', 'recordUrl'], 'asset source');
  if (!['procedural', 'external'].includes(envelope.source.kind)) {
    throw new Error('asset source kind must be procedural or external');
  }
  if (!Array.isArray(envelope.records)) throw new Error('records must be an array');
  if (envelope.records.length !== envelope.manifest.records.count) {
    throw new Error(
      `manifest declares ${envelope.manifest.records.count} records, received ${envelope.records.length}`,
    );
  }
  if (!isPowerOfTwo(envelope.records.length) || envelope.records.length > 256) {
    throw new Error('lesson 06 requires a power-of-two record count between 2 and 256');
  }
  envelope.records.forEach(validateRecord);
  return envelope;
}

export function createProceduralAsset() {
  const recordCount = 32;
  const manifest = {
    schema: MANIFEST_SCHEMA,
    version: 1,
    name: 'procedural-depth-and-motion',
    time: { start: 0, end: 1, initial: 0.5 },
    camera: { focalY: 1.35, near: 0.2, far: 10, minSigmaNdc: 0.003 },
    render: { alphaMin: 0.035 },
    records: { encoding: RECORD_ENCODING, count: recordCount, uri: null },
  };
  const records = [];
  for (let index = 0; index < recordCount; index += 1) {
    const depthRank = (index * 13) % recordCount;
    const depth = 2.35 + depthRank * 0.105;
    const clustered = index < 12;
    const angle = (index / recordCount) * Math.PI * 2;
    const ndcX = clustered
      ? 0.035 * Math.cos(index * 2.1)
      : 0.67 * Math.cos(angle);
    const ndcY = clustered
      ? 0.035 * Math.sin(index * 1.7)
      : 0.48 * Math.sin(angle);
    const nominalAspect = 1.5;
    const color = clustered
      ? clusterColor(depthRank)
      : hueToRgb((index * 0.61803398875) % 1);
    records.push({
      id: `g${String(index).padStart(2, '0')}`,
      center: [
        (ndcX * depth * nominalAspect) / manifest.camera.focalY,
        (ndcY * depth) / manifest.camera.focalY,
        depth,
      ],
      velocity: clustered
        ? [0.12 * Math.sin(index), 0.1 * Math.cos(index * 1.3), 0]
        : [-0.16 * Math.sin(angle), 0.12 * Math.cos(angle), 0],
      scale: clustered
        ? [0.13 + (index % 3) * 0.02, 0.1 + (index % 2) * 0.025, 0.16]
        : [0.08 + (index % 3) * 0.012, 0.065 + (index % 2) * 0.012, 0.1],
      color,
      opacity: clustered ? 0.7 : 0.58,
      timeCenter: clustered ? 0.5 : 0.15 + ((index * 7) % 15) * 0.05,
      timeSigma: clustered ? 0.34 : 0.13 + (index % 3) * 0.018,
    });
  }
  return {
    manifest,
    records,
    source: { kind: 'procedural', manifestUrl: null, recordUrl: null },
  };
}

function validateManifest(manifest, { external }) {
  exactKeys(manifest, ['schema', 'version', 'name', 'time', 'camera', 'render', 'records'], 'manifest');
  if (manifest.schema !== MANIFEST_SCHEMA) throw new Error(`manifest schema must be ${MANIFEST_SCHEMA}`);
  if (manifest.version !== 1) throw new Error('manifest version must be 1');
  if (typeof manifest.name !== 'string' || manifest.name.trim() === '') {
    throw new Error('manifest name must be a non-empty string');
  }

  exactKeys(manifest.time, ['start', 'end', 'initial'], 'manifest.time');
  finiteTuple([manifest.time.start, manifest.time.end, manifest.time.initial], 3, 'manifest.time');
  if (!(manifest.time.start < manifest.time.end)) throw new Error('time start must be less than end');
  if (manifest.time.initial < manifest.time.start || manifest.time.initial > manifest.time.end) {
    throw new Error('initial time must lie inside the time interval');
  }

  exactKeys(manifest.camera, ['focalY', 'near', 'far', 'minSigmaNdc'], 'manifest.camera');
  finiteTuple(
    [manifest.camera.focalY, manifest.camera.near, manifest.camera.far, manifest.camera.minSigmaNdc],
    4,
    'manifest.camera',
  );
  if (!(manifest.camera.focalY > 0)) throw new Error('camera focalY must be positive');
  if (!(manifest.camera.near > 0 && manifest.camera.near < manifest.camera.far)) {
    throw new Error('camera clip interval is invalid');
  }
  if (!(manifest.camera.minSigmaNdc > 0)) throw new Error('camera minSigmaNdc must be positive');

  exactKeys(manifest.render, ['alphaMin'], 'manifest.render');
  finiteTuple([manifest.render.alphaMin], 1, 'manifest.render');
  if (!(manifest.render.alphaMin > 0 && manifest.render.alphaMin < 1)) {
    throw new Error('render alphaMin must be inside (0, 1)');
  }

  exactKeys(manifest.records, ['encoding', 'count', 'uri'], 'manifest.records');
  if (manifest.records.encoding !== RECORD_ENCODING) {
    throw new Error(`record encoding must be ${RECORD_ENCODING}`);
  }
  if (!Number.isInteger(manifest.records.count) || manifest.records.count < 2) {
    throw new Error('record count must be an integer of at least 2');
  }
  if (external && (typeof manifest.records.uri !== 'string' || manifest.records.uri.length === 0)) {
    throw new Error('an external manifest requires a non-empty records.uri');
  }
  if (external && !isRelativeUrl(manifest.records.uri)) {
    throw new Error('external records.uri must be a relative URL');
  }
  if (!external && manifest.records.uri !== null) {
    throw new Error('the procedural fallback must keep records.uri null');
  }
}

function validateRecord(record, index) {
  const label = `record ${index}`;
  exactKeys(
    record,
    ['id', 'center', 'velocity', 'scale', 'color', 'opacity', 'timeCenter', 'timeSigma'],
    label,
  );
  if (typeof record.id !== 'string' || record.id.length === 0) throw new Error(`${label} id is invalid`);
  finiteTuple(record.center, 3, `${label}.center`);
  finiteTuple(record.velocity, 3, `${label}.velocity`);
  finiteTuple(record.scale, 3, `${label}.scale`);
  finiteTuple(record.color, 3, `${label}.color`);
  finiteTuple([record.opacity, record.timeCenter, record.timeSigma], 3, label);
  if (record.scale.some((value) => value <= 0)) throw new Error(`${label} scale must be positive`);
  if (record.color.some((value) => value < 0 || value > 1)) {
    throw new Error(`${label} color must stay inside [0, 1]`);
  }
  if (!(record.opacity > 0 && record.opacity <= 1)) throw new Error(`${label} opacity is invalid`);
  if (!(record.timeSigma > 0)) throw new Error(`${label} timeSigma must be positive`);
}

async function fetchJson(fetchImpl, url, label) {
  const response = await fetchImpl(url);
  if (!response.ok) throw new Error(`cannot load ${label} ${url.pathname}: HTTP ${response.status}`);
  try {
    return await response.json();
  } catch (error) {
    throw new Error(`${label} is not valid JSON: ${error.message}`);
  }
}

function exactKeys(value, expected, label) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) {
    throw new Error(`${label} keys must be exactly: ${wanted.join(', ')}`);
  }
}

function finiteTuple(value, length, label) {
  if (
    !Array.isArray(value)
    || value.length !== length
    || value.some((item) => !Number.isFinite(item) || !Number.isFinite(Math.fround(item)))
  ) {
    throw new Error(`${label} must contain ${length} values representable as finite f32`);
  }
}

function isRelativeUrl(value) {
  if (value.startsWith('/') || value.includes('\\')) return false;
  if (/^[a-z][a-z\d+.-]*:/i.test(value)) return false;
  try {
    const base = new URL('https://lesson.invalid/assets/manifest.json');
    return new URL(value, base).origin === base.origin;
  } catch {
    return false;
  }
}

function isPowerOfTwo(value) {
  return value > 0 && (value & (value - 1)) === 0;
}

function clusterColor(rank) {
  if (rank < 11) return [0.2, 0.55, 0.96];
  if (rank < 22) return [0.24, 0.86, 0.45];
  return [0.98, 0.34, 0.18];
}

function hueToRgb(hue) {
  const angle = hue * Math.PI * 2;
  return [
    0.58 + 0.36 * Math.cos(angle),
    0.58 + 0.36 * Math.cos(angle - (Math.PI * 2) / 3),
    0.58 + 0.36 * Math.cos(angle + (Math.PI * 2) / 3),
  ];
}
