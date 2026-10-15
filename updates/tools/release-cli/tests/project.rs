use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};
use update_protocol::{ReleaseBundle, fixtures::*};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    product: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        std::fs::create_dir_all(root.join("installer/assets")).unwrap();
        std::fs::create_dir_all(root.join("release")).unwrap();
        std::fs::write(root.join("installer/assets/icon.webp"), b"icon fixture").unwrap();
        let product = root.join("installer/product.json");
        let config = root.join("release/config.json");
        write(
            &product,
            &json!({"schemaVersion":1,"appId":"alpha","displayName":"Alpha","publisher":"TalayLabs","productDirectory":"Alpha","executable":"alpha.exe","icon":"assets/icon.webp","description":"Release test"}),
        );
        write(
            &config,
            &json!({"schemaVersion":1,"protocol":"talaylabs-v1","serverUrl":null,"bucket":null,"channel":"staging","target":"windows-x86_64","package":"../package.bin","output":"../out/{version}","packageSignature":"../package.sig","installationContract":null,"metadataKeyId":"test"}),
        );
        let package = vec![42; 64 * 1024];
        let (signature, public) = package_signature(&package, 1);
        std::fs::write(root.join("package.bin"), package).unwrap();
        std::fs::write(root.join("package.sig"), signature).unwrap();
        std::fs::write(root.join("package.pub"), public).unwrap();
        std::fs::write(root.join("metadata.key"), "01".repeat(32)).unwrap();
        write(
            &root.join("keys.json"),
            &serde_json::to_value(keys(1)).unwrap(),
        );
        Self {
            _dir: dir,
            root,
            product,
            config,
        }
    }
    fn run(&self, action: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_release-cli"))
            .current_dir(std::env::temp_dir())
            .args([
                "project",
                "--product",
                self.product.to_str().unwrap(),
                "--config",
                self.config.to_str().unwrap(),
                "--version",
                "2.0.0",
                "--sequence",
                "2",
                action,
            ])
            .env("RELEASE_METADATA_KEY_FILE", self.root.join("metadata.key"))
            .env(
                "RELEASE_METADATA_PUBLIC_KEY_FILE",
                self.root.join("keys.json"),
            )
            .env(
                "RELEASE_PACKAGE_PUBLIC_KEY_FILE",
                self.root.join("package.pub"),
            )
            .env_remove("UPDATE_ADMIN_TOKEN")
            .env_remove("UPDATE_ADMIN_TOKEN_FILE")
            .output()
            .unwrap()
    }
    fn change(&self, path: &Path, name: &str, value: Value) {
        let mut doc: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        doc[name] = value;
        write(path, &doc);
    }
    fn out(&self) -> PathBuf {
        self.root.join("out/2.0.0")
    }
}
fn write(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn configured_prepare_uses_config_relative_paths_and_verifies_final_bytes() {
    let f = Fixture::new();
    success(f.run("validate"));
    success(f.run("prepare"));
    success(f.run("verify"));
    let b: ReleaseBundle =
        serde_json::from_slice(&std::fs::read(f.out().join("bundle.json")).unwrap()).unwrap();
    b.validate(&keys(1)).unwrap();
    assert_eq!(b.manifest.body.app_id, "alpha");
    assert_eq!(b.manifest.body.sequence, 2);
    assert!(
        !f.run("prepare").status.success(),
        "immutable preparation must not be overwritten"
    );
    let artifact = &b.artifacts[0].body;
    std::fs::write(
        f.out().join(format!("{}.bin", artifact.artifact_id)),
        b"corrupt",
    )
    .unwrap();
    assert!(!f.run("verify").status.success());
}

#[test]
fn configured_verification_rejects_cross_product_or_version() {
    let f = Fixture::new();
    success(f.run("prepare"));
    f.change(&f.product, "appId", json!("other-product"));
    assert!(!f.run("verify").status.success());
    f.change(&f.product, "appId", json!("alpha"));
    f.change(&f.config, "channel", json!("production"));
    assert!(!f.run("verify").status.success());
}

#[test]
fn configured_publish_requires_matching_draft_receipt_before_network() {
    let f = Fixture::new();
    success(f.run("prepare"));
    let result = f.run("publish");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("receipt is missing"));
    write(
        &f.out().join("draft.json"),
        &json!({"appId":"other","protocol":"talaylabs-v1","bundleSha256":"wrong","serverUrl":null,"bucket":null,"releaseId":"wrong"}),
    );
    let result = f.run("publish");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("receipt differs"));
}

#[test]
fn configured_validation_rejects_secrets_unknown_fields_and_unsafe_origins() {
    let f = Fixture::new();
    f.change(&f.config, "secretAccessKey", json!("never-print-this"));
    let result = f.run("validate");
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("never-print-this"));
    let f = Fixture::new();
    f.change(&f.config, "serverUrl", json!("http://example.com"));
    assert!(!f.run("validate").status.success());
    f.change(
        &f.config,
        "serverUrl",
        json!("https://example.com/?token=secret"),
    );
    assert!(!f.run("validate").status.success());
    f.change(&f.config, "serverUrl", Value::Null);
    f.change(&f.config, "output", json!("../{unknown}"));
    assert!(!f.run("validate").status.success());
    let f = Fixture::new();
    f.change(&f.product, "productDirectory", json!("../Alpha"));
    assert!(!f.run("validate").status.success());
}

#[test]
fn configured_prepare_rejects_wrong_signing_key_before_creating_output() {
    let f = Fixture::new();
    std::fs::write(f.root.join("metadata.key"), "02".repeat(32)).unwrap();
    assert!(!f.run("prepare").status.success());
    assert!(!f.out().exists());
}
