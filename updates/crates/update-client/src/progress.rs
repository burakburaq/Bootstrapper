use serde::{Deserialize, Serialize};
use update_protocol::InstallIntent;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Checking,
    Downloading,
    VerifyingPackage,
    Preflight,
    AwaitingAuthorization,
    Staging,
    VerifyingStaging,
    ClosingApplications,
    Installing,
    VerifyingInstallation,
    Completed,
    NoOp,
    Failed,
    Interrupted,
    Uninstalled,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressEvent {
    pub operation_id: String,
    pub intent: InstallIntent,
    pub stage: Stage,
    pub bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub percent: Option<u8>,
    pub error: Option<String>,
}
pub type ProgressCallback<'a> = &'a (dyn Fn(ProgressEvent) + Send + Sync);
impl ProgressEvent {
    pub fn stage(operation_id: &str, intent: InstallIntent, stage: Stage) -> Self {
        Self {
            operation_id: operation_id.into(),
            intent,
            stage,
            bytes: None,
            total_bytes: None,
            percent: None,
            error: None,
        }
    }
}
