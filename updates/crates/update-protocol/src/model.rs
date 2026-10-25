use crate::{
    KeyRing, ProtocolError, Result, Signable, Signed, canonical, hash, identifier, sha256, verify,
};
use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactKind {
    Full,
    Delta {
        base_sha256: String,
        base_size_bytes: u64,
        result_sha256: String,
        result_size_bytes: u64,
        full_artifact_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub schema_version: u32,
    pub app_id: String,
    pub target: String,
    pub artifact_id: String,
    pub version: Version,
    pub object_key: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub artifact: ArtifactKind,
    /// Tauri .sig contents; for deltas this is the reconstructed full package signature.
    pub package_signature: String,
}
impl Signable for ArtifactDescriptor {
    const DOMAIN: &'static str = "artifact";
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub version: Version,
    pub release_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub app_id: String,
    pub channel: String,
    pub target: String,
    pub release_id: String,
    pub version: Version,
    pub sequence: u64,
    pub published_at: DateTime<Utc>,
    pub notes: String,
    pub minimum_supported_version: Option<Version>,
    pub required_checkpoints: Vec<Checkpoint>,
    pub full_artifact_id: String,
    /// SHA-256 of each descriptor's JCS body, indexed by artifact ID.
    pub artifacts: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation: Option<crate::InstallContract>,
}
impl Signable for ReleaseManifest {
    const DOMAIN: &'static str = "release";
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseBundle {
    pub manifest: Signed<ReleaseManifest>,
    pub artifacts: Vec<Signed<ArtifactDescriptor>>,
}
impl ReleaseBundle {
    pub fn validate(&self, keys: &KeyRing) -> Result<()> {
        verify(&self.manifest, keys)?;
        let m = &self.manifest.body;
        if let Some(contract) = &m.installation {
            contract.validate()?;
            if !matches!(
                m.target.as_str(),
                "windows-x86_64"
                    | "windows-aarch64"
                    | "windows-x86_64-nsis"
                    | "windows-aarch64-nsis"
            ) {
                return invalid("Windows installation contract on a different target");
            }
        }
        for id in [&m.app_id, &m.channel, &m.target, &m.release_id] {
            identifier(id)?;
        }
        if m.schema_version != SCHEMA_VERSION
            || m.sequence == 0
            || m.sequence > 9_007_199_254_740_991
            || self.artifacts.is_empty()
            || self.artifacts.len() > 64
            || m.required_checkpoints.len() > 32
            || m.artifacts.len() != self.artifacts.len()
            || !m.version.build.is_empty()
        {
            return invalid("unsupported schema, sequence or artifact count");
        }
        let mut checkpoints = BTreeSet::new();
        if m.minimum_supported_version
            .as_ref()
            .is_some_and(|v| !v.build.is_empty() || v >= &m.version)
        {
            return invalid(
                "minimum supported version must precede the release and exclude build metadata",
            );
        }
        let mut last: Option<&Version> = None;
        for c in &m.required_checkpoints {
            identifier(&c.release_id)?;
            if c.version >= m.version
                || !c.version.build.is_empty()
                || !checkpoints.insert(&c.release_id)
                || last.is_some_and(|v| v >= &c.version)
            {
                return invalid(
                    "checkpoint chain must be unique, increasing and before final version",
                );
            }
            last = Some(&c.version);
        }
        let mut ids = BTreeSet::new();
        for signed in &self.artifacts {
            verify(signed, keys)?;
            let a = &signed.body;
            identifier(&a.artifact_id)?;
            hash(&a.sha256)?;
            if a.schema_version != SCHEMA_VERSION
                || a.app_id != m.app_id
                || a.target != m.target
                || a.version != m.version
                || a.size_bytes == 0
                || a.package_signature.is_empty()
                || !ids.insert(&a.artifact_id)
                || m.artifacts.get(&a.artifact_id) != Some(&sha256(&canonical(a)?))
            {
                return invalid("artifact does not match signed release");
            }
            let prefix = format!("{}/{}/", m.app_id, m.target);
            if !a.object_key.starts_with(&prefix)
                || a.object_key.len() > 1024
                || a.object_key
                    .split('/')
                    .any(|s| s.is_empty() || s == "." || s == "..")
                || !a
                    .object_key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-._/".contains(&c))
            {
                return invalid("object key outside application/target namespace");
            }
        }
        let full = self
            .artifact(&m.full_artifact_id)
            .ok_or_else(|| ProtocolError::Invalid("missing full artifact".into()))?;
        if full.artifact != ArtifactKind::Full {
            return invalid("full fallback must be a full artifact");
        }
        for a in &self.artifacts {
            if let ArtifactKind::Delta {
                base_sha256,
                base_size_bytes,
                result_sha256,
                result_size_bytes,
                full_artifact_id,
            } = &a.body.artifact
            {
                hash(base_sha256)?;
                hash(result_sha256)?;
                if *base_size_bytes == 0
                    || full_artifact_id != &full.artifact_id
                    || result_sha256 != &full.sha256
                    || *result_size_bytes != full.size_bytes
                    || a.body.package_signature != full.package_signature
                {
                    return invalid("delta reconstruction/fallback mismatch");
                }
            }
        }
        Ok(())
    }
    pub fn artifact(&self, id: &str) -> Option<&ArtifactDescriptor> {
        self.artifacts
            .iter()
            .find(|a| a.body.artifact_id == id)
            .map(|a| &a.body)
    }
}
fn invalid<T>(message: &str) -> Result<T> {
    Err(ProtocolError::Invalid(message.into()))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Draft,
    Published,
    Disabled,
    Revoked,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RolloutPolicy {
    pub percentage: u8,
    pub cohort_key: String,
    pub paused: bool,
    pub revision: u64,
}
impl Default for RolloutPolicy {
    fn default() -> Self {
        Self {
            percentage: 100,
            cohort_key: "initial".into(),
            paused: false,
            revision: 0,
        }
    }
}
impl RolloutPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.percentage > 100 {
            return invalid("rollout percentage exceeds 100");
        }
        identifier(&self.cohort_key)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReleaseRecord {
    pub bundle: ReleaseBundle,
    pub lifecycle: Lifecycle,
    pub policy: RolloutPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRequest {
    pub app_id: String,
    pub channel: String,
    pub target: String,
    pub current_version: Version,
    pub installation_id: Option<String>,
    #[serde(default)]
    pub supports_delta: bool,
    pub base_sha256: Option<String>,
    pub base_size_bytes: Option<u64>,
    #[serde(default = "schema")]
    pub schema_version: u32,
}
fn schema() -> u32 {
    SCHEMA_VERSION
}
impl CheckRequest {
    pub fn validate(&self) -> Result<()> {
        for id in [&self.app_id, &self.channel, &self.target] {
            identifier(id)?;
        }
        if let Some(id) = &self.installation_id {
            identifier(id)?;
        }
        if let Some(h) = &self.base_sha256 {
            hash(h)?;
        }
        if self.schema_version != SCHEMA_VERSION {
            return invalid("unsupported client schema");
        }
        if self.base_sha256.is_some() != self.base_size_bytes.is_some() {
            return invalid("base hash and size are a pair");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateResponse {
    pub schema_version: u32,
    pub bundle: ReleaseBundle,
    pub selected_artifact_id: String,
    pub download_urls: BTreeMap<String, String>,
    pub status_url: String,
    pub app_id: String,
    pub channel: String,
    pub target: String,
    pub release_id: String,
    pub version: Version,
    pub published_at: DateTime<Utc>,
    pub notes: String,
    pub download_url: String,
    pub signature: String,
    pub signing_key_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub minimum_supported_version: Option<Version>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseStatus {
    pub release_id: String,
    pub lifecycle: Lifecycle,
    pub install_allowed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DraftRequest {
    pub bundle: ReleaseBundle,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminAction {
    pub operation_id: String,
    pub action: String,
    pub policy: Option<RolloutPolicy>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}
