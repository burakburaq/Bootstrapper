use anyhow::Result;
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use semver::Version;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use update_client::*;
use update_protocol::{fixtures::*, *};

#[derive(Clone)]
struct Mock {
    response: UpdateResponse,
    full: Arc<Vec<u8>>,
    patch: Arc<Vec<u8>>,
    full_calls: Arc<AtomicUsize>,
    patch_calls: Arc<AtomicUsize>,
    expired: bool,
    revoked: bool,
}
async fn check(State(s): State<Mock>, headers: HeaderMap) -> impl IntoResponse {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer private-channel");
    Json(s.response)
}
async fn redirect(
    State(s): State<Mock>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer private-channel");
    let host = url::Url::parse(&s.response.status_url).unwrap();
    let kind = if id.starts_with("full-") {
        "full"
    } else {
        "patch"
    };
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(
            header::LOCATION,
            host.join(&format!("/objects/{kind}")).unwrap().to_string(),
        )],
    )
}
async fn object(
    State(s): State<Mock>,
    axum::extract::Path(kind): axum::extract::Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    assert!(
        !headers.contains_key(header::AUTHORIZATION),
        "channel credentials leaked to storage"
    );
    if kind == "full" {
        let call = s.full_calls.fetch_add(1, Ordering::Relaxed);
        if s.expired && call == 0 {
            return (StatusCode::FORBIDDEN, Body::empty()).into_response();
        }
        (StatusCode::OK, Body::from(s.full.as_ref().clone())).into_response()
    } else {
        s.patch_calls.fetch_add(1, Ordering::Relaxed);
        (StatusCode::OK, Body::from(s.patch.as_ref().clone())).into_response()
    }
}
async fn status(State(s): State<Mock>) -> Json<ReleaseStatus> {
    Json(ReleaseStatus {
        release_id: s.response.release_id,
        lifecycle: if s.revoked {
            Lifecycle::Revoked
        } else {
            Lifecycle::Published
        },
        install_allowed: !s.revoked,
    })
}
async fn fixture(
    directory: &Path,
    corrupt: bool,
    expired: bool,
    revoked: bool,
) -> (
    UpdateClient,
    Vec<u8>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let base = vec![42u8; 64 * 1024];
    let mut full = base.clone();
    full[2048..4096].fill(21);
    let patch = delta::create(&base, &full).unwrap();
    let mut bundle = bundle("alpha", "staging", "2.0.0", 2, &full, 1);
    let full_artifact = bundle.artifacts[0].body.clone();
    let patch_hash = sha256(&patch);
    let id = format!("delta-{patch_hash}");
    let descriptor = ArtifactDescriptor {
        schema_version: 1,
        app_id: "alpha".into(),
        target: "windows-x86_64".into(),
        artifact_id: id.clone(),
        version: "2.0.0".parse().unwrap(),
        object_key: format!("alpha/windows-x86_64/{patch_hash}.zstd"),
        sha256: patch_hash,
        size_bytes: patch.len() as u64,
        package_signature: full_artifact.package_signature.clone(),
        artifact: ArtifactKind::Delta {
            base_sha256: sha256(&base),
            base_size_bytes: base.len() as u64,
            result_sha256: full_artifact.sha256.clone(),
            result_size_bytes: full.len() as u64,
            full_artifact_id: full_artifact.artifact_id.clone(),
        },
    };
    bundle
        .artifacts
        .push(sign(descriptor, "test".into(), &key(1)).unwrap());
    resign(&mut bundle, 1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let prefix =
        format!("{origin}/v1/apps/alpha/channels/staging/releases/2.0.0/targets/windows-x86_64");
    let urls = bundle
        .artifacts
        .iter()
        .map(|a| {
            (
                a.body.artifact_id.clone(),
                format!("{prefix}/artifacts/{}/download", a.body.artifact_id),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let response = UpdateResponse {
        schema_version: 1,
        app_id: "alpha".into(),
        channel: "staging".into(),
        target: "windows-x86_64".into(),
        release_id: "release-2".into(),
        version: "2.0.0".parse().unwrap(),
        published_at: bundle.manifest.body.published_at,
        notes: String::new(),
        download_url: urls[&full_artifact.artifact_id].clone(),
        signature: full_artifact.package_signature,
        signing_key_id: "test".into(),
        sha256: full_artifact.sha256,
        size_bytes: full.len() as u64,
        minimum_supported_version: None,
        bundle,
        selected_artifact_id: id,
        download_urls: urls,
        status_url: format!("{prefix}/status"),
    };
    let full_calls = Arc::new(AtomicUsize::new(0));
    let patch_calls = Arc::new(AtomicUsize::new(0));
    let state = Mock {
        response,
        full: Arc::new(full),
        patch: Arc::new(if corrupt { b"corrupt".to_vec() } else { patch }),
        full_calls: full_calls.clone(),
        patch_calls: patch_calls.clone(),
        expired,
        revoked,
    };
    let router=Router::new().route("/v1/update/check",post(check))
        .route("/v1/apps/alpha/channels/staging/releases/2.0.0/targets/windows-x86_64/artifacts/{id}/download",get(redirect))
        .route("/v1/apps/alpha/channels/staging/releases/2.0.0/targets/windows-x86_64/status",get(status))
        .route("/objects/{kind}",get(object)).with_state(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let client = UpdateClient::new(ClientConfig {
        server_url: origin.parse().unwrap(),
        app_id: "alpha".into(),
        channel: "staging".into(),
        target: "windows-x86_64".into(),
        metadata_keys: keys(1),
        package_public_key: package_signature(b"ignored", 1).1,
        channel_token: Some("private-channel".into()),
        allowed_download_hosts: BTreeSet::from(["127.0.0.1".into()]),
        cache_dir: directory.join("cache"),
        max_artifact_bytes: 1024 * 1024,
        allow_loopback_http: true,
    })
    .unwrap();
    (client, base, full_calls, patch_calls, task)
}
struct TestInstaller {
    version: Version,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Installer for TestInstaller {
    async fn inspect_version_and_publisher(&self, _: &Path) -> Result<Version> {
        Ok(self.version.clone())
    }
    async fn install(&self, _: &Path) -> Result<()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    async fn installed_version(&self) -> Result<Version> {
        Ok("1.0.0".parse().unwrap())
    }
}
#[tokio::test]
async fn valid_delta_install_and_reconcile() {
    let dir = tempfile::tempdir().unwrap();
    let (c, base, full, patch, task) = fixture(dir.path(), false, false, false).await;
    let path = dir.path().join("base");
    tokio::fs::write(&path, base).await.unwrap();
    let update = c
        .check("1.0.0".parse().unwrap(), "install-1".into(), Some(&path))
        .await
        .unwrap()
        .unwrap();
    let prepared = c.prepare(update, Some(&path)).await.unwrap();
    assert_eq!(full.load(Ordering::Relaxed), 0);
    assert_eq!(patch.load(Ordering::Relaxed), 1);
    let journal = dir.path().join("state.json");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut state = c
        .install(
            prepared,
            &TestInstaller {
                version: "2.0.0".parse().unwrap(),
                calls: calls.clone(),
            },
            &journal,
        )
        .await
        .unwrap();
    assert_eq!(state.phase, InstallPhase::AwaitingRestart);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(
        state
            .reconcile(&"2.0.0".parse().unwrap(), &journal)
            .await
            .unwrap()
    );
    assert_eq!(
        InstallJournal::load(&journal).await.unwrap().phase,
        InstallPhase::Completed
    );
    task.abort();
}
#[tokio::test]
async fn corrupted_delta_falls_back_and_expired_url_refreshes() {
    let dir = tempfile::tempdir().unwrap();
    let (c, base, full, patch, task) = fixture(dir.path(), true, true, false).await;
    let path = dir.path().join("base");
    tokio::fs::write(&path, base).await.unwrap();
    let update = c
        .check("1.0.0".parse().unwrap(), "install-1".into(), Some(&path))
        .await
        .unwrap()
        .unwrap();
    c.prepare(update, Some(&path)).await.unwrap();
    assert_eq!(patch.load(Ordering::Relaxed), 1);
    assert_eq!(full.load(Ordering::Relaxed), 2);
    task.abort();
}
#[tokio::test]
async fn changed_base_falls_back_without_using_delta() {
    let dir = tempfile::tempdir().unwrap();
    let (c, base, full, patch, task) = fixture(dir.path(), false, false, false).await;
    let path = dir.path().join("base");
    tokio::fs::write(&path, &base).await.unwrap();
    let update = c
        .check("1.0.0".parse().unwrap(), "install-1".into(), Some(&path))
        .await
        .unwrap()
        .unwrap();
    tokio::fs::write(&path, b"wrong-base").await.unwrap();
    c.prepare(update, Some(&path)).await.unwrap();
    assert_eq!(patch.load(Ordering::Relaxed), 0);
    assert_eq!(full.load(Ordering::Relaxed), 1);
    task.abort();
}
#[tokio::test]
async fn revoked_release_never_starts_installer() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _, _, _, task) = fixture(dir.path(), false, false, true).await;
    let update = c
        .check("1.0.0".parse().unwrap(), "install-1".into(), None)
        .await
        .unwrap()
        .unwrap();
    let prepared = c.prepare(update, None).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    assert!(
        c.install(
            prepared,
            &TestInstaller {
                version: "2.0.0".parse().unwrap(),
                calls: calls.clone()
            },
            &dir.path().join("state.json")
        )
        .await
        .is_err()
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    task.abort();
}
#[tokio::test]
async fn higher_manifest_with_old_product_version_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _, _, _, task) = fixture(dir.path(), false, false, false).await;
    let update = c
        .check("1.0.0".parse().unwrap(), "install-1".into(), None)
        .await
        .unwrap()
        .unwrap();
    let prepared = c.prepare(update, None).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    assert!(
        c.install(
            prepared,
            &TestInstaller {
                version: "1.1.0".parse().unwrap(),
                calls: calls.clone()
            },
            &dir.path().join("state.json")
        )
        .await
        .is_err()
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    task.abort();
}
#[test]
fn delta_resource_limit() {
    let base = vec![42; 4096];
    let patch = delta::create(&base, &base).unwrap();
    assert!(delta::apply(&base, &patch, 4096, 1024).is_err());
}

#[tokio::test]
async fn missing_cached_base_does_not_block_full_update() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _, full, patch, task) = fixture(dir.path(), false, false, false).await;
    let missing = dir.path().join("missing-base");
    let update = c
        .check("1.0.0".parse().unwrap(), "install-1".into(), Some(&missing))
        .await
        .unwrap()
        .unwrap();
    c.prepare(update, Some(&missing)).await.unwrap();
    assert_eq!(full.load(Ordering::Relaxed), 1);
    assert_eq!(patch.load(Ordering::Relaxed), 0);
    task.abort();
}
