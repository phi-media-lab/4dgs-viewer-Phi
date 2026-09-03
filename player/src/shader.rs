use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const RENDERER_SHADER_FILES: [&str; 9] = [
    "preprocess.wgsl",
    "build-indirect.wgsl",
    "radix-histogram.wgsl",
    "radix-scatter.wgsl",
    "count-equal-depth.wgsl",
    "tile-bin.wgsl",
    "tile-render.wgsl",
    "splat.wgsl",
    "resolve.wgsl",
];

pub fn load_with_includes(path: &Path) -> Result<String> {
    expand(path, &mut BTreeSet::new())
}

pub fn renderer_bundle_sha256(dir: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    for name in RENDERER_SHADER_FILES {
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(load_with_includes(&dir.join(name))?.as_bytes());
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn expand(path: &Path, stack: &mut BTreeSet<PathBuf>) -> Result<String> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolve shader {}", path.display()))?;
    if !stack.insert(canonical.clone()) {
        bail!("circular WGSL include at {}", canonical.display());
    }
    let source = fs::read_to_string(&canonical)
        .with_context(|| format!("read shader {}", canonical.display()))?;
    let mut output = String::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#include \"") {
            let include = rest
                .strip_suffix('"')
                .with_context(|| format!("malformed include in {}: {line}", canonical.display()))?;
            let include_path = canonical.parent().unwrap().join(include);
            output.push_str(&expand(&include_path, stack)?);
            output.push('\n');
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    stack.remove(&canonical);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::naga::{
        front::wgsl::parse_str,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct PixelState {
        color: [f32; 3],
        transmittance: f32,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct RasterPolicy {
        alpha_cap: f32,
        pixel_alpha_min: f32,
        transmittance_epsilon: f32,
        explicit: bool,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum CompositeOutcome {
        Skipped,
        Contributed,
        TerminatedBeforeContribution,
        ContributedAndTerminated,
    }

    /// Independent CPU oracle for the ordering promised by the raster ABI.
    ///
    /// This deliberately models one candidate rather than parsing WGSL text:
    /// tests below execute the numeric boundaries that distinguish gsplat
    /// classic from Phi's legacy compositing behavior.
    fn composite_reference(
        state: &mut PixelState,
        gaussian_color: [f32; 3],
        gaussian_opacity: f32,
        exponent: f32,
        policy: RasterPolicy,
    ) -> CompositeOutcome {
        if exponent > 0.0 {
            return CompositeOutcome::Skipped;
        }
        let raw_alpha = gaussian_opacity * exponent.exp();
        let alpha = if raw_alpha.is_nan() {
            raw_alpha
        } else {
            policy.alpha_cap.min(raw_alpha)
        };
        if alpha < policy.pixel_alpha_min || alpha.is_nan() {
            return CompositeOutcome::Skipped;
        }

        let next_transmittance = state.transmittance * (1.0 - alpha);
        if policy.explicit && next_transmittance <= policy.transmittance_epsilon {
            return CompositeOutcome::TerminatedBeforeContribution;
        }

        for (accum, channel) in state.color.iter_mut().zip(gaussian_color) {
            *accum += state.transmittance * channel * alpha;
        }
        state.transmittance = next_transmittance;
        if !policy.explicit && state.transmittance < policy.transmittance_epsilon {
            CompositeOutcome::ContributedAndTerminated
        } else {
            CompositeOutcome::Contributed
        }
    }

    fn projected_radii_reference(projected: [f32; 2], explicit: bool) -> [f32; 2] {
        if explicit {
            projected
        } else {
            projected.map(|radius| radius.min(2048.0))
        }
    }

    fn assert_approx_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn native_and_browser_tile_entry_points_are_valid_wgsl() {
        let shader_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
        for name in RENDERER_SHADER_FILES {
            let source = load_with_includes(&shader_dir.join(name)).unwrap();
            let module = parse_str(&source)
                .unwrap_or_else(|error| panic!("parse {name}: {}", error.emit_to_string(&source)));
            Validator::new(ValidationFlags::all(), Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|error| panic!("validate {name}: {error:?}"));
        }
    }

    #[test]
    fn explicit_classic_policy_executes_cutoff_and_pre_contribution_termination_order() {
        let policy = RasterPolicy {
            alpha_cap: 0.999,
            pixel_alpha_min: 1.0 / 255.0,
            transmittance_epsilon: 1.0e-4,
            explicit: true,
        };

        // A below-cutoff candidate is skipped before its next-T value can
        // terminate the pixel. The chosen T makes next-T fall below epsilon.
        let mut cutoff_state = PixelState {
            color: [0.25, 0.5, 0.75],
            transmittance: 0.000_100_1,
        };
        let before_cutoff = cutoff_state;
        assert_eq!(
            composite_reference(&mut cutoff_state, [8.0, 8.0, 8.0], 0.001, 0.0, policy),
            CompositeOutcome::Skipped
        );
        assert_eq!(cutoff_state, before_cutoff);

        let mut nan_state = before_cutoff;
        assert_eq!(
            composite_reference(&mut nan_state, [8.0, 8.0, 8.0], f32::NAN, 0.0, policy),
            CompositeOutcome::Skipped
        );
        assert_eq!(nan_state, before_cutoff);

        // A raw alpha above one is capped before next-T and contribution.
        let mut cap_state = PixelState {
            color: [0.0; 3],
            transmittance: 0.5,
        };
        assert_eq!(
            composite_reference(&mut cap_state, [1.0; 3], 2.0, 0.0, policy),
            CompositeOutcome::Contributed
        );
        assert_approx_eq(cap_state.color[0], 0.4995);
        assert_approx_eq(cap_state.transmittance, 0.0005);

        // Equality at the explicit next-T boundary terminates without adding
        // the candidate and without mutating either accumulated color or T.
        let mut terminal_state = PixelState {
            color: [0.25, 0.5, 0.75],
            transmittance: 0.0002,
        };
        let before_terminal = terminal_state;
        assert_eq!(
            composite_reference(&mut terminal_state, [8.0, 8.0, 8.0], 0.5, 0.0, policy),
            CompositeOutcome::TerminatedBeforeContribution
        );
        assert_eq!(terminal_state, before_terminal);

        let mut contributing_state = PixelState {
            color: [1.0, 1.0, 1.0],
            transmittance: 0.5,
        };
        assert_eq!(
            composite_reference(&mut contributing_state, [2.0, 4.0, 6.0], 0.5, 0.0, policy,),
            CompositeOutcome::Contributed
        );
        assert_approx_eq(contributing_state.color[0], 1.5);
        assert_approx_eq(contributing_state.color[1], 2.0);
        assert_approx_eq(contributing_state.color[2], 2.5);
        assert_approx_eq(contributing_state.transmittance, 0.25);
    }

    #[test]
    fn omitted_policy_executes_legacy_post_contribution_termination_order() {
        let policy = RasterPolicy {
            alpha_cap: 0.99,
            pixel_alpha_min: 1.0 / 255.0,
            transmittance_epsilon: 0.01,
            explicit: false,
        };
        let mut state = PixelState {
            color: [0.0; 3],
            transmittance: 0.5,
        };
        assert_eq!(
            composite_reference(&mut state, [1.0, 2.0, 3.0], 2.0, 0.0, policy),
            CompositeOutcome::ContributedAndTerminated
        );
        assert_approx_eq(state.color[0], 0.495);
        assert_approx_eq(state.color[1], 0.99);
        assert_approx_eq(state.color[2], 1.485);
        assert_approx_eq(state.transmittance, 0.005);

        // Legacy uses a strict post-update comparison: equality does not stop.
        let mut equality_state = PixelState {
            color: [0.0; 3],
            transmittance: 0.02,
        };
        assert_eq!(
            composite_reference(&mut equality_state, [1.0; 3], 0.5, 0.0, policy),
            CompositeOutcome::Contributed
        );
        assert_approx_eq(equality_state.transmittance, 0.01);
    }

    #[test]
    fn explicit_policy_has_no_hidden_projected_radius_cap() {
        let projected = [4096.0, 1024.0];
        assert_eq!(projected_radii_reference(projected, true), projected);
        assert_eq!(
            projected_radii_reference(projected, false),
            [2048.0, 1024.0]
        );
    }
}
