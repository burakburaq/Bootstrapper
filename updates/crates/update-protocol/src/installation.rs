//! Installer contracts are part of the offline-signed release, never UI input.
use crate::*;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallIntent {
    Install,
    Update,
    Repair,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PayloadFile {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Fixed application files only: no user data, caches, mutable databases or logs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InstallContract {
    pub schema_version: u32,
    pub executable: String,
    pub publisher_certificate_sha256: String,
    pub minimum_windows_build: u32,
    pub minimum_webview2_version: [u32; 4],
    pub staging_bytes: u64,
    pub active_bytes: u64,
    pub backup_bytes: u64,
    pub files: Vec<PayloadFile>,
}
pub fn relative_payload_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 240
        || !path.is_ascii()
        || path.contains('\\')
        || path.split('/').any(|p| {
            let stem = p.split('.').next().unwrap_or("").to_ascii_uppercase();
            p.is_empty()
                || p == "."
                || p == ".."
                || p.ends_with(['.', ' '])
                || !p
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_. ".contains(&c))
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.as_bytes()[3].is_ascii_digit())
        })
    {
        return Err(ProtocolError::Invalid("unsafe payload path".into()));
    }
    Ok(())
}
impl InstallContract {
    pub fn validate(&self) -> Result<()> {
        hash(&self.publisher_certificate_sha256)?;
        relative_payload_path(&self.executable)?;
        let mut paths = BTreeSet::new();
        let mut total = 0u64;
        for file in &self.files {
            relative_payload_path(&file.path)?;
            hash(&file.sha256)?;
            if !paths.insert(file.path.to_ascii_lowercase()) {
                return Err(ProtocolError::Invalid(
                    "duplicate/case-aliased payload file".into(),
                ));
            }
            total = total
                .checked_add(file.size_bytes)
                .ok_or_else(|| ProtocolError::Invalid("payload size overflow".into()))?;
        }
        // File/directory aliases must also be rejected on Windows.
        for path in &paths {
            let mut prefix = String::new();
            for part in path.split('/').take(path.split('/').count() - 1) {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                if paths.contains(&prefix) {
                    return Err(ProtocolError::Invalid(
                        "payload file/directory alias".into(),
                    ));
                }
            }
        }
        if self.schema_version != 1
            || self.minimum_windows_build < 10240
            || self.files.is_empty()
            || self.files.len() > 8192
            || !self.files.iter().any(|f| f.path == self.executable)
            || self.staging_bytes < total
            || self.active_bytes < total
            || self.backup_bytes < total
            || self
                .staging_bytes
                .checked_add(self.active_bytes)
                .and_then(|v| v.checked_add(self.backup_bytes))
                .is_none()
        {
            return Err(ProtocolError::Invalid(
                "invalid installation contract/budget".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    pub app_id: String,
    pub channel: String,
    pub target: String,
    pub installed_version: Option<Version>,
    pub installation_id: String,
    pub intent: InstallIntent,
}
impl InstallRequest {
    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.app_id,
            &self.channel,
            &self.target,
            &self.installation_id,
        ] {
            identifier(id)?;
        }
        if matches!(
            (self.intent, &self.installed_version),
            (InstallIntent::Install, Some(_))
                | (InstallIntent::Update | InstallIntent::Repair, None)
        ) || self
            .installed_version
            .as_ref()
            .is_some_and(|v| !v.build.is_empty())
        {
            return Err(ProtocolError::Invalid(
                "intent/installed version mismatch".into(),
            ));
        }
        Ok(())
    }
}

pub fn resolve_install<'a>(
    records: &'a [ReleaseRecord],
    request: &InstallRequest,
) -> Result<Option<(&'a ReleaseRecord, String)>> {
    request.validate()?;
    if request.intent == InstallIntent::Update {
        return resolve(
            records,
            &CheckRequest {
                app_id: request.app_id.clone(),
                channel: request.channel.clone(),
                target: request.target.clone(),
                current_version: request.installed_version.clone().expect("validated"),
                installation_id: Some(request.installation_id.clone()),
                supports_delta: false,
                base_sha256: None,
                base_size_bytes: None,
                schema_version: SCHEMA_VERSION,
            },
        )
        .map(|v| v.filter(|(r, _)| r.bundle.manifest.body.installation.is_some()));
    }
    let chosen = records
        .iter()
        .filter(|r| {
            let m = &r.bundle.manifest.body;
            r.lifecycle == Lifecycle::Published
                && !r.policy.paused
                && m.app_id == request.app_id
                && m.channel == request.channel
                && m.target == request.target
                && m.installation.is_some()
                && match request.intent {
                    InstallIntent::Repair => Some(&m.version) == request.installed_version.as_ref(),
                    InstallIntent::Install => {
                        eligible(&request.app_id, &r.policy, Some(&request.installation_id))
                    }
                    InstallIntent::Update => false,
                }
        })
        .max_by(|a, b| {
            a.bundle
                .manifest
                .body
                .version
                .cmp(&b.bundle.manifest.body.version)
        });
    Ok(chosen.map(|r| (r, r.bundle.manifest.body.full_artifact_id.clone())))
}

/// The helper starts a monotonic deadline BEFORE requesting this nonce-bound signature.
/// No timestamp from either machine is used to establish freshness.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InstallAuthorization {
    pub schema_version: u32,
    pub app_id: String,
    pub channel: String,
    pub target: String,
    pub release_id: String,
    pub version: Version,
    pub sequence: u64,
    pub package_sha256: String,
    pub installation_id: String,
    pub operation_id: String,
    pub intent: InstallIntent,
    pub nonce: String,
    pub valid_for_seconds: u32,
}
impl Signable for InstallAuthorization {
    const DOMAIN: &'static str = "install-authorization";
}
impl InstallAuthorization {
    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.app_id,
            &self.channel,
            &self.target,
            &self.release_id,
            &self.installation_id,
            &self.operation_id,
        ] {
            identifier(id)?;
        }
        hash(&self.nonce)?;
        hash(&self.package_sha256)?;
        if self.schema_version != 1 || self.sequence == 0 || self.valid_for_seconds != 300 {
            return Err(ProtocolError::Invalid(
                "invalid installation authorization".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationRequest {
    pub installation: InstallRequest,
    pub release_id: String,
    pub operation_id: String,
    pub nonce: String,
}
