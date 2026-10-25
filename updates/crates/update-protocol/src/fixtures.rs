//! Deterministic TEST ONLY keys and bundles. Feature is never enabled in production builds.
use crate::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey};
use semver::Version;
use std::collections::BTreeMap;

pub fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}
pub fn keys(seed: u8) -> KeyRing {
    BTreeMap::from([(
        "test".into(),
        STANDARD.encode(key(seed).verifying_key().to_bytes()),
    )])
}
pub fn package_signature(bytes: &[u8], seed: u8) -> (String, String) {
    let key = key(seed);
    let id = [seed; 8];
    let signature = key.sign(bytes).to_bytes();
    let mut body = b"Ed".to_vec();
    body.extend(id);
    body.extend(signature);
    let mut public = b"Ed".to_vec();
    public.extend(id);
    public.extend(key.verifying_key().to_bytes());
    let trusted = "timestamp:1";
    let mut global = signature.to_vec();
    global.extend(trusted.as_bytes());
    let text = format!(
        "untrusted comment: test\n{}\ntrusted comment: {trusted}\n{}\n",
        STANDARD.encode(body),
        STANDARD.encode(key.sign(&global).to_bytes())
    );
    (STANDARD.encode(text), STANDARD.encode(public))
}
pub fn bundle(
    app: &str,
    channel: &str,
    version: &str,
    sequence: u64,
    bytes: &[u8],
    seed: u8,
) -> ReleaseBundle {
    let (signature, _) = package_signature(bytes, seed);
    let hash = sha256(bytes);
    let id = format!("full-{hash}");
    let version = Version::parse(version).unwrap();
    let a = ArtifactDescriptor {
        schema_version: SCHEMA_VERSION,
        app_id: app.into(),
        target: "windows-x86_64".into(),
        artifact_id: id.clone(),
        version: version.clone(),
        object_key: format!("{app}/windows-x86_64/{hash}.package"),
        sha256: hash,
        size_bytes: bytes.len() as u64,
        artifact: ArtifactKind::Full,
        package_signature: signature,
    };
    let manifest = ReleaseManifest {
        schema_version: SCHEMA_VERSION,
        app_id: app.into(),
        channel: channel.into(),
        target: "windows-x86_64".into(),
        release_id: format!("release-{sequence}"),
        version,
        sequence,
        published_at: Utc::now(),
        notes: String::new(),
        minimum_supported_version: None,
        required_checkpoints: vec![],
        full_artifact_id: id.clone(),
        artifacts: BTreeMap::from([(id, sha256(&canonical(&a).unwrap()))]),
        installation: None,
    };
    ReleaseBundle {
        manifest: sign(manifest, "test".into(), &key(seed)).unwrap(),
        artifacts: vec![sign(a, "test".into(), &key(seed)).unwrap()],
    }
}
pub fn resign(bundle: &mut ReleaseBundle, seed: u8) {
    let mut m = bundle.manifest.body.clone();
    m.artifacts = bundle
        .artifacts
        .iter()
        .map(|a| {
            (
                a.body.artifact_id.clone(),
                sha256(&canonical(&a.body).unwrap()),
            )
        })
        .collect();
    bundle.manifest = sign(m, "test".into(), &key(seed)).unwrap();
}
