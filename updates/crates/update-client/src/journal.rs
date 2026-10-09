use anyhow::{Result, ensure};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InstallPhase {
    Prepared,
    Installing,
    AwaitingRestart,
    Completed,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstallJournal {
    pub app_id: String,
    pub release_id: String,
    pub target_version: Version,
    pub sequence: u64,
    pub package_sha256: String,
    pub package_path: PathBuf,
    pub phase: InstallPhase,
    pub updated_at: DateTime<Utc>,
    /// Last inventory that passed all installed-file and registration checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_verified_release: Option<update_protocol::ReleaseBundle>,
    /// Written before uninstall changes any files; retries keep the monotonic floor.
    #[serde(default)]
    pub uninstall_pending: bool,
    /// A completed full uninstall retires this rollout identity permanently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_installation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<crate::installation::OperationDetails>,
}
impl InstallJournal {
    #[cfg(any(windows, test))]
    pub(crate) fn uninstall_files(&self, keys: &update_protocol::KeyRing) -> Result<Vec<String>> {
        let current = &self
            .operation
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("installation contract unavailable"))?
            .bundle;
        current.validate(keys)?;
        let manifest = &current.manifest.body;
        ensure!(
            manifest.app_id == self.app_id,
            "foreign installation inventory"
        );
        let identity = manifest
            .installation
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("inventory unavailable"))?;
        let mut files = std::collections::BTreeMap::new();
        for bundle in std::iter::once(current).chain(self.last_verified_release.iter()) {
            bundle.validate(keys)?;
            let m = &bundle.manifest.body;
            let contract = m
                .installation
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("verified inventory unavailable"))?;
            ensure!(
                m.app_id == manifest.app_id
                    && m.channel == manifest.channel
                    && m.target == manifest.target
                    && contract.executable == identity.executable
                    && contract.publisher_certificate_sha256
                        == identity.publisher_certificate_sha256,
                "foreign last verified installation inventory"
            );
            for file in &contract.files {
                files
                    .entry(file.path.to_ascii_lowercase())
                    .or_insert_with(|| file.path.clone());
            }
        }
        Ok(files.into_values().collect())
    }
    pub async fn save(&self, path: &Path) -> Result<()> {
        atomic_write(path, &serde_json::to_vec_pretty(self)?).await
    }
    pub async fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&tokio::fs::read(path).await?)?)
    }
    pub async fn reconcile(&mut self, installed: &Version, path: &Path) -> Result<bool> {
        ensure!(
            self.operation.is_none(),
            "inventory-backed installations require InstallEngine::reconcile"
        );
        let _lock = lock(path).await?;
        let saved = Self::load(path).await?;
        ensure!(
            saved.operation.is_none(),
            "inventory-backed installations require InstallEngine::reconcile"
        );
        ensure!(
            saved.app_id == self.app_id
                && saved.release_id == self.release_id
                && saved.sequence == self.sequence,
            "journal changed before reconciliation"
        );
        self.phase = saved.phase;
        if self.phase == InstallPhase::Completed {
            return Ok(installed == &self.target_version);
        }
        self.phase = if installed == &self.target_version {
            InstallPhase::Completed
        } else {
            InstallPhase::Failed
        };
        self.updated_at = Utc::now();
        self.save(path).await?;
        Ok(self.phase == InstallPhase::Completed)
    }
}

/// Application/platform adapter owns publisher checks, actual product-version inspection,
/// installer execution and reading the installed product version. No Tauri app dependency.
#[async_trait]
pub trait Installer: Send + Sync {
    async fn inspect_version_and_publisher(&self, package: &Path) -> Result<Version>;
    async fn install(&self, package: &Path) -> Result<()>;
    async fn installed_version(&self) -> Result<Version>;
}

pub(crate) async fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;
    let temp = parent.join(format!(".update-{}.tmp", Uuid::new_v4()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    // rename replaces existing regular files on supported Windows/Unix platforms.
    if let Err(error) = tokio::fs::rename(&temp, path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error.into());
    }
    Ok(())
}

pub(crate) async fn lock(path: &Path) -> Result<std::fs::File> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("journal filename required"))?
        .to_string_lossy();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(format!("{name}.lock")))?;
    // OS lock is released even if the process crashes. The persistent lock file is harmless.
    file.try_lock().map_err(|_| {
        anyhow::anyhow!("another process is installing/reconciling this application")
    })?;
    Ok(file)
}

pub(crate) async fn check_floor(
    path: &Path,
    app_id: &str,
    release_id: &str,
    version: &Version,
    sequence: u64,
) -> Result<()> {
    match InstallJournal::load(path).await {
        Ok(previous) => {
            ensure!(
                previous.app_id == app_id,
                "journal belongs to another application"
            );
            ensure!(
                !matches!(
                    previous.phase,
                    InstallPhase::Installing | InstallPhase::AwaitingRestart
                ),
                "interrupted/pending installation must be reconciled first"
            );
            let newer = version > &previous.target_version && sequence > previous.sequence;
            let retry = matches!(
                previous.phase,
                InstallPhase::Failed | InstallPhase::Prepared
            ) && version == &previous.target_version
                && sequence == previous.sequence
                && release_id == previous.release_id;
            ensure!(
                newer || retry,
                "rollback/replayed release rejected, including after a failed attempt"
            );
        }
        Err(_) if !path.exists() => (),
        Err(error) => return Err(error.context("unreadable install journal")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(phase: InstallPhase) -> InstallJournal {
        InstallJournal {
            app_id: "alpha".into(),
            release_id: "release-2".into(),
            target_version: "2.0.0".parse().unwrap(),
            sequence: 20,
            package_sha256: "a".repeat(64),
            package_path: "package".into(),
            phase,
            updated_at: Utc::now(),
            last_verified_release: None,
            uninstall_pending: false,
            retired_installation_id: None,
            operation: None,
        }
    }
    #[tokio::test]
    async fn failed_attempt_keeps_monotonic_floor_and_allows_only_exact_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        state(InstallPhase::Failed).save(&path).await.unwrap();
        assert!(
            check_floor(&path, "alpha", "release-3", &"3.0.0".parse().unwrap(), 15)
                .await
                .is_err()
        );
        assert!(
            check_floor(&path, "alpha", "release-1", &"1.5.0".parse().unwrap(), 25)
                .await
                .is_err()
        );
        assert!(
            check_floor(
                &path,
                "alpha",
                "different-release",
                &"2.0.0".parse().unwrap(),
                20
            )
            .await
            .is_err()
        );
        check_floor(&path, "alpha", "release-2", &"2.0.0".parse().unwrap(), 20)
            .await
            .unwrap();
        check_floor(&path, "alpha", "release-3", &"3.0.0".parse().unwrap(), 30)
            .await
            .unwrap();
    }
    #[test]
    fn old_journal_defaults_new_inventory_and_uninstall_fields() {
        let mut value = serde_json::to_value(state(InstallPhase::Completed)).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("last_verified_release");
        value.as_object_mut().unwrap().remove("uninstall_pending");
        value
            .as_object_mut()
            .unwrap()
            .remove("retired_installation_id");
        let parsed: InstallJournal = serde_json::from_value(value).unwrap();
        assert!(parsed.last_verified_release.is_none());
        assert!(!parsed.uninstall_pending);
        assert!(parsed.retired_installation_id.is_none());
    }
    #[tokio::test]
    async fn interrupted_install_requires_actual_version_reconciliation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut journal = state(InstallPhase::Installing);
        journal.save(&path).await.unwrap();
        assert!(
            check_floor(&path, "alpha", "release-2", &"2.0.0".parse().unwrap(), 20)
                .await
                .is_err()
        );
        assert!(
            !journal
                .reconcile(&"1.0.0".parse().unwrap(), &path)
                .await
                .unwrap()
        );
        assert_eq!(journal.phase, InstallPhase::Failed);
        check_floor(&path, "alpha", "release-2", &"2.0.0".parse().unwrap(), 20)
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn concurrent_journal_lock_is_rejected_and_released_by_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let first = lock(&path).await.unwrap();
        assert!(lock(&path).await.is_err());
        drop(first);
        assert!(lock(&path).await.is_ok());
    }
    #[tokio::test]
    async fn stale_reconciliation_does_not_overwrite_newer_installation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut stale = state(InstallPhase::Installing);
        let mut newer = stale.clone();
        newer.release_id = "release-3".into();
        newer.sequence = 30;
        newer.target_version = "3.0.0".parse().unwrap();
        newer.save(&path).await.unwrap();
        assert!(
            stale
                .reconcile(&"2.0.0".parse().unwrap(), &path)
                .await
                .is_err()
        );
        assert_eq!(InstallJournal::load(&path).await.unwrap().sequence, 30);
    }
}
