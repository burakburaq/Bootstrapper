use crate::config::{AppConfig, Config};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use s3::{Bucket, Region, creds::Credentials};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use update_protocol::{ArtifactDescriptor, sha256, verify_package};

#[async_trait]
pub trait Storage: Send + Sync {
    async fn presign(&self, app: &AppConfig, artifact: &ArtifactDescriptor) -> Result<String>;
    async fn verify(&self, app: &AppConfig, artifact: &ArtifactDescriptor) -> Result<()>;
    async fn ready(&self, app: &AppConfig) -> Result<()>;
}
pub type SharedStorage = Arc<dyn Storage>;
pub struct R2Storage {
    buckets: BTreeMap<String, Box<Bucket>>,
    http: reqwest::Client,
    ttl: u32,
    max: u64,
    verification_slots: tokio::sync::Semaphore,
}
impl R2Storage {
    pub fn new(config: &Config, access: &str, secret: &str) -> Result<Self> {
        let credentials = Credentials::new(Some(access), Some(secret), None, None, None)?;
        let mut buckets = BTreeMap::new();
        for app in &config.apps {
            let mut bucket = Bucket::new(
                &app.bucket,
                Region::Custom {
                    region: "auto".into(),
                    endpoint: config.r2_endpoint.to_string().trim_end_matches('/').into(),
                },
                credentials.clone(),
            )?
            .with_path_style();
            bucket.set_request_timeout(Some(Duration::from_secs(10)));
            buckets.insert(app.app_id.clone(), bucket);
        }
        Ok(Self {
            buckets,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(120))
                .build()?,
            ttl: config.presign_ttl_seconds,
            max: config.max_artifact_bytes,
            verification_slots: tokio::sync::Semaphore::new(2),
        })
    }
    fn bucket(&self, app: &AppConfig) -> Result<&Bucket> {
        Ok(self
            .buckets
            .get(&app.app_id)
            .ok_or_else(|| anyhow::anyhow!("unmapped application storage"))?)
    }
}
#[async_trait]
impl Storage for R2Storage {
    async fn presign(&self, app: &AppConfig, a: &ArtifactDescriptor) -> Result<String> {
        Ok(self
            .bucket(app)?
            .presign_get(&a.object_key, self.ttl, None)
            .await?)
    }
    async fn verify(&self, app: &AppConfig, a: &ArtifactDescriptor) -> Result<()> {
        let _permit = self.verification_slots.acquire().await?;
        ensure!(
            a.size_bytes <= self.max,
            "artifact exceeds server verification limit"
        );
        let url = self.presign(app, a).await?;
        let mut response = self.http.get(url).send().await?;
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "artifact missing/inaccessible"
        );
        if let Some(size) = response.content_length() {
            ensure!(size == a.size_bytes, "storage artifact size differs");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() as u64 + chunk.len() as u64 <= a.size_bytes,
                "storage body exceeds descriptor"
            );
            bytes.extend_from_slice(&chunk);
        }
        ensure!(
            bytes.len() as u64 == a.size_bytes && sha256(&bytes) == a.sha256,
            "storage hash/size differs"
        );
        if a.artifact == update_protocol::ArtifactKind::Full {
            verify_package(&bytes, &a.package_signature, &app.package_public_key)?;
        }
        Ok(())
    }
    async fn ready(&self, app: &AppConfig) -> Result<()> {
        let (metadata, status) = self
            .bucket(app)?
            .head_object(&app.health_object_key)
            .await?;
        ensure!(
            status == 200 && metadata.content_length.is_some(),
            "application R2 health object inaccessible"
        );
        Ok(())
    }
}
