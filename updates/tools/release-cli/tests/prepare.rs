use std::{
    path::Path,
    process::{Command, Output},
};
use update_protocol::{fixtures::*, *};
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_release-cli"))
        .args(args)
        .output()
        .unwrap()
}
fn file(path: &Path) -> &str {
    path.to_str().unwrap()
}
#[test]
fn offline_prepare_delta_verify_and_immutable_output() {
    let directory = tempfile::tempdir().unwrap();
    let d = directory.path();
    let base = vec![42; 64 * 1024];
    let mut full = base.clone();
    full[2048..4096].fill(11);
    std::fs::write(d.join("base"), &base).unwrap();
    std::fs::write(d.join("package"), &full).unwrap();
    let (sig, public) = package_signature(&full, 1);
    std::fs::write(d.join("sig"), sig).unwrap();
    std::fs::write(d.join("pub"), public).unwrap();
    std::fs::write(d.join("metadata.key"), "01".repeat(32)).unwrap();
    std::fs::write(d.join("keys.json"), serde_json::to_vec(&keys(1)).unwrap()).unwrap();
    let package = d.join("package");
    let signature = d.join("sig");
    let public = d.join("pub");
    let key = d.join("metadata.key");
    let out = d.join("release");
    let base_path = d.join("base");
    let args = [
        "prepare",
        "--app-id",
        "alpha",
        "--channel",
        "staging",
        "--target",
        "windows-x86_64",
        "--version",
        "2.0.0",
        "--sequence",
        "2",
        "--package",
        file(&package),
        "--package-signature",
        file(&signature),
        "--package-public-key",
        file(&public),
        "--metadata-key",
        file(&key),
        "--key-id",
        "test",
        "--out",
        file(&out),
        "--base",
        file(&base_path),
    ];
    let prepared = run(&args);
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let bundle_path = out.join("bundle.json");
    let b: ReleaseBundle = serde_json::from_slice(&std::fs::read(&bundle_path).unwrap()).unwrap();
    b.validate(&keys(1)).unwrap();
    assert_eq!(b.artifacts.len(), 2);
    let delta = b
        .artifacts
        .iter()
        .find(|a| matches!(a.body.artifact, ArtifactKind::Delta { .. }))
        .unwrap();
    let patch = std::fs::read(out.join(format!("{}.bin", delta.body.artifact_id))).unwrap();
    assert_eq!(
        update_client::delta::apply(&base, &patch, full.len() as u64, 1024 * 1024).unwrap(),
        full
    );
    assert!(!run(&args).status.success());
    let keys_path = d.join("keys.json");
    assert!(
        run(&[
            "verify",
            "--bundle",
            file(&bundle_path),
            "--public-key",
            file(&keys_path)
        ])
        .status
        .success()
    );
    let mut modified = b;
    modified.manifest.body.version = "3.0.0".parse().unwrap();
    std::fs::write(&bundle_path, serde_json::to_vec(&modified).unwrap()).unwrap();
    assert!(
        !run(&[
            "verify",
            "--bundle",
            file(&bundle_path),
            "--public-key",
            file(&keys_path)
        ])
        .status
        .success()
    );
}
#[test]
fn key_generation_does_not_overwrite_existing_secret() {
    let directory = tempfile::tempdir().unwrap();
    let key = directory.path().join("new.key");
    let first = run(&["keygen", "--out", file(&key), "--key-id", "new"]);
    assert!(first.status.success());
    let keys: KeyRing = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(keys.len(), 1);
    let bytes = std::fs::read(&key).unwrap();
    assert_eq!(bytes.len(), 64);
    assert!(
        !run(&["keygen", "--out", file(&key), "--key-id", "new"])
            .status
            .success()
    );
    assert_eq!(std::fs::read(&key).unwrap(), bytes);
}
#[test]
fn windows_package_inventory_and_real_nsis_compilation() {
    let directory = tempfile::tempdir().unwrap();
    let d = directory.path();
    let payload = d.join("payload");
    std::fs::create_dir_all(payload.join("assets")).unwrap();
    std::fs::write(payload.join("app.exe"), b"fixture; never execute").unwrap();
    std::fs::write(
        payload.join("maintenance.exe"),
        b"UI fixture; never execute",
    )
    .unwrap();
    std::fs::write(payload.join("assets/config.json"), b"{}").unwrap();
    let out = d.join("generated");
    let result = run(&[
        "windows-package",
        "--maintenance-executable",
        "maintenance.exe",
        "--payload",
        file(&payload),
        "--app-id",
        "alpha",
        "--product-directory",
        "Alpha",
        "--display-name",
        "Owllig Academy",
        "--publisher",
        "TalayLabs Test",
        "--copyright",
        "© 2026 TalayLabs Test",
        "--executable",
        "app.exe",
        "--version",
        "2.0.0-beta.1",
        "--publisher-certificate-sha256",
        &"a".repeat(64),
        "--out",
        file(&out),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let contract: InstallContract =
        serde_json::from_slice(&std::fs::read(out.join("contract.json")).unwrap()).unwrap();
    let script = std::fs::read_to_string(out.join("package.nsi")).unwrap();
    assert!(script.contains("\"DisplayIcon\" '$\\\"$Active\\app.exe$\\\",0'"));
    assert!(script.contains("\"ModifyPath\"") && script.contains("--repair'"));
    assert!(script.contains("--uninstall'") && script.contains("\"NoModify\" 0"));
    assert!(!script.contains("@MAINTENANCE_REGISTRATION@"));
    assert!(script.contains(r#"$PROGRAMFILES64\TalayLabs\Alpha"#));
    assert!(script.contains(r#"$PROGRAMFILES64\TalayLabs\Alpha\Service\maintenance-"#));
    assert!(!script.contains("Alpha Update Service"));
    assert!(script.contains(r#""LegalCopyright" "© 2026 TalayLabs Test""#));
    assert!(script.contains(r#""DisplayName" "Owllig Academy""#));
    assert!(script.contains(r#""Publisher" "TalayLabs Test""#));
    update_client::inventory::verify_inventory(&payload, &contract).unwrap();
    std::fs::write(payload.join("unexpected"), b"extra").unwrap();
    assert!(update_client::inventory::verify_inventory(&payload, &contract).is_err());
    std::fs::remove_file(payload.join("unexpected")).unwrap();
    // A live service is excluded only for installed-tree verification.
    std::fs::create_dir(payload.join("Service")).unwrap();
    std::fs::write(
        payload.join("Service/update-service.exe"),
        b"running service",
    )
    .unwrap();
    update_client::inventory::verify_installed_inventory(&payload, &contract).unwrap();
    assert!(update_client::inventory::verify_inventory(&payload, &contract).is_err());
    let mut forbidden = contract.clone();
    forbidden.files[0].path = "sErViCe/helper.exe".into();
    assert!(forbidden.validate().is_err());
    assert!(script.contains("StrCmp $Entry \"Service\" move_read"));
    assert!(!script.contains("Rename \"$Active\" \"$Backup\""));
    #[cfg(windows)]
    if let Some(makensis) = std::env::var_os("UPDATE_TEST_MAKENSIS") {
        let result = Command::new(makensis)
            .arg(out.join("package.nsi"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            update_client::windows::read_product_version(&out.join("package.exe"))
                .unwrap()
                .to_string(),
            "2.0.0-beta.1"
        );
        assert!(
            update_client::windows::verify_authenticode(&out.join("package.exe"), &"a".repeat(64))
                .is_err()
        );
    }
}
