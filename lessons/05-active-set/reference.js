export const RECORD_COUNT = 64;
export const INITIAL_TIME = 0.5;
export const ALPHA_MIN = 0.08;

export function createProceduralRecords() {
  const records = [];
  for (let index = 0; index < RECORD_COUNT; index += 1) {
    const column = index % 8;
    const row = Math.floor(index / 8);
    const hue = (index * 0.61803398875) % 1;
    records.push({
      center: [
        -1.24 + column * 0.355 + 0.025 * Math.sin(index * 1.7),
        -1.05 + row * 0.3 + 0.02 * Math.cos(index * 1.3),
      ],
      sigma: [0.045 + (index % 3) * 0.008, 0.035 + (index % 4) * 0.007],
      timeCenter: ((index * 11) % RECORD_COUNT) / (RECORD_COUNT - 1),
      timeSigma: 0.075 + (index % 5) * 0.012,
      opacity: 0.55 + (index % 4) * 0.1,
      color: hueToRgb(hue),
    });
  }
  return records;
}

export function temporalAlpha(record, time) {
  const normalized = (time - record.timeCenter) / record.timeSigma;
  return record.opacity * Math.exp(-0.5 * normalized * normalized);
}

export function isVisible(record) {
  const extentX = 3 * record.sigma[0];
  const extentY = 3 * record.sigma[1];
  return Math.abs(record.center[0]) <= 1 + extentX
    && Math.abs(record.center[1]) <= 1 + extentY;
}

export function classifyRecords(records, time, alphaMin = ALPHA_MIN) {
  const activeIndices = [];
  const visibleIndices = [];
  for (let index = 0; index < records.length; index += 1) {
    const record = records[index];
    if (temporalAlpha(record, time) < alphaMin) continue;
    activeIndices.push(index);
    if (isVisible(record)) visibleIndices.push(index);
  }
  return { total: records.length, activeIndices, visibleIndices };
}

function hueToRgb(hue) {
  const angle = hue * Math.PI * 2;
  return [
    0.55 + 0.4 * Math.cos(angle),
    0.55 + 0.4 * Math.cos(angle - (Math.PI * 2) / 3),
    0.55 + 0.4 * Math.cos(angle + (Math.PI * 2) / 3),
  ];
}
