use serde::Serialize;

pub const CONTRACT_VERSION: u32 = 1;
pub const APP_ID: &str = "baykuslig";
pub const WEBVIEW_DOWNLOAD_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

pub fn validate_contract(version: u32) -> Result<(), String> {
    if version != CONTRACT_VERSION {
        return Err("contract_incompatible".into());
    }
    Ok(())
}

pub fn supported_platform(major: u32, build: u32, architecture: &str) -> bool {
    major >= 10 && build >= 10240 && matches!(architecture, "x86_64" | "aarch64")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paths {
    pub program: String,
    pub common_state: String,
    pub user_data: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationHint {
    pub scope: &'static str,
    pub registry_view: &'static str,
    pub version: Option<String>,
    pub location: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub contract_version: u32,
    pub app_id: &'static str,
    pub windows_build: u32,
    pub native_architecture: String,
    pub installer_architecture: &'static str,
    pub webview_version: Option<String>,
    pub platform_supported: bool,
    pub paths: Paths,
    pub installations: Vec<InstallationHint>,
    pub common_state_present: bool,
    pub program_directory_present: bool,
    // Configured installation capability; does not assert that an install ran.
    pub installation_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_contract_is_rejected_before_platform_inspection() {
        assert!(validate_contract(CONTRACT_VERSION).is_ok());
        assert_eq!(validate_contract(0), Err("contract_incompatible".into()));
        assert_eq!(validate_contract(2), Err("contract_incompatible".into()));
    }

    #[test]
    fn minimum_windows_build_and_native_architecture_are_required() {
        assert!(supported_platform(10, 10240, "x86_64"));
        assert!(supported_platform(10, 26100, "aarch64"));
        assert!(!supported_platform(10, 9600, "x86_64"));
        assert!(!supported_platform(6, 26100, "x86_64"));
        assert!(!supported_platform(10, 26100, "x86"));
        assert!(!supported_platform(10, 26100, "unknown"));
    }
}
