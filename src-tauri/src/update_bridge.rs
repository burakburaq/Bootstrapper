//! Native coordinator for the shared client's signed Windows installation flow.
use async_trait::async_trait;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::Mutex as AsyncMutex;
use update_client::{
    ClientConfig, PreparedUpdate, UpdateClient, VerifiedUpdate,
    installation::{BeginResult, PreflightReport},
    progress::{ProgressCallback, ProgressEvent, Stage},
    update_protocol::{InstallAuthorization, InstallIntent, KeyRing, Signed},
};
use url::Url;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub server_url: Option<String>,
    pub channel: String,
    pub metadata_keys: KeyRing,
    pub package_public_key: Option<String>,
    pub allowed_download_hosts: BTreeSet<String>,
    pub max_artifact_bytes: u64,
    #[cfg(windows)]
    #[serde(default)]
    pub windows_policy: Option<Signed<update_client::windows::WindowsPolicy>>,
    #[cfg(windows)]
    #[serde(default)]
    pub bootstrap_helper: Option<crate::service_bootstrap::HelperDownload>,
}
impl Settings {
    pub fn embedded() -> Result<Self, String> {
        serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/trust.json")))
            .map_err(|_| "update_config_invalid".into())
    }
    #[cfg(windows)]
    pub fn policy(&self) -> Result<Option<update_client::windows::WindowsPolicy>, String> {
        let Some(signed) = &self.windows_policy else {
            return Ok(None);
        };
        update_client::update_protocol::verify(signed, &self.metadata_keys)
            .map_err(|_| "service_policy_signature_invalid")?;
        let policy = &signed.body;
        policy.validate().map_err(|_| "service_policy_invalid")?;
        if policy.app_id != crate::product::APP_ID
            || policy.product_directory != crate::product::PRODUCT_DIRECTORY
            || policy.executable != crate::product::EXECUTABLE
            || policy.channel != self.channel
            || policy.target != format!("windows-{}-nsis", std::env::consts::ARCH)
            || policy.metadata_keys != self.metadata_keys
            || Some(&policy.package_public_key) != self.package_public_key.as_ref()
        {
            return Err("service_policy_context_mismatch".into());
        }
        Ok(Some(policy.clone()))
    }
    fn client(&self, architecture: &str, cache: PathBuf) -> Result<Option<UpdateClient>, String> {
        let Some(server) = &self.server_url else {
            return Ok(None);
        };
        if self.allowed_download_hosts.is_empty() {
            return Err("update_download_hosts_missing".into());
        }
        let config = ClientConfig {
            server_url: Url::parse(server).map_err(|_| "update_server_invalid")?,
            app_id: crate::product::APP_ID.into(),
            channel: self.channel.clone(),
            target: format!("windows-{architecture}-nsis"),
            metadata_keys: self.metadata_keys.clone(),
            package_public_key: self
                .package_public_key
                .clone()
                .ok_or("update_package_key_missing")?,
            channel_token: None,
            allowed_download_hosts: self.allowed_download_hosts.clone(),
            cache_dir: cache,
            max_artifact_bytes: self.max_artifact_bytes,
            allow_loopback_http: false,
        };
        #[cfg(feature = "local-test-tls")]
        let client = UpdateClient::new_with_local_ca(
            config,
            include_bytes!(concat!(env!("OUT_DIR"), "/local-ca.pem")),
        );
        #[cfg(not(feature = "local-test-tls"))]
        let client = UpdateClient::new(config);
        client.map(Some).map_err(|_| "update_trust_invalid".into())
    }
}
#[async_trait]
pub trait InstallationService: Send + Sync {
    async fn installed_is_healthy(&self) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn context(&self) -> anyhow::Result<(Option<Version>, String)>;
    async fn pending(&self) -> anyhow::Result<Option<ProgressEvent>>;
    async fn begin(
        &self,
        prepared: &PreparedUpdate,
        id: &str,
        intent: InstallIntent,
    ) -> anyhow::Result<BeginResult>;
    async fn commit(
        &self,
        prepared: &PreparedUpdate,
        id: &str,
        authorization: Signed<InstallAuthorization>,
        callback: ProgressCallback<'_>,
    ) -> anyhow::Result<ProgressEvent>;
}
#[cfg(windows)]
#[async_trait]
impl InstallationService for update_client::windows::service::WindowsServiceClient {
    async fn installed_is_healthy(&self) -> anyhow::Result<bool> {
        self.installed_is_healthy().await
    }
    async fn context(&self) -> anyhow::Result<(Option<Version>, String)> {
        self.installation_context().await
    }
    async fn pending(&self) -> anyhow::Result<Option<ProgressEvent>> {
        self.pending().await
    }
    async fn begin(
        &self,
        prepared: &PreparedUpdate,
        id: &str,
        intent: InstallIntent,
    ) -> anyhow::Result<BeginResult> {
        self.begin(prepared, id, intent).await
    }
    async fn commit(
        &self,
        prepared: &PreparedUpdate,
        id: &str,
        authorization: Signed<InstallAuthorization>,
        callback: ProgressCallback<'_>,
    ) -> anyhow::Result<ProgressEvent> {
        self.commit(prepared, id, authorization, callback).await
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub installed_healthy: bool,
    pub installed_version: Option<String>,
    pub operation_id: String,
    pub stage: String,
    pub download_percent: Option<u8>,
    pub install_percent: Option<u8>,
    pub bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub intent: Option<InstallIntent>,
    pub version: Option<String>,
    pub release_id: Option<String>,
    pub configured: bool,
    pub service_ready: bool,
    pub error: Option<String>,
    pub preflight: Option<PreflightReport>,
    pub capabilities: Capabilities,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub check: bool,
    pub prepare: bool,
    pub install: bool,
    pub repair: bool,
    pub byte_progress: bool,
    pub installed_file_verification: bool,
}
struct Pending {
    verified: Option<VerifiedUpdate>,
    prepared: Option<PreparedUpdate>,
    intent: Option<InstallIntent>,
}
fn service_is_starting(error: &anyhow::Error) -> bool {
    // Retry only pipe-open errors. A missing file during signature verification
    // is an authentication failure, not a service that has yet to register.
    error
        .chain()
        .any(|cause| cause.to_string() == "IPC pipe open failed")
        && error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| matches!(io.raw_os_error(), Some(2 | 231)))
        })
}

pub struct UpdateBridge {
    client: Option<UpdateClient>,
    service: Option<Arc<dyn InstallationService>>,
    operation_id: String,
    pending: AsyncMutex<Pending>,
    status: Mutex<UpdateStatus>,
}
impl UpdateBridge {
    pub fn new(settings: Settings, architecture: &str, cache: PathBuf) -> Result<Self, String> {
        #[cfg(windows)]
        let service = settings.policy()?.map(|policy| {
            Arc::new(update_client::windows::service::WindowsServiceClient { policy })
                as Arc<dyn InstallationService>
        });
        #[cfg(not(windows))]
        let service = None;
        Ok(Self::from_parts(
            settings.client(architecture, cache)?,
            service,
        ))
    }
    fn from_parts(
        client: Option<UpdateClient>,
        service: Option<Arc<dyn InstallationService>>,
    ) -> Self {
        let configured = client.is_some();
        let installs = configured && service.is_some();
        let operation_id = uuid::Uuid::new_v4().to_string();
        Self {
            client,
            service,
            operation_id: operation_id.clone(),
            pending: AsyncMutex::new(Pending {
                verified: None,
                prepared: None,
                intent: None,
            }),
            status: Mutex::new(UpdateStatus {
                installed_healthy: false,
                installed_version: None,
                operation_id,
                stage: if configured { "idle" } else { "unconfigured" }.into(),
                download_percent: None,
                install_percent: None,
                bytes: None,
                total_bytes: None,
                intent: None,
                version: None,
                release_id: None,
                configured,
                service_ready: false,
                error: None,
                preflight: None,
                capabilities: Capabilities {
                    check: configured,
                    prepare: configured,
                    install: installs,
                    repair: installs,
                    byte_progress: configured,
                    installed_file_verification: installs,
                },
            }),
        }
    }
    pub fn status(&self) -> Result<UpdateStatus, String> {
        self.status
            .lock()
            .map(|s| s.clone())
            .map_err(|_| "update_state_failed".into())
    }
    pub fn block(&self, code: &str) -> Result<(), String> {
        self.set_stage("blocked", Some(code))
    }
    fn set_stage(&self, stage: &str, error: Option<&str>) -> Result<(), String> {
        let mut state = self.status.lock().map_err(|_| "update_state_failed")?;
        state.stage = stage.into();
        state.error = error.map(str::to_owned);
        Ok(())
    }
    pub fn validate_id(&self, id: &str) -> Result<(), String> {
        if id != self.operation_id {
            return Err("operation_mismatch".into());
        }
        Ok(())
    }
    pub async fn service_ready(&self) -> bool {
        self.probe_service().await.unwrap_or(false)
    }
    pub async fn probe_service(&self) -> Result<bool, String> {
        if let Some(service) = &self.service {
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(10), service.pending())
                    .await
                    .map_err(|_| "service_ipc_timeout".to_string())?;
            match result {
                Ok(_) => Ok(true),
                Err(error) => {
                    // A missing/busy pipe is expected while the worker downloads
                    // and registers the service. Authentication and protocol
                    // failures are not readiness states and must reach the UI.
                    if service_is_starting(&error) {
                        Ok(false)
                    } else {
                        Err(format!("service_ipc_failed: {error:#}"))
                    }
                }
            }
        } else {
            Ok(false)
        }
    }
    fn progress(&self, event: ProgressEvent) {
        if event.operation_id != self.operation_id {
            return;
        }
        if let Ok(mut state) = self.status.lock() {
            if state.intent.is_some_and(|intent| intent != event.intent) {
                return;
            }
            state.stage = serde_json::to_value(event.stage)
                .ok()
                .and_then(|s| s.as_str().map(str::to_owned))
                .unwrap_or_else(|| "failed".into());
            state.bytes = event.bytes;
            state.total_bytes = event.total_bytes;
            state.error = event.error.as_ref().map(|_| "installation_failed".into());
            match event.stage {
                Stage::Downloading => state.download_percent = event.percent.map(|n| n.min(100)),
                Stage::VerifyingPackage => state.download_percent = Some(100),
                Stage::Installing => {
                    state.download_percent = Some(100);
                    state.install_percent = event.percent.map(|n| n.min(100));
                }
                Stage::Completed | Stage::NoOp => {
                    state.download_percent = Some(100);
                    state.install_percent = Some(100);
                }
                _ => (),
            }
        }
    }
    pub async fn refresh(&self) -> Result<UpdateStatus, String> {
        let client = self.client.as_ref().ok_or("update_not_configured")?;
        let service = self.service.as_ref().ok_or("service_policy_missing")?;
        let mut pending = self.pending.try_lock().map_err(|_| "update_busy")?;
        if !matches!(self.status()?.stage.as_str(), "idle" | "awaiting_service") {
            return self.status();
        }
        let head = match service.pending().await {
            Ok(head) => head,
            Err(_) => {
                self.set_stage("awaiting_service", None)?;
                return self.status();
            }
        };
        self.status
            .lock()
            .map_err(|_| "update_state_failed")?
            .service_ready = true;
        if head.is_some_and(|e| {
            !matches!(e.stage, Stage::Completed | Stage::NoOp | Stage::Uninstalled)
        }) {
            self.block("pending_installation_requires_attention")?;
            return self.status();
        }
        let (installed, identity) = service
            .context()
            .await
            .map_err(|_| "installation_context_failed")?;
        let healthy = if installed.is_some() {
            service
                .installed_is_healthy()
                .await
                .map_err(|_| "installation_verification_failed")?
        } else {
            false
        };
        self.status
            .lock()
            .map_err(|_| "update_state_failed")?
            .installed_healthy = healthy;
        self.status
            .lock()
            .map_err(|_| "update_state_failed")?
            .installed_version = installed.as_ref().map(ToString::to_string);
        let mut intent = if installed.is_none() {
            InstallIntent::Install
        } else {
            InstallIntent::Update
        };
        self.status
            .lock()
            .map_err(|_| "update_state_failed")?
            .intent = Some(intent);
        let callback = |e| self.progress(e);
        let result = async {
            let mut verified = client
                .check_installation(
                    installed.clone(),
                    identity.clone(),
                    intent,
                    &self.operation_id,
                    &callback,
                )
                .await?;
            if verified.is_none() && installed.is_some() {
                intent = InstallIntent::Repair;
                self.status
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state poisoned"))?
                    .intent = Some(intent);
                verified = client
                    .check_installation(installed, identity, intent, &self.operation_id, &callback)
                    .await?;
            }
            anyhow::Ok(verified)
        }
        .await;
        match result {
            Ok(Some(verified)) => {
                let mut state = self.status.lock().map_err(|_| "update_state_failed")?;
                state.version = Some(verified.version().to_string());
                state.release_id = Some(verified.release_id().into());
                state.stage = "available".into();
                state.error = None;
                pending.intent = Some(intent);
                pending.verified = Some(verified);
            }
            Ok(None) => self.set_stage("no_release", None)?,
            Err(_) => {
                self.set_stage("failed", Some("installation_check_failed"))?;
                return Err("installation_check_failed".into());
            }
        }
        self.status()
    }
    pub async fn install(&self, id: &str) -> Result<UpdateStatus, String> {
        self.validate_id(id)?;
        let client = self.client.as_ref().ok_or("update_not_configured")?;
        let service = self.service.as_ref().ok_or("service_policy_missing")?;
        let mut pending = self.pending.try_lock().map_err(|_| "update_busy")?;
        if matches!(self.status()?.stage.as_str(), "completed" | "no_op") {
            return self.status();
        }
        let intent = pending.intent.ok_or("installation_not_checked")?;
        let callback = |event| self.progress(event);
        let result = async {
            if pending.prepared.is_none() {
                let verified = pending
                    .verified
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("not checked"))?;
                pending.prepared = Some(
                    client
                        .prepare_with_progress(verified, None, id, intent, &callback)
                        .await?,
                );
            }
            let prepared = pending.prepared.as_ref().expect("prepared above");
            match service.begin(prepared, id, intent).await? {
                BeginResult::Existing(event) => Ok(event),
                BeginResult::Challenge(challenge) => {
                    self.status
                        .lock()
                        .map_err(|_| anyhow::anyhow!("state poisoned"))?
                        .preflight = Some(challenge.preflight);
                    callback(challenge.progress);
                    let authorization = client.authorize_installation(&challenge.request).await?;
                    service.commit(prepared, id, authorization, &callback).await
                }
            }
        }
        .await;
        match result {
            Ok(event) if event.operation_id == self.operation_id && event.intent == intent => {
                let success = matches!(event.stage, Stage::Completed | Stage::NoOp);
                self.progress(event);
                if !success {
                    self.set_stage("failed", Some("installation_incomplete"))?;
                    return Err("installation_incomplete".into());
                }
            }
            _ => {
                self.set_stage("failed", Some("installation_failed"))?;
                return Err("installation_failed".into());
            }
        }
        self.status()
    }
}
#[cfg(test)]
#[path = "update_bridge_tests.rs"]
mod tests;
