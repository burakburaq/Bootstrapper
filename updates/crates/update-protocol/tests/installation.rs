use update_protocol::{fixtures::*, *};
fn record(version: &str, sequence: u64) -> ReleaseRecord {
    let mut b = bundle("alpha", "stable", version, sequence, b"NSIS", 1);
    b.manifest.body.installation = Some(contract());
    resign(&mut b, 1);
    ReleaseRecord {
        bundle: b,
        lifecycle: Lifecycle::Published,
        policy: RolloutPolicy::default(),
    }
}
fn contract() -> InstallContract {
    InstallContract {
        schema_version: 1,
        executable: "app.exe".into(),
        publisher_certificate_sha256: "a".repeat(64),
        minimum_windows_build: 10240,
        minimum_webview2_version: [0; 4],
        staging_bytes: 3,
        active_bytes: 3,
        backup_bytes: 3,
        files: vec![PayloadFile {
            path: "app.exe".into(),
            size_bytes: 3,
            sha256: sha256(b"app"),
        }],
    }
}
fn request(intent: InstallIntent, version: Option<&str>) -> InstallRequest {
    InstallRequest {
        app_id: "alpha".into(),
        channel: "stable".into(),
        target: "windows-x86_64".into(),
        installed_version: version.map(|v| v.parse().unwrap()),
        installation_id: "machine-1".into(),
        intent,
    }
}
#[test]
fn install_update_and_repair_have_distinct_selection_rules() {
    let mut records = vec![record("1.0.0", 1), record("2.0.0", 2)];
    assert_eq!(
        resolve_install(&records, &request(InstallIntent::Install, None))
            .unwrap()
            .unwrap()
            .0
            .bundle
            .manifest
            .body
            .version
            .to_string(),
        "2.0.0"
    );
    assert_eq!(
        resolve_install(&records, &request(InstallIntent::Repair, Some("1.0.0")))
            .unwrap()
            .unwrap()
            .0
            .bundle
            .manifest
            .body
            .version
            .to_string(),
        "1.0.0"
    );
    assert!(
        resolve_install(&records, &request(InstallIntent::Update, Some("2.0.0")))
            .unwrap()
            .is_none()
    );
    records[0].policy.percentage = 0;
    assert!(
        resolve_install(&records, &request(InstallIntent::Repair, Some("1.0.0")))
            .unwrap()
            .is_some()
    );
    records[0].policy.paused = true;
    assert!(
        resolve_install(&records, &request(InstallIntent::Repair, Some("1.0.0")))
            .unwrap()
            .is_none()
    );
    assert!(
        request(InstallIntent::Install, Some("1.0.0"))
            .validate()
            .is_err()
    );
    assert!(request(InstallIntent::Repair, None).validate().is_err());
}
#[test]
fn inventory_is_signed_and_rejects_windows_aliases_and_overflow() {
    let r = record("1.0.0", 1);
    r.bundle.validate(&keys(1)).unwrap();
    let mut b = r.bundle;
    b.manifest.body.installation.as_mut().unwrap().files[0].sha256 = "f".repeat(64);
    assert!(b.validate(&keys(1)).is_err());
    for path in [
        "../app.exe",
        "C:/app.exe",
        "//host/share",
        "app.exe:stream",
        "con.txt",
        "app.",
        "app ",
        "a\\b",
    ] {
        assert!(relative_payload_path(path).is_err(), "accepted {path}");
    }
    let mut c = contract();
    c.files.push(PayloadFile {
        path: "APP.EXE".into(),
        size_bytes: 0,
        sha256: sha256(b""),
    });
    assert!(c.validate().is_err());
    let mut c = contract();
    c.backup_bytes = u64::MAX;
    assert!(c.validate().is_err());
}
#[test]
fn adding_optional_contract_preserves_old_signed_bytes() {
    let b = bundle("alpha", "stable", "1.0.0", 1, b"NSIS", 1);
    let json = serde_json::to_string(&b).unwrap();
    assert!(!json.contains("installation"));
    let parsed: ReleaseBundle = serde_json::from_str(&json).unwrap();
    parsed.validate(&keys(1)).unwrap();
}
