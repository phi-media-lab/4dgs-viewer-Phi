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
}
