use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"4DGSWG01";
const HEADER_BYTES: usize = 64;
const RECORD_BYTES: usize = 80;
const SH3_COEFFICIENTS: usize = 15;
const SH3_CHANNELS: usize = 3;
const SH3_RECORD_BYTES: usize = 92;
const SHADER_SAFE_ABS: f32 = 1.0e30;
const CAMERA_ORTHONORMAL_TOLERANCE: f64 = 1.0e-3;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryDescription {
    pub uri: String,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceDescription {
    pub uri: String,
    pub bytes: usize,
    pub sha256: String,
    pub degree: u32,
    pub coefficients: usize,
    pub channels: usize,
    pub encoding: String,
    pub record_stride: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimeDescription {
    pub domain: [f32; 2],
    pub max_duration: f32,
    pub units: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Representation {
    pub velocity: String,
    pub rotation: String,
    pub scale: String,
    pub opacity: String,
    pub gate: String,
    pub duration: String,
    pub color: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub temporal_threshold: f32,
    pub alpha_min: f32,
    pub low_pass: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RenderDescription {
    pub working_space: String,
    pub background: [f32; 4],
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixedCamera {
    pub world_to_camera_row_major: [[f32; 4]; 4],
    pub intrinsics: [f32; 4],
    pub source_size: [u32; 2],
    pub near: f32,
    pub far: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CameraDescription {
    pub fixed: FixedCamera,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub version: u32,
    pub name: String,
    pub gaussian_count: u32,
    pub record_stride: usize,
    pub binary: BinaryDescription,
    pub time: TimeDescription,
    pub representation: Representation,
    pub policy: Policy,
    pub render: RenderDescription,
    pub camera: CameraDescription,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<AppearanceDescription>,
    pub provenance: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
pub struct Asset {
    pub manifest_sha256: String,
    pub manifest: Manifest,
    pub records: Vec<u8>,
    pub sha256: String,
    pub sh_coefficients: Option<Vec<u8>>,
    pub appearance_sha256: Option<String>,
}

impl Asset {
    pub fn load(path: &Path) -> Result<Self> {
        let manifest_bytes =
            fs::read(path).with_context(|| format!("read manifest {}", path.display()))?;
        let manifest_sha256 = format!("{:x}", Sha256::digest(&manifest_bytes));
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
            .with_context(|| format!("parse manifest {}", path.display()))?;
        validate_manifest(&manifest)?;

        let binary_path = resolve_payload(path, &manifest.binary.uri, "Gaussian binary")?;
        let binary = fs::read(&binary_path)
            .with_context(|| format!("read Gaussian binary {}", binary_path.display()))?;
        ensure!(
            binary.len() == manifest.binary.bytes,
            "binary length mismatch: manifest {}, file {}",
            manifest.binary.bytes,
            binary.len()
        );
        let sha256 = format!("{:x}", Sha256::digest(&binary));
        ensure!(
            sha256 == manifest.binary.sha256,
            "binary SHA-256 mismatch: manifest {}, actual {}",
            manifest.binary.sha256,
            sha256
        );
        validate_binary(&binary, &manifest)?;
        validate_records(&binary[HEADER_BYTES..], manifest.gaussian_count)?;

        let (sh_coefficients, appearance_sha256) = if manifest.representation.color == "raw-sh3" {
            let description = manifest
                .appearance
                .as_ref()
                .context("raw-sh3 manifest is missing its validated appearance descriptor")?;
            let appearance_path = resolve_payload(path, &description.uri, "SH3 appearance binary")?;
            let appearance = fs::read(&appearance_path).with_context(|| {
                format!("read SH3 appearance binary {}", appearance_path.display())
            })?;
            ensure!(
                appearance.len() == description.bytes,
                "SH3 appearance length mismatch: manifest {}, file {}",
                description.bytes,
                appearance.len()
            );
            let appearance_sha256 = format!("{:x}", Sha256::digest(&appearance));
            ensure!(
                appearance_sha256 == description.sha256,
                "SH3 appearance SHA-256 mismatch: manifest {}, actual {}",
                description.sha256,
                appearance_sha256
            );
            validate_sh3_records(&appearance, manifest.gaussian_count)?;
            (Some(appearance), Some(appearance_sha256))
        } else {
            (None, None)
        };

        Ok(Self {
            manifest_sha256,
            records: binary[HEADER_BYTES..].to_vec(),
            manifest,
            sha256,
            sh_coefficients,
            appearance_sha256,
        })
    }

    pub fn sh_degree(&self) -> u32 {
        match self.manifest.representation.color.as_str() {
            "raw-sh0" => 0,
            "raw-sh3" => 3,
            _ => unreachable!("validated color representation"),
        }
    }
}

fn resolve_payload(manifest_path: &Path, uri: &str, label: &str) -> Result<PathBuf> {
    ensure!(!uri.is_empty(), "{label} URI must not be empty");
    let uri_path = Path::new(uri);
    ensure!(uri_path.is_relative(), "{label} URI must be relative");

    let asset_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_root = fs::canonicalize(asset_dir)
        .with_context(|| format!("resolve asset directory {}", asset_dir.display()))?;
    let candidate = asset_dir.join(uri_path);
    let canonical_candidate = fs::canonicalize(&candidate)
        .with_context(|| format!("resolve {label} {}", candidate.display()))?;
    ensure!(
        canonical_candidate.starts_with(&canonical_root),
        "{label} URI escapes the asset directory"
    );
    Ok(canonical_candidate)
}

fn validate_records(records: &[u8], count: u32) -> Result<()> {
    for (index, record) in records.chunks_exact(RECORD_BYTES).enumerate() {
        let mut quaternion_norm = 0.0_f32;
        for component in 0..20 {
            let offset = component * 4;
            let value = f32::from_le_bytes(record[offset..offset + 4].try_into().unwrap());
            ensure!(
                value.is_finite() && value.abs() < SHADER_SAFE_ABS,
                "Gaussian {index} component {component} is not finite or exceeds the shader-safe magnitude"
            );
            if (8..12).contains(&component) {
                quaternion_norm += value * value;
            }
        }
        ensure!(
            quaternion_norm.is_finite() && quaternion_norm > 1e-12,
            "Gaussian {index} has an invalid quaternion norm"
        );
    }
    ensure!(
        records.len() == count as usize * RECORD_BYTES,
        "record validation did not cover the declared Gaussian count"
    );
    Ok(())
}

fn validate_sh3_records(records: &[u8], count: u32) -> Result<()> {
    let expected_bytes = count as usize * SH3_RECORD_BYTES;
    ensure!(
        records.len() == expected_bytes,
        "SH3 appearance byte count {} does not match {} records",
        records.len(),
        count
    );
    for (index, record) in records.chunks_exact(SH3_RECORD_BYTES).enumerate() {
        for coefficient in 0..SH3_COEFFICIENTS * SH3_CHANNELS {
            let offset = coefficient * 2;
            let bits = u16::from_le_bytes(record[offset..offset + 2].try_into().unwrap());
            ensure!(
                bits & 0x7c00 != 0x7c00,
                "SH3 appearance record {index} coefficient {coefficient} is not finite"
            );
        }
        ensure!(
            u16::from_le_bytes(record[90..92].try_into().unwrap()) == 0,
            "SH3 appearance record {index} reserved padding must be zero"
        );
    }
    Ok(())
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    ensure!(
        manifest.schema == "phi.4dgs.explicit.v1",
        "unsupported schema {}",
        manifest.schema
    );
    ensure!(
        manifest.version == 1,
        "unsupported manifest version {}",
        manifest.version
    );
    ensure!(
        manifest.record_stride == RECORD_BYTES,
        "record_stride must be {RECORD_BYTES}"
    );
    ensure!(
        !manifest.name.trim().is_empty(),
        "asset name must not be empty"
    );
    ensure!(
        manifest.gaussian_count > 0,
        "gaussian_count must be positive"
    );
    let expected_binary_bytes = (manifest.gaussian_count as usize)
        .checked_mul(RECORD_BYTES)
        .and_then(|bytes| bytes.checked_add(HEADER_BYTES))
        .context("Gaussian byte count overflows usize")?;
    ensure!(
        manifest.binary.bytes == expected_binary_bytes,
        "binary byte count does not match gaussian_count"
    );
    ensure!(
        manifest.time.domain == [0.0, 1.0],
        "only normalized time [0, 1] is supported"
    );
    ensure!(
        manifest.time.units == "normalized"
            && manifest.time.max_duration.is_finite()
            && manifest.time.max_duration > 0.0
            && manifest.time.max_duration < SHADER_SAFE_ABS,
        "time must use normalized units and a positive finite max_duration"
    );
    ensure!(
        manifest.representation.velocity == "explicit-linear",
        "only explicit-linear velocity is supported"
    );
    ensure!(
        manifest.representation.rotation == "raw-xyzw"
            && manifest.representation.scale == "raw-log"
            && manifest.representation.opacity == "raw-logit"
            && manifest.representation.gate == "raw-logit-times-20"
            && manifest.representation.duration == "raw-logit-max-duration-over-6",
        "unsupported explicit Gaussian representation"
    );
    ensure!(
        matches!(
            manifest.representation.color.as_str(),
            "raw-sh0" | "raw-sh3"
        ),
        "only raw-sh0/raw-sh3 color is supported"
    );
    ensure!(
        manifest.render.working_space == "display-srgb",
        "only display-srgb is supported"
    );
    ensure!(
        manifest.policy.temporal_threshold.is_finite()
            && (0.0..=1.0).contains(&manifest.policy.temporal_threshold),
        "temporal_threshold must be finite and in [0, 1]"
    );
    ensure!(
        manifest.policy.alpha_min.is_finite()
            && manifest.policy.alpha_min > 0.0
            && manifest.policy.alpha_min < 1.0,
        "alpha_min must be finite and in (0, 1)"
    );
    ensure!(
        manifest.policy.low_pass.is_finite()
            && manifest.policy.low_pass >= 0.0
            && manifest.policy.low_pass < SHADER_SAFE_ABS,
        "low_pass must be finite and non-negative"
    );
    ensure!(
        manifest
            .render
            .background
            .iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value)),
        "render background must contain finite values in [0, 1]"
    );
    ensure!(
        manifest.render.background[3] == 1.0,
        "render background alpha must be exactly 1 for the opaque output contract"
    );
    let camera = &manifest.camera.fixed;
    ensure!(
        camera
            .world_to_camera_row_major
            .iter()
            .flatten()
            .chain(camera.intrinsics.iter())
            .all(|value| value.is_finite() && value.abs() < SHADER_SAFE_ABS),
        "camera transform and intrinsics must be shader-safe finite values"
    );
    ensure!(
        camera.intrinsics[0] > 0.0
            && camera.intrinsics[1] > 0.0
            && camera.source_size[0] > 0
            && camera.source_size[1] > 0
            && camera.near.is_finite()
            && camera.far.is_finite()
            && camera.near > 0.0
            && camera.far < SHADER_SAFE_ABS
            && camera.far > camera.near,
        "camera focal lengths, source size, and clipping range are invalid"
    );
    ensure!(
        rigid_world_to_camera(&camera.world_to_camera_row_major),
        "world_to_camera_row_major must be a right-handed rigid affine transform"
    );
    ensure!(
        manifest.binary.sha256.len() == 64
            && manifest
                .binary
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "binary sha256 is malformed"
    );
    match manifest.representation.color.as_str() {
        "raw-sh0" => ensure!(
            manifest.appearance.is_none(),
            "raw-sh0 diagnostic assets must not declare an SH3 appearance sidecar"
        ),
        "raw-sh3" => {
            let appearance = manifest
                .appearance
                .as_ref()
                .context("raw-sh3 requires an appearance sidecar")?;
            ensure!(
                !appearance.uri.is_empty()
                    && appearance.degree == 3
                    && appearance.coefficients == SH3_COEFFICIENTS
                    && appearance.channels == SH3_CHANNELS
                    && appearance.encoding == "float16-le-padded46"
                    && appearance.record_stride == SH3_RECORD_BYTES
                    && appearance.bytes == manifest.gaussian_count as usize * SH3_RECORD_BYTES
                    && appearance.sha256.len() == 64
                    && appearance
                        .sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "raw-sh3 requires the versioned float16 appearance sidecar contract"
            );
        }
        _ => unreachable!("validated color representation"),
    }
    Ok(())
}

fn rigid_world_to_camera(rows: &[[f32; 4]; 4]) -> bool {
    let target = [0.0_f64, 0.0, 0.0, 1.0];
    if rows[3]
        .iter()
        .zip(target)
        .any(|(actual, expected)| (*actual as f64 - expected).abs() > CAMERA_ORTHONORMAL_TOLERANCE)
    {
        return false;
    }
    for row in 0..3 {
        for other in 0..3 {
            let dot = (0..3)
                .map(|column| rows[row][column] as f64 * rows[other][column] as f64)
                .sum::<f64>();
            let expected = if row == other { 1.0 } else { 0.0 };
            if (dot - expected).abs() > CAMERA_ORTHONORMAL_TOLERANCE {
                return false;
            }
        }
    }
    let r = rows;
    let determinant = r[0][0] as f64
        * (r[1][1] as f64 * r[2][2] as f64 - r[1][2] as f64 * r[2][1] as f64)
        - r[0][1] as f64 * (r[1][0] as f64 * r[2][2] as f64 - r[1][2] as f64 * r[2][0] as f64)
        + r[0][2] as f64 * (r[1][0] as f64 * r[2][1] as f64 - r[1][1] as f64 * r[2][0] as f64);
    (determinant - 1.0).abs() <= CAMERA_ORTHONORMAL_TOLERANCE
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn validate_binary(binary: &[u8], manifest: &Manifest) -> Result<()> {
    ensure!(
        binary.len() >= HEADER_BYTES,
        "binary shorter than {HEADER_BYTES}-byte header"
    );
    ensure!(&binary[0..8] == MAGIC, "bad Gaussian binary magic");
    ensure!(read_u32(binary, 8) == 1, "unsupported binary version");
    ensure!(
        read_u32(binary, 12) as usize == HEADER_BYTES,
        "invalid header size"
    );
    ensure!(
        read_u32(binary, 16) == manifest.gaussian_count,
        "binary count differs from manifest"
    );
    ensure!(
        read_u32(binary, 20) as usize == RECORD_BYTES,
        "invalid record stride"
    );
    ensure!(
        read_u64(binary, 24) as usize == HEADER_BYTES,
        "invalid record offset"
    );
    let record_bytes = read_u64(binary, 32) as usize;
    ensure!(
        record_bytes == manifest.gaussian_count as usize * RECORD_BYTES,
        "inconsistent record byte count"
    );
    ensure!(
        HEADER_BYTES + record_bytes == binary.len(),
        "binary has missing or trailing bytes"
    );
    for offset in (40..64).step_by(8) {
        ensure!(
            read_u64(binary, offset) == 0,
            "reserved header words must be zero"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use sha2::{Digest, Sha256};

    use super::{
        AppearanceDescription, Asset, BinaryDescription, CameraDescription, FixedCamera, Manifest,
        Policy, RECORD_BYTES, RenderDescription, Representation, SH3_RECORD_BYTES, TimeDescription,
        validate_manifest, validate_records, validate_sh3_records,
    };

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "phi-native-raw-sh3-{}-{}",
                std::process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn record_with_unit_quaternion() -> Vec<u8> {
        let mut record = vec![0_u8; RECORD_BYTES];
        record[8 * 4..9 * 4].copy_from_slice(&1.0_f32.to_le_bytes());
        record
    }

    fn binary_with_unit_quaternion() -> Vec<u8> {
        let mut binary = vec![0_u8; 64];
        binary[0..8].copy_from_slice(b"4DGSWG01");
        binary[8..12].copy_from_slice(&1_u32.to_le_bytes());
        binary[12..16].copy_from_slice(&64_u32.to_le_bytes());
        binary[16..20].copy_from_slice(&1_u32.to_le_bytes());
        binary[20..24].copy_from_slice(&(RECORD_BYTES as u32).to_le_bytes());
        binary[24..32].copy_from_slice(&64_u64.to_le_bytes());
        binary[32..40].copy_from_slice(&(RECORD_BYTES as u64).to_le_bytes());
        binary.extend(record_with_unit_quaternion());
        binary
    }

    fn sh3_description(bytes: &[u8]) -> AppearanceDescription {
        AppearanceDescription {
            uri: "sh3.f16".into(),
            bytes: bytes.len(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            degree: 3,
            coefficients: 15,
            channels: 3,
            encoding: "float16-le-padded46".into(),
            record_stride: SH3_RECORD_BYTES,
        }
    }

    fn manifest(binary: &[u8], color: &str, appearance: Option<AppearanceDescription>) -> Manifest {
        Manifest {
            schema: "phi.4dgs.explicit.v1".into(),
            version: 1,
            name: "test".into(),
            gaussian_count: 1,
            record_stride: RECORD_BYTES,
            binary: BinaryDescription {
                uri: "gaussians.bin".into(),
                bytes: binary.len(),
                sha256: format!("{:x}", Sha256::digest(binary)),
            },
            time: TimeDescription {
                domain: [0.0, 1.0],
                max_duration: 1.0,
                units: "normalized".into(),
            },
            representation: Representation {
                velocity: "explicit-linear".into(),
                rotation: "raw-xyzw".into(),
                scale: "raw-log".into(),
                opacity: "raw-logit".into(),
                gate: "raw-logit-times-20".into(),
                duration: "raw-logit-max-duration-over-6".into(),
                color: color.into(),
            },
            policy: Policy {
                temporal_threshold: 0.002,
                alpha_min: 1.0 / 255.0,
                low_pass: 0.3,
            },
            render: RenderDescription {
                working_space: "display-srgb".into(),
                background: [0.0, 0.0, 0.0, 1.0],
            },
            camera: CameraDescription {
                fixed: FixedCamera {
                    world_to_camera_row_major: [
                        [1.0, 0.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                        [0.0, 0.0, 0.0, 1.0],
                    ],
                    intrinsics: [1.0, 1.0, 0.5, 0.5],
                    source_size: [1, 1],
                    near: 0.01,
                    far: 100.0,
                },
            },
            appearance,
            provenance: serde_json::Map::new(),
        }
    }

    #[test]
    fn record_validation_accepts_finite_unit_quaternion() {
        validate_records(&record_with_unit_quaternion(), 1).unwrap();
    }

    #[test]
    fn record_validation_rejects_zero_quaternion() {
        let error = validate_records(&[0_u8; RECORD_BYTES], 1).unwrap_err();
        assert!(error.to_string().contains("invalid quaternion norm"));
    }

    #[test]
    fn record_validation_rejects_non_finite_component() {
        let mut record = record_with_unit_quaternion();
        record[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
        let error = validate_records(&record, 1).unwrap_err();
        assert!(error.to_string().contains("not finite"));
    }

    #[test]
    fn record_validation_rejects_shader_unsafe_magnitude() {
        let mut record = record_with_unit_quaternion();
        record[4..8].copy_from_slice(&1.0e30_f32.to_le_bytes());
        let error = validate_records(&record, 1).unwrap_err();
        assert!(error.to_string().contains("shader-safe magnitude"));
    }

    #[test]
    fn manifest_deserialization_rejects_unknown_fields_and_non_object_provenance() {
        let binary = binary_with_unit_quaternion();
        let value = serde_json::to_value(manifest(&binary, "raw-sh0", None)).unwrap();

        let mut unknown = value.clone();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), true.into());
        assert!(serde_json::from_value::<Manifest>(unknown).is_err());

        let mut null_provenance = value;
        null_provenance
            .as_object_mut()
            .unwrap()
            .insert("provenance".into(), serde_json::Value::Null);
        assert!(serde_json::from_value::<Manifest>(null_provenance).is_err());
    }

    #[test]
    fn manifest_rejects_out_of_range_background_and_non_rigid_camera() {
        let binary = binary_with_unit_quaternion();
        let mut invalid_background = manifest(&binary, "raw-sh0", None);
        invalid_background.render.background[0] = 1.01;
        assert!(
            validate_manifest(&invalid_background)
                .unwrap_err()
                .to_string()
                .contains("[0, 1]")
        );

        let mut non_rigid = manifest(&binary, "raw-sh0", None);
        non_rigid.camera.fixed.world_to_camera_row_major[0][0] = 2.0;
        assert!(
            validate_manifest(&non_rigid)
                .unwrap_err()
                .to_string()
                .contains("rigid affine")
        );
    }

    #[test]
    fn raw_sh3_manifest_requires_versioned_sidecar() {
        let binary = binary_with_unit_quaternion();
        let error = validate_manifest(&manifest(&binary, "raw-sh3", None)).unwrap_err();
        assert!(error.to_string().contains("requires an appearance sidecar"));
    }

    #[test]
    fn raw_sh0_diagnostic_manifest_remains_supported() {
        let binary = binary_with_unit_quaternion();
        validate_manifest(&manifest(&binary, "raw-sh0", None)).unwrap();
    }

    #[test]
    fn asset_loads_and_verifies_raw_sh3_coefficients() {
        let directory = TestDir::new();
        let binary = binary_with_unit_quaternion();
        let coefficients = vec![0_u8; SH3_RECORD_BYTES];
        let manifest = manifest(&binary, "raw-sh3", Some(sh3_description(&coefficients)));
        fs::write(directory.path().join("gaussians.bin"), &binary).unwrap();
        fs::write(directory.path().join("sh3.f16"), &coefficients).unwrap();
        fs::write(
            directory.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let asset = Asset::load(&directory.path().join("manifest.json")).unwrap();
        assert_eq!(asset.sh_degree(), 3);
        assert_eq!(
            asset.sh_coefficients.as_deref(),
            Some(coefficients.as_slice())
        );
        assert_eq!(
            asset.appearance_sha256.as_deref(),
            manifest
                .appearance
                .as_ref()
                .map(|value| value.sha256.as_str())
        );
    }

    #[test]
    fn asset_rejects_corrupted_raw_sh3_sidecar() {
        let directory = TestDir::new();
        let binary = binary_with_unit_quaternion();
        let coefficients = vec![0_u8; SH3_RECORD_BYTES];
        let manifest = manifest(&binary, "raw-sh3", Some(sh3_description(&coefficients)));
        let mut corrupted = coefficients;
        corrupted[0] = 1;
        fs::write(directory.path().join("gaussians.bin"), &binary).unwrap();
        fs::write(directory.path().join("sh3.f16"), corrupted).unwrap();
        fs::write(
            directory.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = Asset::load(&directory.path().join("manifest.json")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("SH3 appearance SHA-256 mismatch")
        );
    }

    #[test]
    fn asset_rejects_payload_outside_manifest_directory() {
        let directory = TestDir::new();
        let asset_directory = directory.path().join("asset");
        fs::create_dir(&asset_directory).unwrap();
        let binary = binary_with_unit_quaternion();
        fs::write(directory.path().join("outside.bin"), &binary).unwrap();
        let mut manifest = manifest(&binary, "raw-sh0", None);
        manifest.binary.uri = "../outside.bin".into();
        fs::write(
            asset_directory.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let error = Asset::load(&asset_directory.join("manifest.json")).unwrap_err();
        assert!(error.to_string().contains("escapes the asset directory"));
    }

    #[test]
    fn checked_in_synthetic_assets_match_the_runtime_loader() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
        let sh0 = Asset::load(&examples.join("minimal-sh0/manifest.json")).unwrap();
        assert_eq!(sh0.manifest.gaussian_count, 3);
        assert_eq!(sh0.sh_degree(), 0);

        let sh3 = Asset::load(&examples.join("synthetic-motion-sh3/manifest.json")).unwrap();
        assert_eq!(sh3.manifest.gaussian_count, 4096);
        assert_eq!(sh3.sh_degree(), 3);
        assert_eq!(
            sh3.sh_coefficients.as_ref().map(Vec::len),
            Some(4096 * SH3_RECORD_BYTES)
        );
    }

    #[test]
    fn sh3_record_validation_rejects_non_finite_coefficients_and_padding() {
        let mut coefficients = vec![0_u8; SH3_RECORD_BYTES];
        coefficients[0..2].copy_from_slice(&0x7c00_u16.to_le_bytes());
        let error = validate_sh3_records(&coefficients, 1).unwrap_err();
        assert!(error.to_string().contains("coefficient 0 is not finite"));

        coefficients[0..2].fill(0);
        coefficients[90] = 1;
        let error = validate_sh3_records(&coefficients, 1).unwrap_err();
        assert!(error.to_string().contains("reserved padding must be zero"));
    }
}
