//! Fixed, embedded, signed helper bootstrap. Called only in the UAC worker.
use crate::update_bridge::Settings;
use std::{
    io::{Read, Write},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
};
use update_client::windows::verify_authenticode;

pub fn register() -> Result<(), String> {
    let settings = Settings::embedded()?;
    let policy = settings.policy()?.ok_or("service_policy_missing")?;
    let executable = std::env::current_exe().map_err(|_| "bootstrap_path_failed")?;
    verify_authenticode(&executable, &policy.publisher_certificate_sha256)
        .map_err(|_| "bootstrap_publisher_invalid")?;
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/update-service.exe"));
    if bytes.is_empty() {
        return Err("bootstrap_helper_missing".into());
    }
    let directory = tempfile::Builder::new()
        .prefix("BaykusLig-bootstrap-")
        .tempdir()
        .map_err(|_| "bootstrap_directory_failed")?;
    let helper = directory.path().join("update-service.exe");
    let policy_path = directory.path().join("policy.json");
    for (path, content) in [
        (&helper, bytes.as_slice().to_vec()),
        (
            &policy_path,
            serde_json::to_vec_pretty(
                settings
                    .windows_policy
                    .as_ref()
                    .ok_or("service_policy_missing")?,
            )
            .map_err(|_| "service_policy_invalid")?,
        ),
    ] {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| "bootstrap_file_failed")?;
        file.write_all(&content)
            .and_then(|()| file.sync_all())
            .map_err(|_| "bootstrap_file_failed")?;
    }
    // Pin the executable against writes/deletion while validating and running it.
    let mut guard = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(&helper)
        .map_err(|_| "bootstrap_helper_invalid")?;
    if guard
        .metadata()
        .map_err(|_| "bootstrap_helper_invalid")?
        .file_attributes()
        & 0x400
        != 0
    {
        return Err("bootstrap_helper_invalid".into());
    }
    let mut actual = Vec::new();
    guard
        .read_to_end(&mut actual)
        .map_err(|_| "bootstrap_helper_invalid")?;
    if actual != bytes {
        return Err("bootstrap_helper_changed".into());
    }
    let _policy_guard = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&policy_path)
        .map_err(|_| "bootstrap_policy_invalid")?;
    verify_authenticode(&helper, &policy.publisher_certificate_sha256)
        .map_err(|_| "bootstrap_publisher_invalid")?;
    let status = std::process::Command::new(&helper)
        .arg("register")
        .arg(&policy_path)
        .creation_flags(0x08000000)
        .status()
        .map_err(|_| "bootstrap_launch_failed")?;
    if !status.success() {
        return Err("bootstrap_register_failed".into());
    }
    Ok(())
}
