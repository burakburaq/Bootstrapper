use crate::{
    InstallJournal, InstallPhase, Installer, delta, journal, verify_content, verify_package,
};
use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use reqwest::{Client, StatusCode};
use semver::Version;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncReadExt;
use update_protocol::{
    ArtifactDescriptor, ArtifactKind, CheckRequest, KeyRing, Lifecycle, ReleaseStatus,
    SCHEMA_VERSION, UpdateResponse,
};
use url::Url;

pub struct ClientConfig {
    pub server_url: Url,
    pub app_id: String,
    pub channel: String,
    pub target: String,
    pub metadata_keys: KeyRing,
    pub package_public_key: String,
    pub channel_token: Option<String>,
    pub allowed_download_hosts: BTreeSet<String>,
    pub cache_dir: PathBuf,
    pub max_artifact_bytes: u64,
    /// For local integration tests only. Never allows non-loopback cleartext hosts.
    pub allow_loopback_http: bool,
}
pub struct UpdateClient {
    config: ClientConfig,
    http: Client,
    storage_http: Client,
}
pub struct VerifiedUpdate {
    response: UpdateResponse,
}
pub struct PreparedUpdate {
    journal: InstallJournal,
    status_url: Url,
    bundle: update_protocol::ReleaseBundle,
}
impl VerifiedUpdate {
    pub fn version(&self) -> &Version {
        &self.response.version
    }
    pub fn release_id(&self) -> &str {
        &self.response.release_id
    }
}
impl PreparedUpdate {
    pub fn bundle(&self) -> &update_protocol::ReleaseBundle {
        &self.bundle
    }
    pub fn package_path(&self) -> &Path {
        &self.journal.package_path
    }
}

impl UpdateClient {
    pub fn new(config: ClientConfig) -> Result<Self> {
        Self::with_control_certificate(config, None)
    }

    /// Trust an explicit PEM CA for a local HTTPS control server only.
    /// Requires the opt-in `local-test-tls` feature; normal construction is unchanged.
    #[cfg(feature = "local-test-tls")]
    pub fn new_with_local_ca(config: ClientConfig, ca_pem: &[u8]) -> Result<Self> {
        let local_host = match config.server_url.host() {
            Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".test"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        ensure!(
            config.server_url.scheme() == "https" && local_host,
            "local CA requires HTTPS on localhost, a loopback IP or a .test hostname"
        );
        ensure!(
            !ca_pem.is_empty() && ca_pem.len() <= 64 * 1024,
            "local CA PEM must be non-empty and at most 64 KiB"
        );
        let pem_text = std::str::from_utf8(ca_pem)
            .context("local CA PEM must be UTF-8 text")?
            .trim();
        ensure!(
            pem_text.starts_with("-----BEGIN CERTIFICATE-----")
                && pem_text.ends_with("-----END CERTIFICATE-----")
                && pem_text.matches("-----BEGIN ").count() == 1
                && pem_text.matches("-----END ").count() == 1,
            "local CA PEM must contain exactly one certificate"
        );
        use base64::Engine as _;
        let encoded: String = pem_text
            .strip_prefix("-----BEGIN CERTIFICATE-----")
            .and_then(|value| value.strip_suffix("-----END CERTIFICATE-----"))
            .context("invalid local CA PEM certificate")?
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect();
        ensure!(!encoded.is_empty(), "local CA PEM certificate is empty");
        let der = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("invalid local CA PEM encoding")?;
        let certificate =
            reqwest::Certificate::from_der(&der).context("invalid local CA certificate")?;
        Self::with_control_certificate(config, Some(certificate))
    }

    fn with_control_certificate(
        config: ClientConfig,
        certificate: Option<reqwest::Certificate>,
    ) -> Result<Self> {
        update_protocol::identifier(&config.app_id)?;
        update_protocol::identifier(&config.channel)?;
        update_protocol::identifier(&config.target)?;
        validate_transport(&config.server_url, config.allow_loopback_http)?;
        ensure!(
            config.server_url.path() == "/"
                && config.server_url.username().is_empty()
                && config.server_url.password().is_none()
                && config.server_url.query().is_none()
                && config.server_url.fragment().is_none(),
            "server_url must be an origin"
        );
        update_protocol::validate_trust(&config.metadata_keys, &config.package_public_key)?;
        ensure!(
            !config.metadata_keys.is_empty()
                && config.max_artifact_bytes > 0
                && config.max_artifact_bytes <= 2 * 1024 * 1024 * 1024,
            "trust/size configuration required"
        );
        let storage_http = http_builder().build()?;
        let http = match certificate {
            Some(certificate) => http_builder().add_root_certificate(certificate).build()?,
            None => storage_http.clone(),
        };
        Ok(Self {
            config,
            http,
            storage_http,
        })
    }
    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.config.channel_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
    fn control_url(&self, value: &str) -> Result<Url> {
        let url = Url::parse(value)?;
        ensure!(
            url.origin() == self.config.server_url.origin()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "foreign control URL"
        );
        let prefix = format!(
            "/v1/apps/{}/channels/{}/",
            self.config.app_id, self.config.channel
        );
        ensure!(
            url.path().starts_with(&prefix),
            "control URL outside application/channel"
        );
        Ok(url)
    }
    pub async fn check(
        &self,
        current: Version,
        installation_id: String,
        base: Option<&Path>,
    ) -> Result<Option<VerifiedUpdate>> {
        let base_bytes = if let Some(path) = base {
            read_package(path, self.config.max_artifact_bytes)
                .await
                .ok()
        } else {
            None
        };
        let request = CheckRequest {
            app_id: self.config.app_id.clone(),
            channel: self.config.channel.clone(),
            target: self.config.target.clone(),
            current_version: current,
            installation_id: Some(installation_id),
            supports_delta: true,
            base_sha256: base_bytes.as_ref().map(|b| update_protocol::sha256(b)),
            base_size_bytes: base_bytes.as_ref().map(|b| b.len() as u64),
            schema_version: SCHEMA_VERSION,
        };
        request.validate()?;
        let endpoint = self.config.server_url.join("/v1/update/check")?;
        let response = self
            .authorize(self.http.post(endpoint))
            .json(&request)
            .send()
            .await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        ensure!(
            response.status() == StatusCode::OK,
            "update check rejected: {}",
            response.status()
        );
        let bytes = bounded(response, 2 * 1024 * 1024).await?;
        let response: UpdateResponse = serde_json::from_slice(&bytes)?;
        self.verify_response(
            response,
            Some(&request.current_version),
            update_protocol::InstallIntent::Update,
        )
    }
    fn verify_response(
        &self,
        response: UpdateResponse,
        installed: Option<&Version>,
        intent: update_protocol::InstallIntent,
    ) -> Result<Option<VerifiedUpdate>> {
        response.bundle.validate(&self.config.metadata_keys)?;
        let manifest = &response.bundle.manifest.body;
        ensure!(
            response.schema_version == SCHEMA_VERSION
                && manifest.app_id == self.config.app_id
                && manifest.channel == self.config.channel
                && manifest.target == self.config.target
                && match intent {
                    update_protocol::InstallIntent::Install => installed.is_none(),
                    update_protocol::InstallIntent::Update =>
                        installed.is_some_and(|v| &manifest.version > v),
                    update_protocol::InstallIntent::Repair => installed == Some(&manifest.version),
                }
                && response.version == manifest.version
                && response.release_id == manifest.release_id
                && response.app_id == manifest.app_id
                && response.channel == manifest.channel
                && response.target == manifest.target,
            "wrong application/version in response"
        );
        ensure!(
            manifest
                .required_checkpoints
                .iter()
                .all(|c| installed.is_none_or(|v| &c.version <= v)),
            "server skipped a required checkpoint"
        );
        ensure!(
            manifest
                .minimum_supported_version
                .as_ref()
                .is_none_or(|v| installed.is_none_or(|current| current >= v)),
            "incompatible direct upgrade"
        );
        ensure!(
            response
                .bundle
                .artifact(&response.selected_artifact_id)
                .is_some(),
            "unknown selected artifact"
        );
        self.control_url(&response.status_url)?;
        for a in &response.bundle.artifacts {
            ensure!(
                a.body.size_bytes <= self.config.max_artifact_bytes,
                "artifact exceeds client limit"
            );
            let value = response
                .download_urls
                .get(&a.body.artifact_id)
                .context("missing artifact URL")?;
            let url = self.control_url(value)?;
            let expected = format!(
                "/v1/apps/{}/channels/{}/releases/{}/targets/{}/artifacts/{}/download",
                manifest.app_id,
                manifest.channel,
                manifest.version,
                manifest.target,
                a.body.artifact_id
            );
            ensure!(
                url.path() == expected,
                "artifact URL does not match signed metadata"
            );
        }
        let expected_status = format!(
            "/v1/apps/{}/channels/{}/releases/{}/targets/{}/status",
            manifest.app_id, manifest.channel, manifest.version, manifest.target
        );
        ensure!(
            self.control_url(&response.status_url)?.path() == expected_status,
            "wrong lifecycle URL"
        );
        Ok(Some(VerifiedUpdate { response }))
    }
    pub async fn check_installation(
        &self,
        installed: Option<Version>,
        installation_id: String,
        intent: update_protocol::InstallIntent,
        operation_id: &str,
        callback: crate::progress::ProgressCallback<'_>,
    ) -> Result<Option<VerifiedUpdate>> {
        update_protocol::identifier(operation_id)?;
        let request = update_protocol::InstallRequest {
            app_id: self.config.app_id.clone(),
            channel: self.config.channel.clone(),
            target: self.config.target.clone(),
            installed_version: installed,
            installation_id,
            intent,
        };
        request.validate()?;
        callback(crate::progress::ProgressEvent::stage(
            operation_id,
            intent,
            crate::progress::Stage::Checking,
        ));
        let response = self
            .authorize(
                self.http
                    .post(self.config.server_url.join("/v1/install/resolve")?),
            )
            .json(&request)
            .send()
            .await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        ensure!(
            response.status() == StatusCode::OK,
            "installation resolution rejected: {}",
            response.status()
        );
        let response: UpdateResponse =
            serde_json::from_slice(&bounded(response, 2 * 1024 * 1024).await?)?;
        ensure!(
            response.bundle.manifest.body.installation.is_some(),
            "installation contract missing"
        );
        self.verify_response(response, request.installed_version.as_ref(), intent)
    }
    /// Called by the normal application only AFTER user consent and a helper challenge.
    pub async fn authorize_installation(
        &self,
        request: &update_protocol::AuthorizationRequest,
    ) -> Result<update_protocol::Signed<update_protocol::InstallAuthorization>> {
        ensure!(
            request.installation.app_id == self.config.app_id
                && request.installation.channel == self.config.channel
                && request.installation.target == self.config.target,
            "foreign authorization request"
        );
        let response = self
            .authorize(
                self.http
                    .post(self.config.server_url.join("/v1/install/authorize")?),
            )
            .json(request)
            .send()
            .await?;
        ensure!(
            response.status() == StatusCode::OK,
            "installation authorization unavailable: {}",
            response.status()
        );
        Ok(serde_json::from_slice(
            &bounded(response, 64 * 1024).await?,
        )?)
    }
    async fn download(
        &self,
        response: &UpdateResponse,
        artifact: &ArtifactDescriptor,
        progress: Option<(
            &str,
            update_protocol::InstallIntent,
            crate::progress::ProgressCallback<'_>,
        )>,
    ) -> Result<Vec<u8>> {
        let stable = self.control_url(
            response
                .download_urls
                .get(&artifact.artifact_id)
                .context("missing artifact URL")?,
        )?;
        for attempt in 0..2 {
            let redirect = self.authorize(self.http.get(stable.clone())).send().await?;
            ensure!(
                redirect.status() == StatusCode::TEMPORARY_REDIRECT,
                "download not authorized/available"
            );
            let location = redirect
                .headers()
                .get(reqwest::header::LOCATION)
                .context("missing redirect")?
                .to_str()?;
            let url = Url::parse(location)?;
            validate_transport(&url, self.config.allow_loopback_http)?;
            ensure!(
                url.username().is_empty()
                    && url.password().is_none()
                    && self
                        .config
                        .allowed_download_hosts
                        .contains(url.host_str().unwrap_or_default()),
                "untrusted storage host"
            );
            // Private channel credentials never leave the control server.
            let download = self.storage_http.get(url).send().await?;
            if attempt == 0 && matches!(download.status(), StatusCode::FORBIDDEN | StatusCode::GONE)
            {
                continue;
            }
            ensure!(
                download.status() == StatusCode::OK,
                "storage download failed"
            );
            let bytes = bounded_progress(download, artifact.size_bytes, progress).await?;
            verify_content(&bytes, artifact.size_bytes, &artifact.sha256)?;
            return Ok(bytes);
        }
        bail!("expired artifact URL")
    }
    pub async fn prepare(
        &self,
        update: VerifiedUpdate,
        base: Option<&Path>,
    ) -> Result<PreparedUpdate> {
        self.prepare_inner(update, base, None).await
    }
    pub async fn prepare_with_progress(
        &self,
        update: VerifiedUpdate,
        base: Option<&Path>,
        operation_id: &str,
        intent: update_protocol::InstallIntent,
        callback: crate::progress::ProgressCallback<'_>,
    ) -> Result<PreparedUpdate> {
        update_protocol::identifier(operation_id)?;
        self.prepare_inner(update, base, Some((operation_id, intent, callback)))
            .await
    }
    async fn prepare_inner(
        &self,
        update: VerifiedUpdate,
        base: Option<&Path>,
        progress: Option<(
            &str,
            update_protocol::InstallIntent,
            crate::progress::ProgressCallback<'_>,
        )>,
    ) -> Result<PreparedUpdate> {
        let r = update.response;
        let m = &r.bundle.manifest.body;
        let full = r
            .bundle
            .artifact(&m.full_artifact_id)
            .context("missing full artifact")?;
        let selected = r
            .bundle
            .artifact(&r.selected_artifact_id)
            .context("missing selected artifact")?;
        let mut result = None;
        if let ArtifactKind::Delta {
            base_sha256,
            base_size_bytes,
            result_sha256,
            result_size_bytes,
            ..
        } = &selected.artifact
            && let Some(base_path) = base
        {
            let delta_attempt: Result<Vec<u8>> = async {
                ensure!(
                    tokio::fs::metadata(base_path).await?.len() == *base_size_bytes
                        && *base_size_bytes <= self.config.max_artifact_bytes,
                    "base unavailable"
                );
                let base = read_package(base_path, self.config.max_artifact_bytes).await?;
                verify_content(&base, *base_size_bytes, base_sha256)?;
                let patch = self.download(&r, selected, progress).await?;
                let result = delta::apply(
                    &base,
                    &patch,
                    *result_size_bytes,
                    self.config.max_artifact_bytes,
                )?;
                verify_content(&result, *result_size_bytes, result_sha256)?;
                Ok(result)
            }
            .await;
            result = delta_attempt.ok();
        }
        let bytes = match result {
            Some(bytes) => bytes,
            None => self.download(&r, full, progress).await?,
        };
        if let Some((id, intent, callback)) = progress {
            callback(crate::progress::ProgressEvent::stage(
                id,
                intent,
                crate::progress::Stage::VerifyingPackage,
            ));
        }
        verify_content(&bytes, full.size_bytes, &full.sha256)?;
        verify_package(
            &bytes,
            &full.package_signature,
            &self.config.package_public_key,
        )?;
        let package_path = self
            .config
            .cache_dir
            .join(format!("{}.package", full.sha256));
        journal::atomic_write(&package_path, &bytes).await?;
        let journal = InstallJournal {
            last_verified_release: None,
            uninstall_pending: false,
            retired_installation_id: None,
            app_id: m.app_id.clone(),
            release_id: m.release_id.clone(),
            target_version: m.version.clone(),
            sequence: m.sequence,
            package_sha256: full.sha256.clone(),
            package_path,
            phase: InstallPhase::Prepared,
            updated_at: Utc::now(),
            operation: None,
        };
        Ok(PreparedUpdate {
            journal,
            status_url: self.control_url(&r.status_url)?,
            bundle: r.bundle,
        })
    }
    pub async fn install(
        &self,
        mut update: PreparedUpdate,
        installer: &dyn Installer,
        journal_path: &Path,
    ) -> Result<InstallJournal> {
        ensure!(
            update.bundle.manifest.body.installation.is_none(),
            "inventory-backed Windows releases require the privileged service API"
        );
        let _lock = journal::lock(journal_path).await?;
        journal::check_floor(
            journal_path,
            &self.config.app_id,
            &update.journal.release_id,
            &update.journal.target_version,
            update.journal.sequence,
        )
        .await?;
        let bytes =
            read_package(&update.journal.package_path, self.config.max_artifact_bytes).await?;
        ensure!(
            update_protocol::sha256(&bytes) == update.journal.package_sha256,
            "cached package changed"
        );
        let version = installer
            .inspect_version_and_publisher(&update.journal.package_path)
            .await?;
        ensure!(
            version == update.journal.target_version,
            "signed package product version differs from manifest"
        );
        let installed = installer.installed_version().await?;
        ensure!(version > installed, "installed version is not older");
        let response = self
            .authorize(self.http.get(update.status_url))
            .send()
            .await?;
        ensure!(
            response.status() == StatusCode::OK,
            "cannot revalidate lifecycle before install"
        );
        let status: ReleaseStatus = serde_json::from_slice(&bounded(response, 64 * 1024).await?)?;
        ensure!(
            status.release_id == update.journal.release_id
                && status.lifecycle == Lifecycle::Published
                && status.install_allowed,
            "release disabled or revoked before install"
        );
        update.journal.save(journal_path).await?;
        update.journal.phase = InstallPhase::Installing;
        update.journal.save(journal_path).await?;
        if let Err(error) = installer.install(&update.journal.package_path).await {
            update.journal.phase = InstallPhase::Failed;
            update.journal.updated_at = Utc::now();
            update.journal.save(journal_path).await?;
            return Err(error);
        }
        update.journal.phase = InstallPhase::AwaitingRestart;
        update.journal.updated_at = Utc::now();
        update.journal.save(journal_path).await?;
        Ok(update.journal)
    }
}
fn http_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
}
fn validate_transport(url: &Url, loopback: bool) -> Result<()> {
    ensure!(
        url.scheme() == "https"
            || (loopback
                && url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
        "HTTPS required"
    );
    Ok(())
}
async fn read_package(path: &Path, max: u64) -> Result<Vec<u8>> {
    let file = tokio::fs::File::open(path).await?;
    ensure!(
        file.metadata().await?.len() <= max,
        "package exceeds resource limit"
    );
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() as u64 <= max,
        "package grew beyond resource limit"
    );
    Ok(bytes)
}
async fn bounded(response: reqwest::Response, max: u64) -> Result<Vec<u8>> {
    bounded_progress(response, max, None).await
}
async fn bounded_progress(
    mut response: reqwest::Response,
    max: u64,
    progress: Option<(
        &str,
        update_protocol::InstallIntent,
        crate::progress::ProgressCallback<'_>,
    )>,
) -> Result<Vec<u8>> {
    if let Some(size) = response.content_length() {
        ensure!(size <= max, "response exceeds expected size");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            (bytes.len() as u64).saturating_add(chunk.len() as u64) <= max,
            "response exceeds expected size"
        );
        bytes.extend_from_slice(&chunk);
        if let Some((id, intent, callback)) = progress {
            let mut event = crate::progress::ProgressEvent::stage(
                id,
                intent,
                crate::progress::Stage::Downloading,
            );
            event.bytes = Some(bytes.len() as u64);
            event.total_bytes = Some(max);
            callback(event);
        }
    }
    Ok(bytes)
}

#[cfg(all(test, feature = "local-test-tls"))]
mod local_tls_tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair, date_time_ymd};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, rustls};
    use update_protocol::fixtures::{keys, package_signature};

    fn config(origin: Url) -> ClientConfig {
        ClientConfig {
            server_url: origin,
            app_id: "alpha".into(),
            channel: "staging".into(),
            target: "windows-x86_64".into(),
            metadata_keys: keys(1),
            package_public_key: package_signature(b"fixture", 1).1,
            channel_token: None,
            allowed_download_hosts: BTreeSet::from(["localhost".into()]),
            cache_dir: std::env::temp_dir(),
            max_artifact_bytes: 1024,
            allow_loopback_http: false,
        }
    }

    async fn server(expired: bool) -> (Url, Vec<u8>, tokio::task::JoinHandle<()>) {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        if expired {
            params.not_before = date_time_ymd(2019, 1, 1);
            params.not_after = date_time_ymd(2020, 1, 1);
        }
        let certificate = params.self_signed(&key).unwrap();
        let pem = certificate.pem().into_bytes();
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut stream) = acceptor.accept(socket).await {
                        let mut request = [0; 4096];
                        if stream.read(&mut request).await.is_ok() {
                            let _ = stream
                                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                                .await;
                            let _ = stream.shutdown().await;
                        }
                    }
                });
            }
        });
        (
            format!("https://localhost:{port}/").parse().unwrap(),
            pem,
            task,
        )
    }

    #[tokio::test]
    async fn explicit_ca_trusts_control_only_and_normal_client_stays_strict() {
        let (origin, pem, task) = server(false).await;
        let client = UpdateClient::new_with_local_ca(config(origin.clone()), &pem).unwrap();
        assert_eq!(
            client
                .http
                .get(origin.clone())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        assert!(
            client
                .storage_http
                .get(origin.clone())
                .send()
                .await
                .is_err()
        );
        let normal = UpdateClient::new(config(origin.clone())).unwrap();
        assert!(normal.http.get(origin).send().await.is_err());
        task.abort();
    }

    #[tokio::test]
    async fn unrelated_ca_is_rejected() {
        let (origin, _, task) = server(false).await;
        let key = KeyPair::generate().unwrap();
        let other_ca = CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let client =
            UpdateClient::new_with_local_ca(config(origin.clone()), other_ca.pem().as_bytes())
                .unwrap();
        assert!(client.http.get(origin).send().await.is_err());
        task.abort();
    }

    #[tokio::test]
    async fn trusted_ca_does_not_disable_hostname_verification() {
        let (mut origin, pem, task) = server(false).await;
        origin.set_host(Some("127.0.0.1")).unwrap();
        let client = UpdateClient::new_with_local_ca(config(origin.clone()), &pem).unwrap();
        assert!(client.http.get(origin).send().await.is_err());
        task.abort();
    }

    #[tokio::test]
    async fn expired_certificate_is_rejected() {
        let (origin, pem, task) = server(true).await;
        let client = UpdateClient::new_with_local_ca(config(origin.clone()), &pem).unwrap();
        assert!(client.http.get(origin).send().await.is_err());
        task.abort();
    }

    #[test]
    fn local_ca_rejects_production_origins_http_and_invalid_pem() {
        let ca = rcgen::generate_simple_self_signed(vec!["localhost".into()])
            .unwrap()
            .cert
            .pem();
        for origin in ["https://updates.example.com/", "http://localhost/"] {
            assert!(
                UpdateClient::new_with_local_ca(config(origin.parse().unwrap()), ca.as_bytes())
                    .is_err()
            );
        }
        for invalid_pem in [
            "invalid certificate",
            "-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----",
            "-----BEGIN CERTIFICATE-----\naW52YWxpZA==\n-----END CERTIFICATE-----",
        ] {
            assert!(
                UpdateClient::new_with_local_ca(
                    config("https://localhost/".parse().unwrap()),
                    invalid_pem.as_bytes()
                )
                .is_err()
            );
        }
    }
}
