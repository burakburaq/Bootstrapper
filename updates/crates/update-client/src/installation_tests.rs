use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use update_protocol::fixtures::*;

const PACKAGE: &[u8] = b"signed NSIS fixture";
const PAYLOAD: &[u8] = b"final signed application fixture";
struct Platform {
    version: Mutex<Option<Version>>,
    registration: AtomicBool,
    installs: AtomicUsize,
    corrupt_stage: AtomicBool,
    locked: AtomicBool,
    corrupt_install: AtomicBool,
}
#[async_trait]
impl PlatformInstaller for Platform {
    async fn installed_version(&self, _: &InstallContext) -> Result<Option<Version>> {
        Ok(self.version.lock().unwrap().clone())
    }
    async fn preflight(
        &self,
        _: &InstallContext,
        _: &InstallContract,
        _: u64,
    ) -> Result<PreflightReport> {
        Ok(PreflightReport::default())
    }
    async fn inspect_package(
        &self,
        _: &Path,
        _: &InstallContract,
        _: &Version,
        _: &str,
    ) -> Result<()> {
        Ok(())
    }
    async fn stage(&self, _: &Path, destination: &Path) -> Result<()> {
        std::fs::create_dir_all(destination)?;
        std::fs::write(
            destination.join("app.exe"),
            if self.corrupt_stage.load(Ordering::SeqCst) {
                b"bad"
            } else {
                PAYLOAD
            },
        )?;
        Ok(())
    }
    async fn ensure_applications_closed(&self, _: &InstallContext) -> Result<()> {
        ensure!(
            !self.locked.load(Ordering::SeqCst),
            "running in another session"
        );
        Ok(())
    }
    async fn activate(&self, _: &Path, staging: &Path, context: &InstallContext) -> Result<()> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        std::fs::create_dir_all(&context.active_dir)?;
        std::fs::copy(staging.join("app.exe"), context.active_dir.join("app.exe"))?;
        if self.corrupt_install.load(Ordering::SeqCst) {
            std::fs::write(context.active_dir.join("app.exe"), b"corrupt")?;
        }
        *self.version.lock().unwrap() = Some("2.0.0".parse()?);
        self.registration.store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn verify_registration(&self, _: &InstallContext, _: &Version) -> Result<()> {
        ensure!(
            self.registration.load(Ordering::SeqCst),
            "registration absent"
        );
        Ok(())
    }
}
fn setup(installed: Option<&str>) -> (tempfile::TempDir, InstallEngine, Platform, ReleaseBundle) {
    let dir = tempfile::tempdir().unwrap();
    let context = InstallContext {
        app_id: "alpha".into(),
        channel: "stable".into(),
        target: "windows-x86_64".into(),
        installation_id: "machine-1".into(),
        active_dir: dir.path().join("active"),
        staging_root: dir.path().join("stage"),
        state_dir: dir.path().join("state"),
    };
    let engine =
        InstallEngine::new(context, keys(1), keys(2), package_signature(PACKAGE, 1).1).unwrap();
    let platform = Platform {
        version: Mutex::new(installed.map(|v| v.parse().unwrap())),
        registration: AtomicBool::new(false),
        installs: AtomicUsize::new(0),
        corrupt_stage: AtomicBool::new(false),
        locked: AtomicBool::new(false),
        corrupt_install: AtomicBool::new(false),
    };
    let mut b = bundle("alpha", "stable", "2.0.0", 20, PACKAGE, 1);
    b.manifest.body.installation = Some(InstallContract {
        schema_version: 1,
        executable: "app.exe".into(),
        publisher_certificate_sha256: "a".repeat(64),
        minimum_windows_build: 10240,
        minimum_webview2_version: [0; 4],
        staging_bytes: PAYLOAD.len() as u64,
        active_bytes: PAYLOAD.len() as u64,
        backup_bytes: PAYLOAD.len() as u64,
        files: vec![PayloadFile {
            path: "app.exe".into(),
            size_bytes: PAYLOAD.len() as u64,
            sha256: sha256(PAYLOAD),
        }],
    });
    resign(&mut b, 1);
    (dir, engine, platform, b)
}
fn permit(challenge: &InstallChallenge, b: &ReleaseBundle) -> Signed<InstallAuthorization> {
    let r = &challenge.request;
    let i = &r.installation;
    let m = &b.manifest.body;
    sign(
        InstallAuthorization {
            schema_version: 1,
            app_id: i.app_id.clone(),
            channel: i.channel.clone(),
            target: i.target.clone(),
            release_id: m.release_id.clone(),
            version: m.version.clone(),
            sequence: m.sequence,
            package_sha256: b.artifact(&m.full_artifact_id).unwrap().sha256.clone(),
            installation_id: i.installation_id.clone(),
            operation_id: r.operation_id.clone(),
            intent: i.intent,
            nonce: r.nonce.clone(),
            valid_for_seconds: 300,
        },
        "test".into(),
        &key(2),
    )
    .unwrap()
}
async fn begin(
    engine: &mut InstallEngine,
    p: &Platform,
    b: &ReleaseBundle,
    intent: InstallIntent,
) -> InstallChallenge {
    let BeginResult::Challenge(c) = engine
        .begin("operation-1", intent, b.clone(), p)
        .await
        .unwrap()
    else {
        panic!("expected challenge")
    };
    let (path, _) = engine.package_destination("operation-1").await.unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, PACKAGE).unwrap();
    *c
}
#[tokio::test]
async fn first_install_verifies_and_retries_return_existing_success() {
    let (_d, mut engine, p, b) = setup(None);
    let c = begin(&mut engine, &p, &b, InstallIntent::Install).await;
    assert!(c.request.installation.installed_version.is_none());
    let events = Mutex::new(Vec::new());
    let callback = |e| events.lock().unwrap().push(e);
    let auth = permit(&c, &b);
    assert_eq!(
        engine
            .commit("operation-1", auth.clone(), &p, &callback)
            .await
            .unwrap()
            .stage,
        Stage::Completed
    );
    assert_eq!(
        engine
            .commit("operation-1", auth, &p, &callback)
            .await
            .unwrap()
            .stage,
        Stage::Completed
    );
    assert_eq!(p.installs.load(Ordering::SeqCst), 1);
    assert_eq!(
        engine
            .pending_operation()
            .await
            .unwrap()
            .unwrap()
            .last_verified_release
            .as_ref(),
        Some(&b)
    );
    let stages: Vec<_> = events.lock().unwrap().iter().map(|e| e.stage).collect();
    assert!(
        stages.contains(&Stage::VerifyingPackage)
            && stages.contains(&Stage::VerifyingStaging)
            && stages.contains(&Stage::VerifyingInstallation)
    );
    assert!(
        engine
            .begin("operation-1", InstallIntent::Repair, b.clone(), &p)
            .await
            .is_err()
    );
    assert!(matches!(
        engine
            .begin("repair", InstallIntent::Repair, b, &p)
            .await
            .unwrap(),
        BeginResult::Existing(ProgressEvent {
            stage: Stage::NoOp,
            ..
        })
    ));
}
#[tokio::test]
async fn same_version_corruption_requires_signed_repair_and_registration() {
    let (_d, mut engine, p, b) = setup(Some("2.0.0"));
    assert!(
        engine
            .begin("update", InstallIntent::Update, b.clone(), &p)
            .await
            .is_err()
    );
    let c = begin(&mut engine, &p, &b, InstallIntent::Repair).await;
    engine
        .commit("operation-1", permit(&c, &b), &p, &|_| {})
        .await
        .unwrap();
    std::fs::write(engine.context.active_dir.join("app.exe"), b"bad").unwrap();
    assert!(matches!(
        engine
            .begin("repair-2", InstallIntent::Repair, b.clone(), &p)
            .await
            .unwrap(),
        BeginResult::Challenge(_)
    ));
    let mut lower = b;
    lower.manifest.body.sequence -= 1;
    resign(&mut lower, 1);
    assert!(
        engine
            .begin("lower", InstallIntent::Repair, lower, &p)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn nonce_expiry_restart_and_foreign_binding_fail_closed() {
    let (_d, mut engine, p, b) = setup(None);
    let c = begin(&mut engine, &p, &b, InstallIntent::Install).await;
    let mut forged = permit(&c, &b);
    forged.body.nonce = "b".repeat(64);
    forged = sign(forged.body, "test".into(), &key(2)).unwrap();
    assert!(
        engine
            .commit("operation-1", forged, &p, &|_| {})
            .await
            .is_err()
    );
    let auth = permit(&c, &b);
    engine.pending.get_mut("operation-1").unwrap().started -= Duration::from_secs(301);
    assert!(
        engine
            .commit("operation-1", auth.clone(), &p, &|_| {})
            .await
            .is_err()
    );
    engine.pending.clear();
    assert!(
        engine
            .commit("operation-1", auth, &p, &|_| {})
            .await
            .is_err()
    );
    assert_eq!(p.installs.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn bad_stage_running_process_and_bad_postinstall_never_succeed() {
    for failure in 0..3 {
        let (_d, mut engine, p, b) = setup(None);
        if failure == 0 {
            p.corrupt_stage.store(true, Ordering::SeqCst);
        }
        if failure == 1 {
            p.locked.store(true, Ordering::SeqCst);
        }
        if failure == 2 {
            p.corrupt_install.store(true, Ordering::SeqCst);
        }
        let c = begin(&mut engine, &p, &b, InstallIntent::Install).await;
        assert!(
            engine
                .commit("operation-1", permit(&c, &b), &p, &|_| {})
                .await
                .is_err()
        );
        assert_eq!(
            engine.query("operation-1").await.unwrap().unwrap().stage,
            Stage::Failed
        );
        if failure != 2 {
            assert_eq!(p.installs.load(Ordering::SeqCst), 0);
        }
    }
}
#[tokio::test]
async fn crash_reconcile_checks_files_and_registrations_not_only_version() {
    for healthy in [false, true] {
        let (_d, mut engine, p, b) = setup(None);
        let _c = begin(&mut engine, &p, &b, InstallIntent::Install).await;
        let mut j = InstallJournal::load(&engine.record("operation-1").unwrap())
            .await
            .unwrap();
        j.phase = InstallPhase::Installing;
        engine.save(&j, true).await.unwrap();
        *p.version.lock().unwrap() = Some("2.0.0".parse().unwrap());
        if healthy {
            std::fs::create_dir_all(&engine.context.active_dir).unwrap();
            std::fs::write(engine.context.active_dir.join("app.exe"), PAYLOAD).unwrap();
            p.registration.store(true, Ordering::SeqCst);
        }
        assert_eq!(
            engine.reconcile(&p).await.unwrap().unwrap().stage,
            if healthy {
                Stage::Completed
            } else {
                Stage::Interrupted
            }
        );
        assert_eq!(
            engine
                .pending_operation()
                .await
                .unwrap()
                .unwrap()
                .last_verified_release
                .is_some(),
            healthy
        );
    }
}
#[tokio::test]
async fn changed_package_and_skipped_checkpoint_cannot_activate() {
    let (_d, mut engine, p, mut b) = setup(Some("1.0.0"));
    b.manifest.body.required_checkpoints.push(Checkpoint {
        version: "1.5.0".parse().unwrap(),
        release_id: "checkpoint".into(),
    });
    resign(&mut b, 1);
    assert!(
        engine
            .begin("skip", InstallIntent::Update, b.clone(), &p)
            .await
            .is_err()
    );
    b.manifest.body.required_checkpoints.clear();
    resign(&mut b, 1);
    let c = begin(&mut engine, &p, &b, InstallIntent::Update).await;
    let (path, _) = engine.package_destination("operation-1").await.unwrap();
    std::fs::write(path, b"tampered").unwrap();
    assert!(
        engine
            .commit("operation-1", permit(&c, &b), &p, &|_| {})
            .await
            .is_err()
    );
    assert_eq!(p.installs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_update_and_retry_keep_last_verified_inventory_for_uninstall() {
    let (_d, mut engine, p, b) = setup(Some("1.0.0"));
    let mut old = bundle("alpha", "stable", "1.0.0", 10, PACKAGE, 1);
    let mut contract = b.manifest.body.installation.clone().unwrap();
    contract.files.push(PayloadFile {
        path: "obsolete.dll".into(),
        size_bytes: 3,
        sha256: sha256(b"old"),
    });
    contract.staging_bytes += 3;
    contract.active_bytes += 3;
    contract.backup_bytes += 3;
    old.manifest.body.installation = Some(contract);
    resign(&mut old, 1);
    // Simulate a journal written before last_verified_release was introduced.
    let previous = engine
        .make_journal(
            "old-install",
            InstallIntent::Install,
            old.clone(),
            Stage::NoOp,
            None,
        )
        .unwrap();
    engine.save(&previous, true).await.unwrap();
    p.locked.store(true, Ordering::SeqCst);
    let c = begin(&mut engine, &p, &b, InstallIntent::Update).await;
    assert!(
        engine
            .commit("operation-1", permit(&c, &b), &p, &|_| {})
            .await
            .is_err()
    );
    let failed = engine.pending_operation().await.unwrap().unwrap();
    assert_eq!(failed.target_version, "2.0.0".parse::<Version>().unwrap());
    assert_eq!(failed.sequence, 20);
    assert_eq!(failed.last_verified_release.as_ref(), Some(&old));
    assert_eq!(
        failed.uninstall_files(&keys(1)).unwrap(),
        vec!["app.exe", "obsolete.dll"]
    );
    engine
        .begin("retry", InstallIntent::Update, b.clone(), &p)
        .await
        .unwrap();
    let retry = InstallJournal::load(&engine.record("retry").unwrap())
        .await
        .unwrap();
    assert_eq!(retry.last_verified_release, Some(old.clone()));
    let mut forged = failed.clone();
    forged
        .last_verified_release
        .as_mut()
        .unwrap()
        .manifest
        .body
        .installation
        .as_mut()
        .unwrap()
        .files[1]
        .path = "user-data.db".into();
    assert!(forged.uninstall_files(&keys(1)).is_err());
    let mut foreign = old;
    foreign.manifest.body.channel = "other".into();
    resign(&mut foreign, 1);
    forged.last_verified_release = Some(foreign);
    assert!(forged.uninstall_files(&keys(1)).is_err());
}

#[tokio::test]
async fn authorization_precheck_does_not_consume_nonce_and_rechecks_at_commit() {
    let (_d, mut engine, p, b) = setup(None);
    let c = begin(&mut engine, &p, &b, InstallIntent::Install).await;
    let auth = permit(&c, &b);
    let mut wrong = auth.body.clone();
    wrong.nonce = "a".repeat(64);
    let wrong = sign(wrong, "test".into(), &key(2)).unwrap();
    assert!(
        engine
            .validate_authorization("operation-1", &wrong)
            .await
            .is_err()
    );
    engine
        .validate_authorization("operation-1", &auth)
        .await
        .unwrap();
    engine
        .validate_authorization("operation-1", &auth)
        .await
        .unwrap();
    assert_eq!(
        engine
            .commit("operation-1", auth.clone(), &p, &|_| {})
            .await
            .unwrap()
            .stage,
        Stage::Completed
    );
    assert!(
        engine
            .validate_authorization("operation-1", &auth)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn noop_records_verified_inventory_and_pending_uninstall_blocks_new_mutation() {
    let (_d, mut engine, p, b) = setup(Some("2.0.0"));
    assert!(!engine.installed_is_healthy(&p).await.unwrap());
    std::fs::create_dir_all(&engine.context.active_dir).unwrap();
    std::fs::write(engine.context.active_dir.join("app.exe"), PAYLOAD).unwrap();
    p.registration.store(true, Ordering::SeqCst);
    assert!(matches!(
        engine
            .begin("healthy", InstallIntent::Repair, b.clone(), &p)
            .await
            .unwrap(),
        BeginResult::Existing(_)
    ));
    let mut j = engine.pending_operation().await.unwrap().unwrap();
    assert_eq!(j.last_verified_release.as_ref(), Some(&b));
    let saved_before = std::fs::read(engine.head()).unwrap();
    assert!(engine.installed_is_healthy(&p).await.unwrap());
    assert_eq!(std::fs::read(engine.head()).unwrap(), saved_before);
    p.registration.store(false, Ordering::SeqCst);
    assert!(!engine.installed_is_healthy(&p).await.unwrap());
    p.registration.store(true, Ordering::SeqCst);
    std::fs::write(engine.context.active_dir.join("app.exe"), b"corrupt").unwrap();
    assert!(!engine.installed_is_healthy(&p).await.unwrap());
    std::fs::write(engine.context.active_dir.join("app.exe"), PAYLOAD).unwrap();
    j.uninstall_pending = true;
    j.phase = InstallPhase::Failed;
    j.operation.as_mut().unwrap().progress.stage = Stage::Interrupted;
    engine.save(&j, true).await.unwrap();
    assert!(!engine.installed_is_healthy(&p).await.unwrap());
    assert_eq!(
        engine.reconcile(&p).await.unwrap().unwrap().stage,
        Stage::Interrupted
    );
    assert!(
        engine
            .begin("repair-during-uninstall", InstallIntent::Repair, b, &p)
            .await
            .is_err()
    );
    let saved = engine.pending_operation().await.unwrap().unwrap();
    assert!(saved.uninstall_pending);
    assert_eq!(saved.target_version, j.target_version);
    assert_eq!(saved.sequence, j.sequence);
}

#[tokio::test]
async fn reinstall_attempt_does_not_inherit_retired_identity_or_launch_proof() {
    let (_d, mut engine, p, b) = setup(None);
    let mut removed = engine
        .make_journal(
            "removed",
            InstallIntent::Install,
            b.clone(),
            Stage::Uninstalled,
            None,
        )
        .unwrap();
    removed.phase = InstallPhase::Failed;
    removed.retired_installation_id = Some("retired-machine".into());
    engine.save(&removed, true).await.unwrap();
    engine
        .begin("reinstall", InstallIntent::Install, b, &p)
        .await
        .unwrap();
    let attempt = InstallJournal::load(&engine.record("reinstall").unwrap())
        .await
        .unwrap();
    assert!(attempt.retired_installation_id.is_none());
    assert!(attempt.last_verified_release.is_none());
    let floor = engine.pending_operation().await.unwrap().unwrap();
    assert_eq!(
        floor.retired_installation_id.as_deref(),
        Some("retired-machine")
    );
    assert_eq!(floor.sequence, removed.sequence);
    assert_eq!(floor.target_version, removed.target_version);
}

#[tokio::test]
async fn uninstall_during_upload_retires_an_already_authorized_challenge() {
    let (_d, mut engine, p, b) = setup(None);
    let challenge = begin(&mut engine, &p, &b, InstallIntent::Install).await;
    let auth = permit(&challenge, &b);
    engine
        .validate_authorization("operation-1", &auth)
        .await
        .unwrap();
    let mut removed = engine
        .make_journal(
            "removed",
            InstallIntent::Install,
            b.clone(),
            Stage::Uninstalled,
            None,
        )
        .unwrap();
    removed.phase = InstallPhase::Failed;
    removed.retired_installation_id = Some(engine.context.installation_id.clone());
    engine.save(&removed, true).await.unwrap();
    assert!(
        engine
            .validate_authorization("operation-1", &auth)
            .await
            .is_err()
    );
    assert!(
        engine
            .commit("operation-1", auth, &p, &|_| {})
            .await
            .is_err()
    );
    assert!(
        engine
            .begin("stale-identity", InstallIntent::Install, b, &p)
            .await
            .is_err()
    );
    assert_eq!(p.installs.load(Ordering::SeqCst), 0);
    assert_eq!(
        engine
            .pending_operation()
            .await
            .unwrap()
            .unwrap()
            .retired_installation_id,
        removed.retired_installation_id
    );
}
