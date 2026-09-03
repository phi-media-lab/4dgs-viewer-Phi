struct Camera {
  viewport: vec2f,
  focal: vec2f,
  principal: vec2f,
  minimum_variance: f32,
  case_count: u32,
}

struct Gaussian3D {
  center_opacity: vec4f,
  covariance_row0: vec4f,
  covariance_row1: vec4f,
  covariance_row2: vec4f,
  color: vec4f,
}

struct ProjectedGaussian {
  mean: vec2f,
  covariance: vec3f,
  conic: vec3f,
}

struct ProjectionOutput {
  mean_covariance0: vec4f,
  covariance_conic: vec4f,
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> gaussians: array<Gaussian3D>;
@group(0) @binding(2) var<storage, read_write> projection_output: array<ProjectionOutput>;

fn project_gaussian(gaussian: Gaussian3D) -> ProjectedGaussian {
  let center = gaussian.center_opacity.xyz;
  let inverse_z = 1.0 / center.z;
  let inverse_z_squared = inverse_z * inverse_z;

  // Jacobian of (fx X/Z + cx, fy Y/Z + cy).
  let a = camera.focal.x * inverse_z;
  let b = -camera.focal.x * center.x * inverse_z_squared;
  let c = camera.focal.y * inverse_z;
  let d = -camera.focal.y * center.y * inverse_z_squared;

  let s00 = gaussian.covariance_row0.x;
  let s01 = gaussian.covariance_row0.y;
  let s02 = gaussian.covariance_row0.z;
  let s11 = gaussian.covariance_row1.y;
  let s12 = gaussian.covariance_row1.z;
  let s22 = gaussian.covariance_row2.z;

  // Sigma_2D = J Sigma_3D J^T + minimum_variance I.
  let xx = a * a * s00 + 2.0 * a * b * s02 + b * b * s22
    + camera.minimum_variance;
  let xy = a * c * s01 + a * d * s02 + b * c * s12 + b * d * s22;
  let yy = c * c * s11 + 2.0 * c * d * s12 + d * d * s22
    + camera.minimum_variance;
  let determinant = xx * yy - xy * xy;

  var projected: ProjectedGaussian;
  projected.mean = vec2f(
    camera.focal.x * center.x * inverse_z + camera.principal.x,
    camera.focal.y * center.y * inverse_z + camera.principal.y,
  );
  projected.covariance = vec3f(xx, xy, yy);
  projected.conic = vec3f(yy, -xy, xx) / determinant;
  return projected;
}

struct VertexOutput {
  @builtin(position) position: vec4f,
  @location(0) delta_px: vec2f,
  @location(1) @interpolate(flat) conic: vec3f,
  @location(2) @interpolate(flat) color_opacity: vec4f,
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
fn vs_main(
  @builtin(vertex_index) vertex_index: u32,
  @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
  let gaussian = gaussians[instance_index];
  let projected = project_gaussian(gaussian);
  let corner = QUAD_CORNERS[vertex_index];
  // An axis-aligned 3-sigma bound encloses the rotated ellipse. The fragment
  // conic, not this box, decides the analytic footprint.
  let extent = 3.0 * sqrt(vec2f(projected.covariance.x, projected.covariance.z));
  let delta = corner * extent;
  let pixel = projected.mean + delta;
  let ndc = vec2f(
    2.0 * pixel.x / camera.viewport.x - 1.0,
    1.0 - 2.0 * pixel.y / camera.viewport.y,
  );

  var output: VertexOutput;
  output.position = vec4f(ndc, 0.0, 1.0);
  output.delta_px = delta;
  output.conic = projected.conic;
  output.color_opacity = vec4f(gaussian.color.xyz, gaussian.center_opacity.w);
  return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4f {
  let delta = input.delta_px;
  let conic = input.conic;
  let radius_squared = conic.x * delta.x * delta.x
    + 2.0 * conic.y * delta.x * delta.y
    + conic.z * delta.y * delta.y;
  if (radius_squared > 9.0) {
    discard;
  }
  let alpha = input.color_opacity.w * exp(-0.5 * radius_squared);
  return vec4f(input.color_opacity.xyz * alpha, alpha);
}

@compute @workgroup_size(4)
fn verify_projection(@builtin(global_invocation_id) global_id: vec3u) {
  let index = global_id.x;
  if (index >= camera.case_count) {
    return;
  }
  let projected = project_gaussian(gaussians[index]);
  projection_output[index].mean_covariance0 = vec4f(
    projected.mean,
    projected.covariance.x,
    projected.covariance.y,
  );
  projection_output[index].covariance_conic = vec4f(
    projected.covariance.z,
    projected.conic,
  );
}
