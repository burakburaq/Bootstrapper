//! Offline privileged installation engine. No HTTP client or network calls.
#[cfg(test)]
#[path = "installation_tests.rs"]
mod tests;
use crate::{InstallJournal, InstallPhase, inventory, journal, progress::*};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use chrono::Utc;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use update_protocol::*;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationDetails {
    pub operation_id: String,
    pub intent: InstallIntent,
    pub progress: ProgressEvent,
    pub bundle: ReleaseBundle,
    pub installed_version_at_start: Option<Version>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstallChallenge {
    pub request: AuthorizationRequest,
    pub progress: ProgressEvent,
    pub preflight: PreflightReport,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PreflightReport {
    pub required_total_bytes: u64,
    pub available_staging_bytes: u64,
    pub available_active_bytes: u64,
    pub native_target: String,
    pub windows_build: Option<u32>,
    pub webview2_version: Option<[u32; 4]>,
    pub paths_writable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BeginResult {
    Challenge(Box<InstallChallenge>),
    Existing(ProgressEvent),
}

/// Paths are constructed by the privileged platform adapter from trusted known
/// folders. Never deserialize this context from IPC or WebView input.
pub struct InstallContext {
    pub app_id: String,
    pub channel: String,
    pub target: String,
    pub installation_id: String,
    pub active_dir: PathBuf,
    pub staging_root: PathBuf,
    pub state_dir: PathBuf,
}
#[async_trait]
pub trait PlatformInstaller: Send + Sync {
    /// Application startup holds a shared platform gate; activation takes it exclusively.
    async fn acquire_activation_guard(
        &self,
        _context: &InstallContext,
    ) -> Result<Box<dyn Send + Sync>> {
        Ok(Box::new(()))
    }
    async fn installed_version(&self, context: &InstallContext) -> Result<Option<Version>>;
    async fn preflight(
        &self,
        context: &InstallContext,
        contract: &InstallContract,
        package_bytes: u64,
    ) -> Result<PreflightReport>;
    async fn inspect_package(
        &self,
        package: &Path,
        contract: &InstallContract,
        version: &Version,
        target: &str,
    ) -> Result<()>;
    /// Must be stage-only: no active files, registry or service changes.
    async fn stage(&self, package: &Path, destination: &Path) -> Result<()>;
    /// Must consider all interactive sessions. Refuse mutation while any product process remains.
    async fn ensure_applications_closed(&self, context: &InstallContext) -> Result<()>;
    async fn activate(
        &self,
        package: &Path,
        staging: &Path,
        context: &InstallContext,
    ) -> Result<()>;
    async fn verify_registration(&self, context: &InstallContext, version: &Version) -> Result<()>;
}
struct Pending {
    challenge: InstallChallenge,
    started: Instant,
}
pub struct InstallEngine {
    context: InstallContext,
    metadata_keys: KeyRing,
    permit_keys: KeyRing,
    package_public_key: String,
    pending: BTreeMap<String, Pending>,
}
impl InstallEngine {
    pub fn new(
        context: InstallContext,
        metadata_keys: KeyRing,
        permit_keys: KeyRing,
        package_public_key: String,
    ) -> Result<Self> {
        identifier(&context.app_id)?;
        identifier(&context.installation_id)?;
        validate_trust(&metadata_keys, &package_public_key)?;
        validate_trust(&permit_keys, &package_public_key)?;
        ensure!(
            !permit_keys.is_empty(),
            "install authorization trust required"
        );
        for (id, key) in &permit_keys {
            identifier(id)?;
            ensure!(
                !metadata_keys.values().any(|v| v == key),
                "authorization and release keys must be separate"
            );
        }
        Ok(Self {
            context,
            metadata_keys,
            permit_keys,
            package_public_key,
            pending: BTreeMap::new(),
        })
    }
    pub fn context(&self) -> &InstallContext {
        &self.context
    }
    fn head(&self) -> PathBuf {
        self.context.state_dir.join("install.json")
    }
    fn record(&self, id: &str) -> Result<PathBuf> {
        identifier(id)?;
        Ok(self
            .context
            .state_dir
            .join("operations")
            .join(format!("{id}.json")))
    }
    pub async fn query(&self, id: &str) -> Result<Option<ProgressEvent>> {
        let path = self.record(id)?;
        if !path.exists() {
            return Ok(None);
        }
        let j = InstallJournal::load(&path).await?;
        Ok(Some(
            j.operation.context("missing operation details")?.progress,
        ))
    }
    pub async fn pending_operation(&self) -> Result<Option<InstallJournal>> {
        let path = self.head();
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(InstallJournal::load(&path).await?))
    }
    async fn save(&self, j: &InstallJournal, head: bool) -> Result<()> {
        let op = j.operation.as_ref().context("missing operation details")?;
        // Head carries the monotonic floor even if a crash interrupts the archive write.
        if head {
            j.save(&self.head()).await?;
        }
        j.save(&self.record(&op.operation_id)?).await
    }
    async fn change(
        &self,
        j: &mut InstallJournal,
        stage: Stage,
        callback: ProgressCallback<'_>,
    ) -> Result<()> {
        j.updated_at = Utc::now();
        let op = j.operation.as_mut().context("missing operation")?;
        op.progress = ProgressEvent::stage(&op.operation_id, op.intent, stage);
        let event = op.progress.clone();
        self.save(j, true).await?;
        callback(event);
        Ok(())
    }
    pub async fn installed_version(
        &self,
        platform: &dyn PlatformInstaller,
    ) -> Result<Option<Version>> {
        let actual = platform.installed_version(&self.context).await?;
        if actual.is_some() {
            return Ok(actual);
        }
        // A missing main EXE is a repairable installed product, not version 0.0.0.
        let previous = self.pending_operation().await?;
        match previous {
            Some(j) => {
                ensure!(
                    !j.uninstall_pending,
                    "interrupted uninstall must be resumed first"
                );
                let op = j
                    .operation
                    .context("legacy journal cannot establish installed identity")?;
                op.bundle.validate(&self.metadata_keys)?;
                if op.progress.stage == Stage::Uninstalled {
                    return Ok(None);
                }
                Ok(if j.phase == InstallPhase::Completed {
                    Some(j.target_version)
                } else {
                    op.installed_version_at_start
                })
            }
            None => Ok(None),
        }
    }
    pub async fn begin(
        &mut self,
        operation_id: &str,
        intent: InstallIntent,
        bundle: ReleaseBundle,
        platform: &dyn PlatformInstaller,
    ) -> Result<BeginResult> {
        identifier(operation_id)?;
        self.pending
            .retain(|_, p| p.started.elapsed() < Duration::from_secs(300));
        ensure!(
            self.pending.contains_key(operation_id) || self.pending.len() < 64,
            "too many pending install challenges"
        );
        bundle.validate(&self.metadata_keys)?;
        let m = &bundle.manifest.body;
        ensure!(
            m.app_id == self.context.app_id
                && m.channel == self.context.channel
                && m.target == self.context.target,
            "foreign installation bundle"
        );
        let contract = m
            .installation
            .as_ref()
            .context("signed installation contract required")?;
        let _lock = journal::lock(&self.head()).await?;
        let record = self.record(operation_id)?;
        let mut retry_version = None;
        if record.exists() {
            let j = InstallJournal::load(&record).await?;
            let op = j.operation.context("missing operation")?;
            ensure!(
                op.intent == intent && op.bundle == bundle,
                "operation ID reused with different intent/release"
            );
            if matches!(
                op.progress.stage,
                Stage::Completed | Stage::NoOp | Stage::Uninstalled
            ) {
                return Ok(BeginResult::Existing(op.progress));
            }
            if let Some(p) = self.pending.get(operation_id)
                && p.started.elapsed() < Duration::from_secs(300)
            {
                return Ok(BeginResult::Challenge(Box::new(p.challenge.clone())));
            }
            retry_version = Some(op.installed_version_at_start);
        }
        let actual = self.installed_version(platform).await?;
        ensure!(
            actual.as_ref().is_none_or(|v| v <= &m.version),
            "installed version newer than retry target"
        );
        let installed = retry_version.unwrap_or(actual);
        let installation = InstallRequest {
            app_id: m.app_id.clone(),
            channel: m.channel.clone(),
            target: m.target.clone(),
            installed_version: installed.clone(),
            installation_id: self.context.installation_id.clone(),
            intent,
        };
        installation.validate()?;
        ensure!(
            m.required_checkpoints
                .iter()
                .all(|c| installed.as_ref().is_none_or(|v| &c.version <= v))
                && m.minimum_supported_version
                    .as_ref()
                    .is_none_or(|minimum| installed.as_ref().is_none_or(|v| v >= minimum)),
            "required checkpoint or minimum upgrade version not satisfied"
        );
        ensure!(
            match intent {
                InstallIntent::Install => installed.is_none(),
                InstallIntent::Update => installed.as_ref().is_some_and(|v| v < &m.version),
                InstallIntent::Repair => installed.as_ref() == Some(&m.version),
            },
            "installation intent/version mismatch; downgrade refused"
        );
        self.check_floor(m, intent).await?;
        let preflight = platform
            .preflight(
                &self.context,
                contract,
                bundle
                    .artifact(&m.full_artifact_id)
                    .context("full")?
                    .size_bytes,
            )
            .await?;
        if intent == InstallIntent::Repair && self.verify_installed(&bundle, platform).await.is_ok()
        {
            let mut j = self.make_journal(
                operation_id,
                intent,
                bundle.clone(),
                Stage::NoOp,
                installed.clone(),
            )?;
            j.last_verified_release = Some(bundle.clone());
            self.save(&j, true).await?;
            return Ok(BeginResult::Existing(j.operation.expect("set").progress));
        }
        let full = bundle.artifact(&m.full_artifact_id).context("full")?;
        let nonce = sha256(format!("{}{}", Uuid::new_v4(), Uuid::new_v4()).as_bytes());
        let challenge = InstallChallenge {
            request: AuthorizationRequest {
                installation,
                release_id: m.release_id.clone(),
                operation_id: operation_id.into(),
                nonce,
            },
            progress: ProgressEvent::stage(operation_id, intent, Stage::AwaitingAuthorization),
            preflight,
        };
        ensure!(
            full.size_bytes <= 2 * 1024 * 1024 * 1024,
            "package too large"
        );
        let mut j = self.make_journal(
            operation_id,
            intent,
            bundle,
            Stage::AwaitingAuthorization,
            installed,
        )?;
        if let Some(previous) = self.pending_operation().await? {
            j.last_verified_release = previous.last_verified_release.or_else(|| {
                if previous.phase == InstallPhase::Completed {
                    previous.operation.map(|op| op.bundle)
                } else {
                    None
                }
            });
        }
        self.save(&j, false).await?;
        self.pending.insert(
            operation_id.into(),
            Pending {
                challenge: challenge.clone(),
                started: Instant::now(),
            },
        );
        Ok(BeginResult::Challenge(Box::new(challenge)))
    }
    pub async fn package_destination(&self, id: &str) -> Result<(PathBuf, u64)> {
        let j = InstallJournal::load(&self.record(id)?).await?;
        let op = j.operation.context("missing operation")?;
        ensure!(
            self.pending.contains_key(id),
            "operation has no live challenge"
        );
        let size = op
            .bundle
            .artifact(&op.bundle.manifest.body.full_artifact_id)
            .context("full")?
            .size_bytes;
        Ok((j.package_path, size))
    }
    fn make_journal(
        &self,
        id: &str,
        intent: InstallIntent,
        bundle: ReleaseBundle,
        stage: Stage,
        installed_version_at_start: Option<Version>,
    ) -> Result<InstallJournal> {
        let m = &bundle.manifest.body;
        let full = bundle.artifact(&m.full_artifact_id).context("full")?;
        Ok(InstallJournal {
            app_id: m.app_id.clone(),
            release_id: m.release_id.clone(),
            target_version: m.version.clone(),
            sequence: m.sequence,
            package_sha256: full.sha256.clone(),
            package_path: self.context.staging_root.join(id).join("package.exe"),
            phase: if stage == Stage::NoOp {
                InstallPhase::Completed
            } else {
                InstallPhase::Prepared
            },
            updated_at: Utc::now(),
            last_verified_release: None,
            uninstall_pending: false,
            retired_installation_id: None,
            operation: Some(OperationDetails {
                operation_id: id.into(),
                intent,
                progress: ProgressEvent::stage(id, intent, stage),
                bundle,
                installed_version_at_start,
            }),
        })
    }
    async fn check_floor(&self, m: &ReleaseManifest, intent: InstallIntent) -> Result<()> {
        if let Some(prev) = self.pending_operation().await? {
            ensure!(
                prev.retired_installation_id.as_deref()
                    != Some(self.context.installation_id.as_str()),
                "installation identity was retired; obtain a new challenge after reinstall bootstrap"
            );
            ensure!(
                !prev.uninstall_pending,
                "interrupted uninstall must be resumed first"
            );
            ensure!(
                prev.app_id == self.context.app_id,
                "foreign protected journal"
            );
            ensure!(
                !matches!(
                    prev.phase,
                    InstallPhase::Installing | InstallPhase::AwaitingRestart
                ),
                "reconcile interrupted operation first"
            );
            let newer = m.version > prev.target_version && m.sequence > prev.sequence;
            let same = m.version == prev.target_version
                && m.sequence == prev.sequence
                && m.release_id == prev.release_id
                && (intent == InstallIntent::Repair
                    || matches!(prev.phase, InstallPhase::Failed | InstallPhase::Prepared));
            ensure!(newer || same, "release below protected monotonic floor");
        }
        Ok(())
    }
    async fn verify_installed(
        &self,
        bundle: &ReleaseBundle,
        platform: &dyn PlatformInstaller,
    ) -> Result<()> {
        let m = &bundle.manifest.body;
        inventory::verify_inventory(
            &self.context.active_dir,
            m.installation.as_ref().context("contract")?,
        )?;
        ensure!(
            platform.installed_version(&self.context).await?.as_ref() == Some(&m.version),
            "installed EXE version differs"
        );
        platform
            .verify_registration(&self.context, &m.version)
            .await
    }
    /// Check the live nonce-bound permission before accepting privileged package bytes.
    /// Does not consume the challenge; commit rechecks it under the product lock.
    pub async fn validate_authorization(
        &self,
        operation_id: &str,
        permit: &Signed<InstallAuthorization>,
    ) -> Result<()> {
        let j = InstallJournal::load(&self.record(operation_id)?).await?;
        let op = j.operation.as_ref().context("missing operation")?;
        op.bundle.validate(&self.metadata_keys)?;
        verify(permit, &self.permit_keys)?;
        permit.body.validate()?;
        let pending = self
            .pending
            .get(operation_id)
            .context("no live authorization challenge; request a new one")?;
        let p = &permit.body;
        let c = &pending.challenge.request;
        let m = &op.bundle.manifest.body;
        ensure!(
            pending.started.elapsed() < Duration::from_secs(u64::from(p.valid_for_seconds))
                && p.nonce == c.nonce
                && p.app_id == m.app_id
                && p.channel == m.channel
                && p.target == m.target
                && p.release_id == m.release_id
                && p.version == m.version
                && p.sequence == m.sequence
                && p.package_sha256 == j.package_sha256
                && p.installation_id == self.context.installation_id
                && p.operation_id == operation_id
                && p.intent == op.intent,
            "expired/foreign install authorization"
        );
        self.check_floor(m, op.intent).await
    }
    /// `package` must already be copied into the service-owned operation directory.
    /// An arbitrary user cache path is never executed by this engine.
    pub async fn commit(
        &mut self,
        operation_id: &str,
        permit: Signed<InstallAuthorization>,
        platform: &dyn PlatformInstaller,
        callback: ProgressCallback<'_>,
    ) -> Result<ProgressEvent> {
        let _lock = journal::lock(&self.head()).await?;
        let mut j = InstallJournal::load(&self.record(operation_id)?).await?;
        let op = j.operation.as_ref().context("missing operation")?;
        if matches!(op.progress.stage, Stage::Completed | Stage::NoOp) {
            return Ok(op.progress.clone());
        }
        self.validate_authorization(operation_id, &permit).await?;
        // Other completed operations may have changed the last verified inventory
        // between this operation's begin and commit.
        if let Some(previous) = self.pending_operation().await? {
            j.last_verified_release = previous.last_verified_release.or_else(|| {
                if previous.phase == InstallPhase::Completed {
                    previous.operation.map(|op| op.bundle)
                } else {
                    None
                }
            });
        }
        self.pending.remove(operation_id); // single use, even when installation fails
        ensure!(
            platform
                .installed_version(&self.context)
                .await?
                .as_ref()
                .is_none_or(|v| v <= &j.target_version),
            "installed version changed to a newer release"
        );
        let result = self.install_inner(&mut j, platform, callback).await;
        if let Err(error) = result {
            j.phase = InstallPhase::Failed;
            self.change(&mut j, Stage::Failed, callback).await?;
            j.operation.as_mut().expect("operation").progress.error = Some(error.to_string());
            self.save(&j, true).await?;
            callback(j.operation.as_ref().expect("operation").progress.clone());
            return Err(error);
        }
        Ok(j.operation.expect("operation").progress)
    }
    async fn install_inner(
        &self,
        j: &mut InstallJournal,
        platform: &dyn PlatformInstaller,
        callback: ProgressCallback<'_>,
    ) -> Result<()> {
        let bundle = j.operation.as_ref().context("operation")?.bundle.clone();
        bundle.validate(&self.metadata_keys)?;
        let m = &bundle.manifest.body;
        let contract = m.installation.as_ref().context("contract")?;
        let full = bundle.artifact(&m.full_artifact_id).context("full")?;
        self.change(j, Stage::VerifyingPackage, callback).await?;
        let _guard = inventory::package_guard(&j.package_path)?;
        let bytes = tokio::fs::read(&j.package_path).await?;
        crate::verify_content(&bytes, full.size_bytes, &full.sha256)?;
        verify_package(&bytes, &full.package_signature, &self.package_public_key)?;
        drop(bytes);
        platform
            .inspect_package(&j.package_path, contract, &m.version, &m.target)
            .await?;
        self.change(j, Stage::Preflight, callback).await?;
        platform
            .preflight(&self.context, contract, full.size_bytes)
            .await?;
        let stage = j
            .package_path
            .parent()
            .context("operation directory")?
            .join("payload");
        ensure!(
            !stage.exists(),
            "staging payload already exists; reconcile/clean failed attempt first"
        );
        j.phase = InstallPhase::Installing;
        self.change(j, Stage::Staging, callback).await?;
        platform.stage(&j.package_path, &stage).await?;
        self.change(j, Stage::VerifyingStaging, callback).await?;
        inventory::verify_inventory(&stage, contract)?;
        self.change(j, Stage::ClosingApplications, callback).await?;
        let _runtime_guard = platform.acquire_activation_guard(&self.context).await?;
        platform.ensure_applications_closed(&self.context).await?;
        self.change(j, Stage::Installing, callback).await?;
        platform
            .activate(&j.package_path, &stage, &self.context)
            .await?;
        self.change(j, Stage::VerifyingInstallation, callback)
            .await?;
        self.verify_installed(&bundle, platform).await?;
        j.phase = InstallPhase::Completed;
        j.last_verified_release = Some(bundle.clone());
        self.change(j, Stage::Completed, callback).await?;
        drop(_guard);
        // Failure to remove a verified staging directory cannot change installation success.
        if let Some(dir) = j.package_path.parent() {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
        Ok(())
    }
    pub async fn reconcile(
        &self,
        platform: &dyn PlatformInstaller,
    ) -> Result<Option<ProgressEvent>> {
        let _lock = journal::lock(&self.head()).await?;
        let Some(mut j) = self.pending_operation().await? else {
            return Ok(None);
        };
        if j.uninstall_pending {
            return Ok(Some(j.operation.context("operation")?.progress));
        }
        let bundle = j
            .operation
            .as_ref()
            .context("legacy journal needs legacy reconciliation")?
            .bundle
            .clone();
        bundle.validate(&self.metadata_keys)?;
        if matches!(
            j.phase,
            InstallPhase::Installing | InstallPhase::AwaitingRestart
        ) {
            let complete = self.verify_installed(&bundle, platform).await.is_ok();
            j.phase = if complete {
                InstallPhase::Completed
            } else {
                InstallPhase::Failed
            };
            if complete {
                j.last_verified_release = Some(bundle.clone());
            }
            self.change(
                &mut j,
                if complete {
                    Stage::Completed
                } else {
                    Stage::Interrupted
                },
                &|_| {},
            )
            .await?;
        }
        Ok(Some(j.operation.context("operation")?.progress))
    }
}
