export function covarianceFromScaleRotation(scales, eulerRadians = [0, 0, 0]) {
  const [rx, ry, rz] = eulerRadians;
  const [cx, sx] = [Math.cos(rx), Math.sin(rx)];
  const [cy, sy] = [Math.cos(ry), Math.sin(ry)];
  const [cz, sz] = [Math.cos(rz), Math.sin(rz)];

  // R = Rz * Ry * Rx, stored row-major.
  const rotation = [
    cz * cy,
    cz * sy * sx - sz * cx,
    cz * sy * cx + sz * sx,
    sz * cy,
    sz * sy * sx + cz * cx,
    sz * sy * cx - cz * sx,
    -sy,
    cy * sx,
    cy * cx,
  ];
  const variance = scales.map((scale) => scale * scale);
  const covariance = new Array(9).fill(0);
  for (let row = 0; row < 3; row += 1) {
    for (let column = 0; column < 3; column += 1) {
      for (let axis = 0; axis < 3; axis += 1) {
        covariance[row * 3 + column] += rotation[row * 3 + axis]
          * variance[axis]
          * rotation[column * 3 + axis];
      }
    }
  }
  return covariance;
}

export function projectionJacobian(center, camera) {
  const [x, y, z] = center;
  const [fx, fy] = camera.focal;
  if (!(z > 0)) throw new RangeError('Projection requires camera-space z > 0.');
  return [
    fx / z, 0, -fx * x / (z * z),
    0, fy / z, -fy * y / (z * z),
  ];
}

export function projectGaussian(center, covariance3D, camera) {
  if (covariance3D.length !== 9) throw new TypeError('covariance3D must contain 9 row-major values.');
  const [x, y, z] = center;
  const [fx, fy] = camera.focal;
  const [cx, cy] = camera.principal;
  const jacobian = projectionJacobian(center, camera);
  const [a, , b, , c, d] = jacobian;

  const s00 = covariance3D[0];
  const s01 = covariance3D[1];
  const s02 = covariance3D[2];
  const s11 = covariance3D[4];
  const s12 = covariance3D[5];
  const s22 = covariance3D[8];
  const minimumVariance = camera.minimumVariance ?? 0;
  const xx = a * a * s00 + 2 * a * b * s02 + b * b * s22 + minimumVariance;
  const xy = a * c * s01 + a * d * s02 + b * c * s12 + b * d * s22;
  const yy = c * c * s11 + 2 * c * d * s12 + d * d * s22 + minimumVariance;
  const determinant = xx * yy - xy * xy;
  if (!(determinant > 0) || !Number.isFinite(determinant)) {
    throw new RangeError(`Projected covariance is not positive definite (det=${determinant}).`);
  }

  return {
    mean: [fx * x / z + cx, fy * y / z + cy],
    covariance: [xx, xy, yy],
    conic: [yy / determinant, -xy / determinant, xx / determinant],
    jacobian,
    determinant,
  };
}

export function createProjectionScene(width, height, focalScale = 0.72) {
  const shortSide = Math.min(width, height);
  const focal = focalScale * shortSide;
  const camera = {
    focal: [focal, focal],
    principal: [width * 0.5, height * 0.5],
    minimumVariance: 1,
  };

  const specifications = [
    {
      label: 'near isotropic',
      target: [0.28, 0.3],
      depth: 2,
      scales: [0.16, 0.16, 0.16],
      rotation: [0, 0, 0],
      color: [0.20, 0.66, 1.0],
    },
    {
      label: 'far isotropic',
      target: [0.72, 0.3],
      depth: 5,
      scales: [0.16, 0.16, 0.16],
      rotation: [0, 0, 0],
      color: [0.38, 0.82, 0.48],
    },
    {
      label: 'rotated anisotropic',
      target: [0.28, 0.72],
      depth: 3.1,
      scales: [0.28, 0.07, 0.10],
      rotation: [0, 0, 0.68],
      color: [1.0, 0.62, 0.22],
    },
    {
      label: 'off-axis depth tilt',
      target: [0.72, 0.72],
      depth: 3,
      scales: [0.08, 0.24, 0.06],
      rotation: [0.4, 0.65, -0.45],
      color: [0.85, 0.38, 0.90],
    },
  ];

  const cases = specifications.map((specification) => {
    const targetPixel = [specification.target[0] * width, specification.target[1] * height];
    const center = [
      (targetPixel[0] - camera.principal[0]) * specification.depth / camera.focal[0],
      (targetPixel[1] - camera.principal[1]) * specification.depth / camera.focal[1],
      specification.depth,
    ];
    return {
      label: specification.label,
      center,
      covariance: covarianceFromScaleRotation(specification.scales, specification.rotation),
      opacity: 0.9,
      color: specification.color,
    };
  });

  return { camera, cases };
}

export function projectedValues(projection) {
  return [
    projection.mean[0],
    projection.mean[1],
    projection.covariance[0],
    projection.covariance[1],
    projection.covariance[2],
    projection.conic[0],
    projection.conic[1],
    projection.conic[2],
  ];
}
