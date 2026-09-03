export const GAUSSIAN_SAMPLE = Object.freeze([1, 0.5]);

export function gaussianWeight(local, opacity = 1) {
  const radiusSquared = local[0] * local[0] + local[1] * local[1];
  return opacity * Math.exp(-0.5 * radiusSquared);
}

export function covariance2D(sigma, rotationRadians) {
  const [sigmaX, sigmaY] = sigma;
  const cosine = Math.cos(rotationRadians);
  const sine = Math.sin(rotationRadians);
  const xx = cosine * cosine * sigmaX * sigmaX
    + sine * sine * sigmaY * sigmaY;
  const xy = cosine * sine * (sigmaX * sigmaX - sigmaY * sigmaY);
  const yy = sine * sine * sigmaX * sigmaX
    + cosine * cosine * sigmaY * sigmaY;
  return [xx, xy, yy];
}

export function ellipsePoint(center, sigma, rotationRadians, local) {
  const cosine = Math.cos(rotationRadians);
  const sine = Math.sin(rotationRadians);
  const scaledX = sigma[0] * local[0];
  const scaledY = sigma[1] * local[1];
  return [
    center[0] + cosine * scaledX - sine * scaledY,
    center[1] + sine * scaledX + cosine * scaledY,
  ];
}
