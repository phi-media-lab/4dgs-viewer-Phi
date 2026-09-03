use std::{env, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=RUSTC");

    let rustc = env::var_os("RUSTC").expect("Cargo did not provide RUSTC");
    let output = Command::new(rustc)
        .arg("--version")
        .output()
        .expect("failed to run rustc --version");
    assert!(output.status.success(), "rustc --version failed");

    let stdout = String::from_utf8(output.stdout).expect("rustc --version was not UTF-8");
    let mut fields = stdout.split_whitespace();
    assert_eq!(
        fields.next(),
        Some("rustc"),
        "unexpected rustc --version output"
    );
    let release = fields.next().expect("rustc --version omitted its release");
    assert!(
        release
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')),
        "unexpected rustc release token"
    );

    println!("cargo:rustc-env=PHI_BUILD_RUSTC_VERSION={release}");
}
