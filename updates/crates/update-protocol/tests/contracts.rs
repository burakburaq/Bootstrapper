#![cfg(feature = "test-support")]
use update_protocol::{fixtures::*, *};

fn request() -> CheckRequest {
    CheckRequest {
        app_id: "alpha".into(),
        channel: "production".into(),
        target: "windows-x86_64".into(),
        current_version: "1.0.0".parse().unwrap(),
        installation_id: Some("install-1".into()),
        supports_delta: false,
        base_sha256: None,
        base_size_bytes: None,
        schema_version: 1,
    }
}
#[test]
fn signature_and_cross_app_trust() {
    let mut b = bundle("alpha", "production", "2.0.0", 2, b"package", 1);
    b.validate(&keys(1)).unwrap();
    assert!(b.validate(&keys(2)).is_err());
    b.manifest.body.version = "3.0.0".parse().unwrap();
    assert!(b.validate(&keys(1)).is_err());
}
#[test]
fn descriptor_and_object_namespace_tampering() {
    let mut b = bundle("alpha", "production", "2.0.0", 2, b"package", 1);
    b.artifacts[0].body.object_key = "beta/windows-x86_64/package".into();
    b.artifacts[0] = sign(b.artifacts[0].body.clone(), "test".into(), &key(1)).unwrap();
    resign(&mut b, 1);
    assert!(b.validate(&keys(1)).is_err());
}
#[test]
fn package_signature_checks_bytes_and_trusted_comment() {
    let (signature, public) = package_signature(b"good", 1);
    verify_package(b"good", &signature, &public).unwrap();
    assert!(verify_package(b"bad", &signature, &public).is_err());
}
#[test]
fn required_checkpoint_cannot_be_skipped() {
    let checkpoint = ReleaseRecord {
        bundle: bundle("alpha", "production", "1.5.0", 1, b"first", 1),
        lifecycle: Lifecycle::Published,
        policy: RolloutPolicy::default(),
    };
    let mut destination = bundle("alpha", "production", "2.0.0", 2, b"second", 1);
    destination
        .manifest
        .body
        .required_checkpoints
        .push(Checkpoint {
            version: "1.5.0".parse().unwrap(),
            release_id: "release-1".into(),
        });
    resign(&mut destination, 1);
    let destination = ReleaseRecord {
        bundle: destination,
        lifecycle: Lifecycle::Published,
        policy: RolloutPolicy::default(),
    };
    let mut records = vec![checkpoint, destination];
    assert_eq!(
        resolve(&records, &request())
            .unwrap()
            .unwrap()
            .0
            .bundle
            .manifest
            .body
            .version
            .to_string(),
        "1.5.0"
    );
    records[0].lifecycle = Lifecycle::Revoked;
    assert!(resolve(&records, &request()).is_err());
}
#[test]
fn cohort_is_stable_and_nested() {
    let mut policy = RolloutPolicy {
        percentage: 25,
        cohort_key: "release-2".into(),
        paused: false,
        revision: 1,
    };
    for n in 0..1000 {
        let id = format!("install-{n}");
        let small = eligible("alpha", &policy, Some(&id));
        policy.percentage = 75;
        let large = eligible("alpha", &policy, Some(&id));
        assert!(!small || large);
        policy.percentage = 25;
    }
    assert!(!eligible("alpha", &policy, None));
}
#[test]
fn revoke_is_terminal() {
    assert!(transition(&Lifecycle::Revoked, "enable").is_err());
    assert!(transition(&Lifecycle::Revoked, "publish").is_err());
}
