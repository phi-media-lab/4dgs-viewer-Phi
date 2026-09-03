export function evaluateReference(records, manifest, { time, aspect }) {
  const projected = records.map((record, sourceIndex) => (
    projectRecord(record, sourceIndex, manifest, time, aspect)
  ));
  const sorted = [...projected].sort(compareProjected);
  return {
    projected,
    sorted,
    activeCount: projected.filter((record) => record.active).length,
    visibleCount: projected.filter((record) => record.valid).length,
  };
}

export function projectRecord(record, sourceIndex, manifest, time, aspect) {
  const { camera, render } = manifest;
  const deltaTime = time - record.timeCenter;
  const center = record.center.map((value, axis) => value + record.velocity[axis] * deltaTime);
  const normalizedTime = deltaTime / record.timeSigma;
  const opacity = record.opacity * Math.exp(-0.5 * normalizedTime * normalizedTime);
  const depth = center[2];
  const invalid = {
    sourceIndex,
    active: false,
    valid: false,
    mean: [0, 0],
    extent: [0, 0],
    conic: [1, 0, 1],
    opacity: 0,
    color: record.color,
    depth,
  };
  if (opacity < render.alphaMin || depth <= camera.near || depth >= camera.far) return invalid;

  const fx = camera.focalY / aspect;
  const fy = camera.focalY;
  const inverseDepth = 1 / depth;
  const mean = [fx * center[0] * inverseDepth, fy * center[1] * inverseDepth];
  const jx = fx * inverseDepth;
  const jy = fy * inverseDepth;
  const jxz = -fx * center[0] * inverseDepth * inverseDepth;
  const jyz = -fy * center[1] * inverseDepth * inverseDepth;
  const variance = record.scale.map((value) => value * value);
  const minimumVariance = camera.minSigmaNdc * camera.minSigmaNdc;
  const cxx = jx * jx * variance[0] + jxz * jxz * variance[2] + minimumVariance;
  const cyy = jy * jy * variance[1] + jyz * jyz * variance[2] + minimumVariance;
  const cxy = jxz * jyz * variance[2];
  const determinant = cxx * cyy - cxy * cxy;
  if (!(determinant > 0)) return { ...invalid, active: true };

  const extent = [3 * Math.sqrt(cxx), 3 * Math.sqrt(cyy)];
  const conic = [cyy / determinant, -cxy / determinant, cxx / determinant];
  const visible = Math.abs(mean[0]) <= 1 + extent[0] && Math.abs(mean[1]) <= 1 + extent[1];
  return {
    sourceIndex,
    active: true,
    valid: visible,
    mean,
    extent,
    conic,
    opacity: visible ? opacity : 0,
    color: record.color,
    depth,
  };
}

export function compositeAtNdc(projectedInOrder, point, alphaFloor) {
  let color = [0, 0, 0];
  let accumulatedAlpha = 0;
  for (const projected of projectedInOrder) {
    if (!projected.valid) continue;
    const dx = point[0] - projected.mean[0];
    const dy = point[1] - projected.mean[1];
    const [a, b, c] = projected.conic;
    const exponent = a * dx * dx + 2 * b * dx * dy + c * dy * dy;
    if (exponent > 9) continue;
    const alpha = projected.opacity * Math.exp(-0.5 * exponent);
    if (alpha < alphaFloor) continue;
    color = color.map((channel, index) => projected.color[index] * alpha + channel * (1 - alpha));
    accumulatedAlpha = alpha + accumulatedAlpha * (1 - alpha);
  }
  return { color, alpha: accumulatedAlpha };
}

export function colorDistance(left, right) {
  return Math.hypot(...left.map((value, index) => value - right[index]));
}

function compareProjected(left, right) {
  if (left.valid !== right.valid) return left.valid ? -1 : 1;
  if (left.valid && left.depth !== right.depth) return right.depth - left.depth;
  return left.sourceIndex - right.sourceIndex;
}
