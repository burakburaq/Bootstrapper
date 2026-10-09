use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use subtle::ConstantTimeEq;
use update_protocol::{KeyRing, identifier, sha256};
use url::Url;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelConfig {
    pub private: bool,
    pub read_token_sha256: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminToken {
    pub token_sha256: String,
    pub channels: BTreeSet<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub app_id: String,
    pub channels: BTreeMap<String, ChannelConfig>,
    pub targets: BTreeSet<String>,
    pub metadata_keys: KeyRing,
    pub package_public_key: String,
    pub bucket: String,
    pub health_object_key: String,
    pub admin_tokens: Vec<AdminToken>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub public_url: Url,
    pub r2_endpoint: Url,
    pub apps: Vec<AppConfig>,
    #[serde(default = "max_bytes")]
    pub max_artifact_bytes: u64,
    #[serde(default = "ttl")]
    pub presign_ttl_seconds: u32,
}
fn max_bytes() -> u64 {
    256 * 1024 * 1024
}
fn ttl() -> u32 {
    300
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.public_url.scheme() == "https"
                && self.public_url.username().is_empty()
                && self.public_url.password().is_none()
                && self.public_url.path() == "/"
                && self.public_url.query().is_none()
                && self.public_url.fragment().is_none(),
            "public_url must be an HTTPS origin"
        );
        ensure!(
            self.r2_endpoint.scheme() == "https"
                && self
                    .r2_endpoint
                    .host_str()
                    .is_some_and(|h| h.ends_with(".r2.cloudflarestorage.com")),
            "R2 S3 API endpoint required"
        );
        ensure!(
            self.r2_endpoint.path() == "/"
                && self.r2_endpoint.query().is_none()
                && self.r2_endpoint.username().is_empty()
                && self.r2_endpoint.password().is_none()
                && self.r2_endpoint.fragment().is_none(),
            "R2 origin required"
        );
        ensure!(
            self.max_artifact_bytes > 0
                && self.max_artifact_bytes <= 2 * 1024 * 1024 * 1024
                && (30..=3600).contains(&self.presign_ttl_seconds),
            "invalid resource limit or URL lifetime"
        );
        let mut ids = BTreeSet::new();
        let mut buckets = BTreeSet::new();
        for app in &self.apps {
            update_protocol::validate_trust(&app.metadata_keys, &app.package_public_key)?;
            identifier(&app.app_id)?;
            identifier(&app.bucket)?;
            ensure!(
                app.health_object_key == format!("{}/health.txt", app.app_id),
                "application health object required"
            );
            ensure!(
                ids.insert(&app.app_id) && buckets.insert(&app.bucket),
                "duplicate app or shared application bucket"
            );
            ensure!(
                !app.channels.is_empty()
                    && !app.targets.is_empty()
                    && !app.metadata_keys.is_empty()
                    && !app.package_public_key.is_empty(),
                "incomplete application trust/configuration"
            );
            for target in &app.targets {
                identifier(target)?;
            }
            for (name, channel) in &app.channels {
                identifier(name)?;
                ensure!(
                    !channel.private || channel.read_token_sha256.is_some(),
                    "private channel requires read token hash"
                );
                if let Some(hash) = &channel.read_token_sha256 {
                    update_protocol::hash(hash)?;
                }
            }
            for token in &app.admin_tokens {
                update_protocol::hash(&token.token_sha256)?;
                ensure!(
                    !token.channels.is_empty()
                        && token.channels.iter().all(|c| app.channels.contains_key(c)),
                    "admin token has invalid channel scope"
                );
            }
        }
        ensure!(!self.apps.is_empty(), "at least one application required");
        Ok(())
    }
    pub fn app(&self, id: &str) -> Option<&AppConfig> {
        self.apps.iter().find(|a| a.app_id == id)
    }
}
fn matches(token: Option<&str>, expected: &str) -> bool {
    token.is_some_and(|token| {
        sha256(token.as_bytes())
            .as_bytes()
            .ct_eq(expected.as_bytes())
            .into()
    })
}
impl AppConfig {
    pub fn can_read(&self, channel: &str, token: Option<&str>) -> bool {
        self.channels.get(channel).is_some_and(|c| {
            !c.private
                || c.read_token_sha256
                    .as_ref()
                    .is_some_and(|h| matches(token, h))
        })
    }
    pub fn can_admin(&self, channel: &str, token: Option<&str>) -> bool {
        self.admin_tokens
            .iter()
            .any(|t| t.channels.contains(channel) && matches(token, &t.token_sha256))
    }
}
