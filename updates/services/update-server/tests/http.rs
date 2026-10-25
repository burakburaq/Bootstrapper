use anyhow::Result;
use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
};
use tower::ServiceExt;
use update_protocol::{fixtures::*, *};
use update_server::{
    config::*,
    http::{AppState, router},
    repository::{MemoryRepository, Repository},
    storage::Storage,
};

struct TestStorage;
fn json_post(path: &str, value: &impl serde::Serialize) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(value).unwrap()))
        .unwrap()
}
#[tokio::test]
async fn installation_resolution_and_fresh_authorization_respect_lifecycle_and_scopes() {
    let repo = Arc::new(MemoryRepository::default());
    let mut b = bundle("alpha", "production", "2.0.0", 2, b"NSIS", 1);
    b.manifest.body.installation = Some(InstallContract {
        schema_version: 1,
        executable: "app.exe".into(),
        publisher_certificate_sha256: "a".repeat(64),
        minimum_windows_build: 10240,
        minimum_webview2_version: [0; 4],
        staging_bytes: 1,
        active_bytes: 1,
        backup_bytes: 1,
        files: vec![PayloadFile {
            path: "app.exe".into(),
            size_bytes: 1,
            sha256: sha256(b"a"),
        }],
    });
    resign(&mut b, 1);
    repo.draft(b).await.unwrap();
    repo.act(
        "alpha",
        "production",
        "release-2",
        &AdminAction {
            operation_id: "publish".into(),
            action: "publish".into(),
            policy: None,
        },
    )
    .await
    .unwrap();
    let s = state(repo.clone());
    let signer = Arc::new(update_server::http::AuthorizationSigner {
        key_id: "test".into(),
        key: key(3),
    });
    let app = update_server::http::router_with_authorization(s, Some(signer));
    let installation = InstallRequest {
        app_id: "alpha".into(),
        channel: "production".into(),
        target: "windows-x86_64".into(),
        installed_version: None,
        installation_id: "machine-1".into(),
        intent: InstallIntent::Install,
    };
    assert_eq!(
        app.clone()
            .oneshot(json_post("/v1/install/resolve", &installation))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let request = AuthorizationRequest {
        installation,
        release_id: "release-2".into(),
        operation_id: "op-1".into(),
        nonce: "a".repeat(64),
    };
    let response = app
        .clone()
        .oneshot(json_post("/v1/install/authorize", &request))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let permit: Signed<InstallAuthorization> =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    verify(&permit, &keys(3)).unwrap();
    assert_eq!(permit.body.nonce, request.nonce);
    assert!(verify(&permit, &keys(1)).is_err());
    let mut private = request.clone();
    private.installation.channel = "staging".into();
    assert_eq!(
        app.clone()
            .oneshot(json_post("/v1/install/authorize", &private))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    repo.act(
        "alpha",
        "production",
        "release-2",
        &AdminAction {
            operation_id: "revoke".into(),
            action: "revoke".into(),
            policy: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        app.oneshot(json_post("/v1/install/authorize", &request))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn install_authorization_is_fail_closed_without_an_operational_key() {
    let repo = Arc::new(MemoryRepository::default());
    let request = AuthorizationRequest {
        installation: InstallRequest {
            app_id: "alpha".into(),
            channel: "production".into(),
            target: "windows-x86_64".into(),
            installed_version: None,
            installation_id: "machine-1".into(),
            intent: InstallIntent::Install,
        },
        release_id: "unknown".into(),
        operation_id: "op-1".into(),
        nonce: "a".repeat(64),
    };
    assert_ne!(
        router(state(repo))
            .oneshot(json_post("/v1/install/authorize", &request))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
struct UnavailableStorage;
#[async_trait]
impl Storage for UnavailableStorage {
    async fn presign(&self, _: &AppConfig, _: &ArtifactDescriptor) -> Result<String> {
        anyhow::bail!("unavailable")
    }
    async fn verify(&self, _: &AppConfig, _: &ArtifactDescriptor) -> Result<()> {
        anyhow::bail!("unavailable")
    }
    async fn ready(&self, _: &AppConfig) -> Result<()> {
        anyhow::bail!("unavailable")
    }
}

#[tokio::test]
async fn r2_failure_is_not_reported_as_no_update_or_a_valid_redirect() {
    let repo = Arc::new(MemoryRepository::default());
    published(&repo, "alpha", "production", 1).await;
    let mut s = state(repo);
    s.storage = Arc::new(UnavailableStorage);
    let app = router(s);
    for path in [
        "/v1/tauri/beta/production/windows-x86_64/1.0.0",
        "/v1/apps/alpha/channels/production/targets/windows-x86_64/updates?current_version=1.0.0",
        "/v1/apps/alpha/channels/production/releases/2.0.0/targets/windows-x86_64/download",
    ] {
        assert_eq!(
            app.clone().oneshot(get(path, None)).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(
        app.oneshot(get("/v1/tauri/beta/staging/windows-x86_64/1.0.0", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
#[async_trait]
impl Storage for TestStorage {
    async fn presign(&self, a: &AppConfig, d: &ArtifactDescriptor) -> Result<String> {
        Ok(format!(
            "https://example.r2.cloudflarestorage.com/{}/{}",
            a.bucket, d.object_key
        ))
    }
    async fn verify(&self, _: &AppConfig, _: &ArtifactDescriptor) -> Result<()> {
        Ok(())
    }
    async fn ready(&self, _: &AppConfig) -> Result<()> {
        Ok(())
    }
}
fn app(id: &str, seed: u8) -> AppConfig {
    AppConfig {
        app_id: id.into(),
        channels: BTreeMap::from([
            (
                "production".into(),
                ChannelConfig {
                    private: false,
                    read_token_sha256: None,
                },
            ),
            (
                "staging".into(),
                ChannelConfig {
                    private: true,
                    read_token_sha256: Some(sha256(format!("{id}-read").as_bytes())),
                },
            ),
        ]),
        targets: BTreeSet::from(["windows-x86_64".into()]),
        metadata_keys: keys(seed),
        package_public_key: package_signature(b"data", seed).1,
        bucket: format!("{id}-updates"),
        health_object_key: format!("{id}/health.txt"),
        admin_tokens: vec![AdminToken {
            token_sha256: sha256(format!("{id}-admin").as_bytes()),
            channels: BTreeSet::from(["production".into(), "staging".into()]),
        }],
    }
}
fn state(repo: Arc<MemoryRepository>) -> AppState {
    AppState {
        config: Arc::new(Config {
            public_url: "https://updates.example.com/".parse().unwrap(),
            r2_endpoint: "https://example.r2.cloudflarestorage.com/".parse().unwrap(),
            apps: vec![app("alpha", 1), app("beta", 2)],
            max_artifact_bytes: 1024 * 1024,
            presign_ttl_seconds: 300,
        }),
        repository: repo,
        storage: Arc::new(TestStorage),
        draining: Arc::new(AtomicBool::new(false)),
        inflight: Arc::new(AtomicUsize::new(0)),
        instance: "test".into(),
    }
}
async fn published(repo: &MemoryRepository, app: &str, channel: &str, seed: u8) {
    let b = bundle(
        app,
        channel,
        "2.0.0",
        2,
        format!("{app}-package").as_bytes(),
        seed,
    );
    repo.draft(b).await.unwrap();
    repo.act(
        app,
        channel,
        "release-2",
        &AdminAction {
            operation_id: "publish-2".into(),
            action: "publish".into(),
            policy: None,
        },
    )
    .await
    .unwrap();
}
fn get(path: &str, token: Option<&str>) -> Request<Body> {
    let mut r = Request::builder().uri(path);
    if let Some(token) = token {
        r = r.header("Authorization", format!("Bearer {token}"));
    }
    r.body(Body::empty()).unwrap()
}
#[tokio::test]
async fn shared_bucket_keeps_apps_artifacts_and_permissions_separate() {
    let repo = Arc::new(MemoryRepository::default());
    published(&repo, "alpha", "production", 1).await;
    published(&repo, "beta", "production", 2).await;
    published(&repo, "beta", "staging", 2).await;
    let mut state = state(repo);
    for app in &mut Arc::make_mut(&mut state.config).apps {
        app.bucket = "shared-private".into();
    }
    state.config.validate().unwrap();
    let router = router(state);
    let a = router
        .clone()
        .oneshot(get("/v1/tauri/alpha/production/windows-x86_64/1.0.0", None))
        .await
        .unwrap();
    let b = router
        .clone()
        .oneshot(get("/v1/tauri/beta/production/windows-x86_64/1.0.0", None))
        .await
        .unwrap();
    assert_eq!(a.status(), StatusCode::OK);
    assert_eq!(b.status(), StatusCode::OK);
    let a: serde_json::Value =
        serde_json::from_slice(&to_bytes(a.into_body(), 100000).await.unwrap()).unwrap();
    let b: serde_json::Value =
        serde_json::from_slice(&to_bytes(b.into_body(), 100000).await.unwrap()).unwrap();
    assert_eq!(a["version"], b["version"]);
    assert_ne!(a["signature"], b["signature"]);
    assert_ne!(a["url"], b["url"]);
    assert_eq!(
        router
            .clone()
            .oneshot(get(
                "/v1/tauri/beta/staging/windows-x86_64/1.0.0",
                Some("alpha-read")
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        router
            .clone()
            .oneshot(get("/v1/tauri/beta/staging/windows-x86_64/1.0.0", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        router
            .oneshot(get(
                "/v1/tauri/beta/staging/windows-x86_64/1.0.0",
                Some("beta-read")
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
#[tokio::test]
async fn redirect_and_terminal_revoke() {
    let repo = Arc::new(MemoryRepository::default());
    published(&repo, "alpha", "production", 1).await;
    let router = router(state(repo.clone()));
    let path = "/v1/apps/alpha/channels/production/releases/2.0.0/targets/windows-x86_64/download";
    let r = router.clone().oneshot(get(path, None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(r.headers()["cache-control"], "no-store");
    assert!(
        r.headers()["location"]
            .to_str()
            .unwrap()
            .contains("/alpha-updates/alpha/")
    );
    repo.act(
        "alpha",
        "production",
        "release-2",
        &AdminAction {
            operation_id: "revoke-2".into(),
            action: "revoke".into(),
            policy: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        router
            .clone()
            .oneshot(get(path, None))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert!(
        repo.act(
            "alpha",
            "production",
            "release-2",
            &AdminAction {
                operation_id: "enable-2".into(),
                action: "enable".into(),
                policy: None
            }
        )
        .await
        .is_err()
    );
    assert_eq!(
        router
            .oneshot(get("/v1/tauri/alpha/production/windows-x86_64/1.0.0", None))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
}
#[tokio::test]
async fn admin_scope_and_idempotent_operation() {
    let repo = Arc::new(MemoryRepository::default());
    published(&repo, "beta", "production", 2).await;
    let router = router(state(repo.clone()));
    let body = serde_json::to_vec(&AdminAction {
        operation_id: "disable-1".into(),
        action: "disable".into(),
        policy: None,
    })
    .unwrap();
    let call = |token: &str| {
        Request::builder()
            .method("POST")
            .uri("/v1/admin/apps/beta/channels/production/releases/release-2?target=windows-x86_64")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(body.clone()))
            .unwrap()
    };
    assert_eq!(
        router
            .clone()
            .oneshot(call("alpha-admin"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        router
            .clone()
            .oneshot(call("beta-admin"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router
            .clone()
            .oneshot(call("beta-admin"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        repo.list("beta", "production", "windows-x86_64")
            .await
            .unwrap()[0]
            .lifecycle,
        Lifecycle::Disabled
    );
}
#[tokio::test]
async fn draft_signature_failure_does_not_replace_current_release() {
    let repo = Arc::new(MemoryRepository::default());
    published(&repo, "alpha", "production", 1).await;
    let router = router(state(repo));
    let mut b = bundle("alpha", "production", "3.0.0", 3, b"next", 1);
    b.manifest.body.version = "4.0.0".parse().unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/v1/admin/apps/alpha/channels/production/drafts")
        .header("Authorization", "Bearer alpha-admin")
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&DraftRequest { bundle: b }).unwrap(),
        ))
        .unwrap();
    assert_eq!(
        router.clone().oneshot(request).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let response = router
        .oneshot(get("/v1/tauri/alpha/production/windows-x86_64/1.0.0", None))
        .await
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 100000).await.unwrap()).unwrap();
    assert_eq!(value["version"], "2.0.0");
}
#[tokio::test]
async fn readiness_and_no_update_are_distinct() {
    let s = state(Arc::new(MemoryRepository::default()));
    let draining = s.draining.clone();
    let router = router(s);
    assert_eq!(
        router
            .clone()
            .oneshot(get("/health/ready", None))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    draining.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        router
            .clone()
            .oneshot(get("/health/ready", None))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        router
            .oneshot(get("/v1/tauri/alpha/production/windows-x86_64/2.0.0", None))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
}
