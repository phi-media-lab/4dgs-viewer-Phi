export const DEFAULT_BACKGROUND = Object.freeze([0.025, 0.025, 0.03]);

export function sortFrontToBack(records) {
  return records
    .map((record, index) => ({ depth: record.depth, index }))
    .sort((a, b) => a.depth - b.depth || a.index - b.index)
    .map(({ index }) => index);
}

export function gaussianAlpha(record, point) {
  const x = (point[0] - record.mean[0]) / record.sigma[0];
  const y = (point[1] - record.mean[1]) / record.sigma[1];
  return Math.min(0.999, Math.max(0, record.opacity * Math.exp(-0.5 * (x * x + y * y))));
}

// Front-to-back accumulation stores premultiplied color and covered alpha:
//   C' = C + (1 - A) alpha_i color_i
//   A' = A + (1 - A) alpha_i
export function compositeFrontToBack(records, order, point, background = DEFAULT_BACKGROUND) {
  const color = [0, 0, 0];
  let alpha = 0;

  for (const index of order) {
    const record = records[index];
    const sampleAlpha = gaussianAlpha(record, point);
    const weight = (1 - alpha) * sampleAlpha;
    for (let channel = 0; channel < 3; channel += 1) {
      color[channel] += weight * record.color[channel];
    }
    alpha += weight;
  }

  const transmittance = 1 - alpha;
  for (let channel = 0; channel < 3; channel += 1) {
    color[channel] += transmittance * background[channel];
  }

  return { color, alpha: 1, transmittance };
}

export function colorDistance(left, right) {
  return Math.hypot(...left.map((value, index) => value - right[index]));
}
