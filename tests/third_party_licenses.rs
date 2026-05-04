use std::fs;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const SCRIPT_PATH: &str = "scripts/generate-third-party-licenses.sh";

#[test]
fn license_report_generator_script_contract_is_stable() {
    let metadata = fs::metadata(SCRIPT_PATH).expect("license report generator script exists");
    assert!(metadata.is_file(), "generator path must be a file");

    #[cfg(unix)]
    assert_ne!(
        metadata.permissions().mode() & 0o111,
        0,
        "generator script must be executable"
    );

    let script = fs::read_to_string(SCRIPT_PATH).expect("generator script is valid UTF-8");
    assert!(
        script.contains("command -v cargo-about"),
        "script must fail clearly before invoking a missing cargo-about"
    );
    assert!(
        script.contains("${HOME}/.cargo/bin/cargo-about"),
        "script must fall back to Cargo's standard install location"
    );
    assert!(
        script.contains("OUTPUT_DIR=\"${ROOT_DIR}/dist\""),
        "script must write into the documented dist directory"
    );
    assert!(
        script.contains("THIRD_PARTY_LICENSES.txt"),
        "script must write to the documented release artifact path"
    );
    assert!(
        script.contains("--locked"),
        "script must use Cargo.lock without changing dependency resolution"
    );
}

#[test]
#[cfg(unix)]
fn license_report_generator_explains_missing_cargo_about() {
    let empty_path = tempfile::tempdir().expect("temporary PATH directory can be created");
    let output = Command::new("/bin/bash")
        .arg(SCRIPT_PATH)
        .env("PATH", empty_path.path())
        .env("HOME", empty_path.path())
        .output()
        .expect("generator script can be executed with bash");

    assert_eq!(
        output.status.code(),
        Some(127),
        "missing cargo-about should use the standard command-not-found exit code"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr is valid UTF-8");
    assert!(
        stderr.contains("cargo-about is required"),
        "missing cargo-about diagnostic should name the required tool: {stderr}"
    );
    assert!(
        stderr.contains("cargo install --locked cargo-about --features cli"),
        "missing cargo-about diagnostic should include install guidance: {stderr}"
    );
    assert!(
        stderr.contains("export PATH=\"$HOME/.cargo/bin:$PATH\""),
        "missing cargo-about diagnostic should mention Cargo's bin directory: {stderr}"
    );
}
