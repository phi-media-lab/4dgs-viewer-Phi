struct Gaussian2D {
  viewport: vec2f,
  center_px: vec2f,
  sigma_px: vec2f,
  rotation: f32,
  opacity: f32,
  verification_sample: vec2f,
  _padding: vec2f,
}

struct Verification {
  alpha: f32,
  exponent: f32,
  radius_squared: f32,
  _padding: f32,
}

@group(0) @binding(0) var<uniform> gaussian: Gaussian2D;
@group(0) @binding(1) var<storage, read_write> verification: Verification;

struct VertexOutput {
  @builtin(position) position: vec4f,
  @location(0) local: vec2f,
}

const QUAD_CORNERS = array<vec2f, 6>(
  vec2f(-1.0, -1.0),
  vec2f( 1.0, -1.0),
  vec2f(-1.0,  1.0),
  vec2f(-1.0,  1.0),
  vec2f( 1.0, -1.0),
  vec2f( 1.0,  1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
  let local = 3.0 * QUAD_CORNERS[vertex_index];
  let cosine = cos(gaussian.rotation);
  let sine = sin(gaussian.rotation);
  let axis_x = vec2f(cosine, sine) * gaussian.sigma_px.x;
  let axis_y = vec2f(-sine, cosine) * gaussian.sigma_px.y;
  let pixel = gaussian.center_px + axis_x * local.x + axis_y * local.y;
  let ndc = vec2f(
    2.0 * pixel.x / gaussian.viewport.x - 1.0,
    1.0 - 2.0 * pixel.y / gaussian.viewport.y,
  );

  var output: VertexOutput;
  output.position = vec4f(ndc, 0.0, 1.0);
  output.local = local;
  return output;
}

fn analytic_alpha(local: vec2f) -> f32 {
  return gaussian.opacity * exp(-0.5 * dot(local, local));
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4f {
  let radius_squared = dot(input.local, input.local);
  if (radius_squared > 9.0) {
    discard;
  }
  let alpha = analytic_alpha(input.local);
  let color = vec3f(0.18, 0.58, 1.0);
  return vec4f(color * alpha, alpha);
}

@compute @workgroup_size(1)
fn verify_gaussian() {
  let sample = gaussian.verification_sample;
  let radius_squared = dot(sample, sample);
  let exponent = -0.5 * radius_squared;
  verification.alpha = gaussian.opacity * exp(exponent);
  verification.exponent = exponent;
  verification.radius_squared = radius_squared;
}
