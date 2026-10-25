//! Narrow Windows API boundary: read-only OS/known-folder inspection, and a fixed
//! official URL opened only after user action. No process/package execution API.
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr};

use windows_sys::{
    Wdk::System::SystemServices::RtlGetVersion,
    Win32::{
        System::{
            Com::CoTaskMemFree,
            SystemInformation::{
                GetNativeSystemInfo, OSVERSIONINFOW, PROCESSOR_ARCHITECTURE_AMD64,
                PROCESSOR_ARCHITECTURE_ARM64, SYSTEM_INFO,
            },
        },
        UI::{
            Shell::{
                FOLDERID_LocalAppData, FOLDERID_ProgramData, FOLDERID_ProgramFiles,
                SHGetKnownFolderPath, ShellExecuteW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::GUID,
};
use winreg::{RegKey, enums::*};

use crate::preflight::{self, Inspection, InstallationHint, Paths};

fn known_folder(id: &GUID) -> Result<PathBuf, String> {
    let mut raw = ptr::null_mut();
    // SAFETY: id and the writable output pointer live through the call. No token
    // or CREATE flag is supplied; the OS allocates a null-terminated UTF-16 path.
    let status = unsafe { SHGetKnownFolderPath(id, 0, ptr::null_mut(), &mut raw) };
    if status < 0 || raw.is_null() {
        if !raw.is_null() {
            // SAFETY: SHGetKnownFolderPath allocated this pointer with COM memory.
            unsafe { CoTaskMemFree(raw.cast()) };
        }
        return Err("known_folder_unavailable".into());
    }
    let mut length = 0;
    // SAFETY: a successful call returned an allocated, null-terminated UTF-16
    // string. It remains allocated until the explicit CoTaskMemFree below.
    unsafe {
        while *raw.add(length) != 0 {
            length += 1;
        }
    }
    // SAFETY: length is the number of code units preceding that terminator.
    let value = OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, length) });
    // SAFETY: free the OS allocation exactly once, after copying its contents.
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(value.into())
}

pub fn installer_profile() -> Result<PathBuf, String> {
    Ok(known_folder(&FOLDERID_LocalAppData)?
        .join(crate::product::INSTALLER_IDENTIFIER)
        .join("EBWebView"))
}

fn exists(path: &std::path::Path) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("installation_inspection_failed".into()),
    }
}

fn installation_hints() -> Result<Vec<InstallationHint>, String> {
    let uninstall_key = format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{}",
        crate::product::APP_ID
    );
    let mut installations = Vec::new();
    for (hive, scope) in [
        (HKEY_CURRENT_USER, "per_user"),
        (HKEY_LOCAL_MACHINE, "per_machine"),
    ] {
        for (flag, view) in [(KEY_WOW64_64KEY, "64"), (KEY_WOW64_32KEY, "32")] {
            let root = RegKey::predef(hive);
            let key = match root.open_subkey_with_flags(&uninstall_key, KEY_READ | flag) {
                Ok(key) => key,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err("installation_registry_unavailable".into()),
            };
            // Registry values are only hints. Never execute an uninstall string or
            // accept the recorded location as authority for privileged mutation.
            installations.push(InstallationHint {
                scope,
                registry_view: view,
                version: key.get_value("DisplayVersion").ok(),
                location: key.get_value("InstallLocation").ok(),
            });
        }
    }
    Ok(installations)
}

pub fn inspect(contract_version: u32) -> Result<Inspection, String> {
    preflight::validate_contract(contract_version)?;
    let mut os = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: correctly sized, initialized OSVERSIONINFOW lives through the call.
    if unsafe { RtlGetVersion(&mut os) } < 0 {
        return Err("windows_version_unavailable".into());
    }
    let mut system = SYSTEM_INFO::default();
    // SAFETY: output points to a valid writable SYSTEM_INFO of the required size.
    unsafe { GetNativeSystemInfo(&mut system) };
    // SAFETY: GetNativeSystemInfo initialized the documented architecture member.
    let architecture = match unsafe { system.Anonymous.Anonymous.wProcessorArchitecture } {
        PROCESSOR_ARCHITECTURE_AMD64 => "x86_64",
        PROCESSOR_ARCHITECTURE_ARM64 => "aarch64",
        _ => "unknown",
    };
    let settings = crate::update_bridge::Settings::embedded()?;
    let policy = settings.policy()?;
    let (program, common_state) = if let Some(policy) = &policy {
        (
            policy
                .application_directory()
                .map_err(|_| "installation_path_failed")?,
            known_folder(&FOLDERID_ProgramData)?
                .join(format!("{} Update", policy.product_directory)),
        )
    } else {
        (
            known_folder(&FOLDERID_ProgramFiles)?
                .join("TalayLabs")
                .join(crate::product::PRODUCT_DIRECTORY),
            known_folder(&FOLDERID_ProgramData)?.join(crate::product::PRODUCT_DIRECTORY),
        )
    };
    let user_data =
        known_folder(&FOLDERID_LocalAppData)?.join(crate::product::APPLICATION_IDENTIFIER);
    Ok(Inspection {
        contract_version: preflight::CONTRACT_VERSION,
        app_id: preflight::APP_ID,
        windows_build: os.dwBuildNumber,
        native_architecture: architecture.into(),
        installer_architecture: std::env::consts::ARCH,
        webview_version: tauri::webview_version()
            .ok()
            .filter(|value| !value.is_empty()),
        platform_supported: preflight::supported_platform(
            os.dwMajorVersion,
            os.dwBuildNumber,
            architecture,
        ),
        installations: installation_hints()?,
        program_directory_present: exists(&program)?,
        common_state_present: exists(&common_state)?,
        paths: Paths {
            program: program.to_string_lossy().into_owned(),
            common_state: common_state.to_string_lossy().into_owned(),
            user_data: user_data.to_string_lossy().into_owned(),
        },
        installation_enabled: policy.is_some() && settings.bootstrap_helper.is_some(),
    })
}

pub fn open_webview_download() -> Result<(), String> {
    let operation: Vec<u16> = "open\0".encode_utf16().collect();
    let url: Vec<u16> = preflight::WEBVIEW_DOWNLOAD_URL
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: UTF-16 buffers are null terminated and live through the call. This
    // launches only the constant official URL in the default browser; no UI path,
    // executable, arguments or elevation verb is accepted.
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        return Err("browser_open_failed".into());
    }
    Ok(())
}
