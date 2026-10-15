use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use update_client::{installation::InstallChallenge, update_protocol::*};
use update_protocol::fixtures::*;

struct Service {
    version: Option<Version>,
    terminal: Stage,
    begins: AtomicUsize,
    commits: AtomicUsize,
    pending: Option<ProgressEvent>,
}
#[async_trait]
impl InstallationService for Service {
    async fn context(&self) -> anyhow::Result<(Option<Version>, String)> {
        Ok((self.version.clone(), "persistent-machine-id".into()))
    }
    async fn pending(&self) -> anyhow::Result<Option<ProgressEvent>> {
        Ok(self.pending.clone())
    }
    async fn begin(
        &self,
        update: &PreparedUpdate,
        id: &str,
        intent: InstallIntent,
    ) -> anyhow::Result<BeginResult> {
        self.begins.fetch_add(1, Ordering::SeqCst);
        if self.terminal == Stage::NoOp {
            return Ok(BeginResult::Existing(ProgressEvent::stage(
                id,
                intent,
                Stage::NoOp,
            )));
        }
        Ok(BeginResult::Challenge(Box::new(InstallChallenge {
            request: AuthorizationRequest {
                installation: InstallRequest {
                    app_id: crate::product::APP_ID.into(),
                    channel: "production".into(),
                    target: "windows-x86_64".into(),
                    installed_version: self.version.clone(),
                    installation_id: "persistent-machine-id".into(),
                    intent,
                },
                release_id: update.bundle().manifest.body.release_id.clone(),
                operation_id: id.into(),
                nonce: "a".repeat(64),
            },
            progress: ProgressEvent::stage(id, intent, Stage::AwaitingAuthorization),
            preflight: PreflightReport {
                paths_writable: true,
                ..Default::default()
            },
        })))
    }
    async fn commit(
        &self,
        _: &PreparedUpdate,
        id: &str,
        permit: Signed<InstallAuthorization>,
        callback: ProgressCallback<'_>,
    ) -> anyhow::Result<ProgressEvent> {
        verify(&permit, &keys(2))?;
        anyhow::ensure!(permit.body.operation_id == id);
        self.commits.fetch_add(1, Ordering::SeqCst);
        callback(ProgressEvent::stage(
            id,
            permit.body.intent,
            Stage::Installing,
        ));
        callback(ProgressEvent::stage(
            id,
            permit.body.intent,
            Stage::VerifyingInstallation,
        ));
        Ok(ProgressEvent::stage(id, permit.body.intent, self.terminal))
    }
}
struct Fixture {
    bridge: UpdateBridge,
    service: Arc<Service>,
    _directory: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn fixture(version: Option<&str>, terminal: Stage, corrupt: bool, pending: bool) -> Fixture {
    use axum::{
        Json, Router,
        http::{StatusCode, header},
        response::IntoResponse,
        routing::{get, post},
    };
    let bytes = b"signed installer fixture".to_vec();
    let mut bundle = bundle(crate::product::APP_ID, "production", "2.0.0", 2, &bytes, 1);
    bundle.manifest.body.installation = Some(InstallContract {
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
            sha256: sha256(b"x"),
        }],
    });
    resign(&mut bundle, 1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let prefix = format!(
        "/v1/apps/{}/channels/production/releases/2.0.0/targets/windows-x86_64",
        crate::product::APP_ID
    );
    let artifact = &bundle.artifacts[0].body;
    let route = format!("{prefix}/artifacts/{}/download", artifact.artifact_id);
    let download_url = format!("{origin}{route}");
    let response = UpdateResponse {
        schema_version: 1,
        app_id: crate::product::APP_ID.into(),
        channel: "production".into(),
        target: "windows-x86_64".into(),
        release_id: "release-2".into(),
        version: "2.0.0".parse().unwrap(),
        published_at: bundle.manifest.body.published_at,
        notes: String::new(),
        download_url: download_url.clone(),
        signature: artifact.package_signature.clone(),
        signing_key_id: "test".into(),
        sha256: artifact.sha256.clone(),
        size_bytes: bytes.len() as u64,
        minimum_supported_version: None,
        download_urls: [(artifact.artifact_id.clone(), download_url)].into(),
        selected_artifact_id: artifact.artifact_id.clone(),
        status_url: format!("{origin}{prefix}/status"),
        bundle: bundle.clone(),
    };
    let object_url = format!("{origin}/object");
    let permit_bundle = bundle;
    let router = Router::new()
        .route(
            "/v1/install/resolve",
            post(move |Json(request): Json<InstallRequest>| {
                let response = response.clone();
                async move {
                    assert_eq!(request.installation_id, "persistent-machine-id");
                    if request.intent == InstallIntent::Update
                        && request.installed_version.as_ref() == Some(&response.version)
                    {
                        StatusCode::NO_CONTENT.into_response()
                    } else {
                        Json(response).into_response()
                    }
                }
            }),
        )
        .route(
            &route,
            get(move || {
                let location = object_url.clone();
                async move {
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(header::LOCATION, location)],
                    )
                }
            }),
        )
        .route(
            "/object",
            get(move || {
                let body = if corrupt {
                    vec![0; bytes.len()]
                } else {
                    bytes.clone()
                };
                async move { body }
            }),
        )
        .route(
            "/v1/install/authorize",
            post(move |Json(request): Json<AuthorizationRequest>| {
                let bundle = permit_bundle.clone();
                async move {
                    let m = &bundle.manifest.body;
                    Json(
                        sign(
                            InstallAuthorization {
                                schema_version: 1,
                                app_id: m.app_id.clone(),
                                channel: m.channel.clone(),
                                target: m.target.clone(),
                                release_id: m.release_id.clone(),
                                version: m.version.clone(),
                                sequence: m.sequence,
                                package_sha256: bundle.artifacts[0].body.sha256.clone(),
                                installation_id: request.installation.installation_id,
                                operation_id: request.operation_id,
                                intent: request.installation.intent,
                                nonce: request.nonce,
                                valid_for_seconds: 300,
                            },
                            "test".into(),
                            &key(2),
                        )
                        .unwrap(),
                    )
                }
            }),
        );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let client = UpdateClient::new(ClientConfig {
        server_url: origin.parse().unwrap(),
        app_id: crate::product::APP_ID.into(),
        channel: "production".into(),
        target: "windows-x86_64".into(),
        metadata_keys: keys(1),
        package_public_key: package_signature(b"", 1).1,
        channel_token: None,
        allowed_download_hosts: ["127.0.0.1".into()].into(),
        cache_dir: directory.path().join("cache"),
        max_artifact_bytes: 1024 * 1024,
        allow_loopback_http: true,
    })
    .unwrap();
    let service = Arc::new(Service {
        version: version.map(|v| v.parse().unwrap()),
        terminal,
        begins: AtomicUsize::new(0),
        commits: AtomicUsize::new(0),
        pending: pending.then(|| {
            ProgressEvent::stage(
                "older-operation",
                InstallIntent::Install,
                Stage::Interrupted,
            )
        }),
    });
    Fixture {
        bridge: UpdateBridge::from_parts(Some(client), Some(service.clone())),
        service,
        _directory: directory,
        task,
    }
}
#[tokio::test]
async fn unconfigured_cannot_install_and_stale_id_is_rejected() {
    let bridge = UpdateBridge::from_parts(None, None);
    let id = bridge.operation_id.clone();
    assert_eq!(
        bridge.install("stale").await.unwrap_err(),
        "operation_mismatch"
    );
    assert_eq!(
        bridge.install(&id).await.unwrap_err(),
        "update_not_configured"
    );
    assert_eq!(bridge.status().unwrap().stage, "unconfigured");
    assert!(!bridge.status().unwrap().capabilities.install);
}
#[tokio::test]
async fn first_install_uses_real_identity_and_authorization_then_is_idempotent() {
    let f = fixture(None, Stage::Completed, false, false).await;
    assert_eq!(
        f.bridge.refresh().await.unwrap().intent,
        Some(InstallIntent::Install)
    );
    let id = f.bridge.operation_id.clone();
    let status = f.bridge.install(&id).await.unwrap();
    assert_eq!(status.stage, "completed");
    assert_eq!(status.install_percent, Some(100));
    assert!(status.preflight.unwrap().paths_writable);
    f.bridge.install(&id).await.unwrap();
    assert_eq!(f.service.begins.load(Ordering::SeqCst), 1);
    assert_eq!(f.service.commits.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn current_release_resolves_repair_and_healthy_noop_never_commits() {
    let f = fixture(Some("2.0.0"), Stage::NoOp, false, false).await;
    assert_eq!(
        f.bridge.refresh().await.unwrap().intent,
        Some(InstallIntent::Repair)
    );
    assert_eq!(
        f.bridge
            .install(&f.bridge.operation_id)
            .await
            .unwrap()
            .stage,
        "no_op"
    );
    assert_eq!(f.service.commits.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn old_release_resolves_update() {
    let f = fixture(Some("1.0.0"), Stage::Completed, false, false).await;
    assert_eq!(
        f.bridge.refresh().await.unwrap().intent,
        Some(InstallIntent::Update)
    );
}
#[tokio::test]
async fn corrupt_package_never_reaches_service() {
    let f = fixture(None, Stage::Completed, true, false).await;
    f.bridge.refresh().await.unwrap();
    assert!(f.bridge.install(&f.bridge.operation_id).await.is_err());
    assert_eq!(f.bridge.status().unwrap().stage, "failed");
    assert_eq!(f.service.begins.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn service_failure_is_not_completion() {
    let f = fixture(None, Stage::Failed, false, false).await;
    f.bridge.refresh().await.unwrap();
    assert!(f.bridge.install(&f.bridge.operation_id).await.is_err());
    assert_ne!(f.bridge.status().unwrap().install_percent, Some(100));
}
#[tokio::test]
async fn interrupted_prior_operation_blocks_fresh_install() {
    let f = fixture(None, Stage::Completed, false, true).await;
    assert_eq!(f.bridge.refresh().await.unwrap().stage, "blocked");
    assert!(f.bridge.install(&f.bridge.operation_id).await.is_err());
}
#[tokio::test]
async fn unknown_percentage_and_foreign_events_are_not_fabricated() {
    let bridge = UpdateBridge::from_parts(None, None);
    bridge.progress(ProgressEvent::stage(
        "foreign",
        InstallIntent::Install,
        Stage::Completed,
    ));
    assert_eq!(bridge.status().unwrap().stage, "unconfigured");
    bridge.progress(ProgressEvent::stage(
        &bridge.operation_id,
        InstallIntent::Install,
        Stage::Installing,
    ));
    assert!(bridge.status().unwrap().install_percent.is_none());
}

#[test]
fn readiness_retries_only_missing_or_busy_pipe_not_authentication_failures() {
    for code in [2, 231] {
        let error = anyhow::Error::new(std::io::Error::from_raw_os_error(code))
            .context("IPC pipe open failed");
        assert!(service_is_starting(&error));
    }
    for code in [5, 109, 233] {
        let error = anyhow::Error::new(std::io::Error::from_raw_os_error(code))
            .context("IPC pipe open failed");
        assert!(!service_is_starting(&error));
    }
    let missing_signed_file = anyhow::Error::new(std::io::Error::from_raw_os_error(2))
        .context("IPC server Authenticode verification failed");
    assert!(!service_is_starting(&missing_signed_file));
}
