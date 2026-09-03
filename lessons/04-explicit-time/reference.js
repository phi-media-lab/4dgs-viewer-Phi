export const PRIMITIVE_FLOATS = 16;
export const PRIMITIVE_BYTES = PRIMITIVE_FLOATS * Float32Array.BYTES_PER_ELEMENT;
export const REFERENCE_TIMES = Object.freeze([0.25, 0.5, 0.75]);

export function evaluatePrimitive(primitive, time) {
  const moving = primitive.moving >= 0.5;
  const deltaTime = time - primitive.timeCenter;
  const duration = Math.max(primitive.duration, 1e-4);
  const normalizedTime = deltaTime / duration;
  const gate = moving ? Math.exp(-0.5 * normalizedTime * normalizedTime) : 1;
  const motionScale = moving ? deltaTime : 0;
  const mean = [
    primitive.mean[0] + primitive.velocity[0] * motionScale,
    primitive.mean[1] + primitive.velocity[1] * motionScale,
  ];
  return {
    mean,
    gate,
    opacity: Math.min(0.999, Math.max(0, primitive.opacity * gate)),
  };
}

export function packPrimitives(primitives) {
  const packed = new Float32Array(primitives.length * PRIMITIVE_FLOATS);
  primitives.forEach((primitive, index) => {
    packed.set([
      ...primitive.mean,
      ...primitive.velocity,
      ...primitive.color,
      1,
      primitive.timeCenter,
      primitive.duration,
      primitive.opacity,
      primitive.moving,
      ...primitive.scale,
      primitive.depth,
      0,
    ], index * PRIMITIVE_FLOATS);
  });
  return packed;
}

export function evaluateScene(primitives, times = REFERENCE_TIMES) {
  return times.flatMap((time) => primitives.map((primitive) => evaluatePrimitive(primitive, time)));
}

export function checkCpuInvariants(primitives, times = REFERENCE_TIMES) {
  const staticPrimitives = primitives.filter((primitive) => primitive.moving < 0.5);
  const movingPrimitives = primitives.filter((primitive) => primitive.moving >= 0.5);
  const firstTime = times[0];
  const lastTime = times[times.length - 1];

  return {
    recordStrideIs64Bytes: PRIMITIVE_BYTES === 64,
    durationsArePositive: primitives.every((primitive) => primitive.duration > 0),
    depthOrderIsNearToFar: primitives.every(
      (primitive, index) => index === 0 || primitives[index - 1].depth <= primitive.depth,
    ),
    staticMeansDoNotMove: staticPrimitives.every((primitive) => {
      const first = evaluatePrimitive(primitive, firstTime).mean;
      const last = evaluatePrimitive(primitive, lastTime).mean;
      return distance(first, primitive.mean) < 1e-12 && distance(last, primitive.mean) < 1e-12;
    }),
    staticGatesStayOpen: staticPrimitives.every((primitive) =>
      times.every((time) => evaluatePrimitive(primitive, time).gate === 1)),
    movingMeansChange: movingPrimitives.every((primitive) =>
      distance(
        evaluatePrimitive(primitive, firstTime).mean,
        evaluatePrimitive(primitive, lastTime).mean,
      ) > 0.05),
    movingGatesAreTimeLocal: movingPrimitives.every((primitive) => {
      const center = evaluatePrimitive(primitive, primitive.timeCenter).gate;
      const left = evaluatePrimitive(primitive, primitive.timeCenter - primitive.duration).gate;
      const right = evaluatePrimitive(primitive, primitive.timeCenter + primitive.duration).gate;
      return center === 1 && Math.abs(left - right) < 1e-12 && left < center;
    }),
  };
}

export function distance(left, right) {
  return Math.hypot(...left.map((value, index) => value - right[index]));
}
