//! Download a pinned, signed helper. Called only in the fixed UAC worker.
use crate::update_bridge::Settings;
use std::{
    io::{Read, Write},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
};
use update_client::windows::verify_authenticode;

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelperDownload {
    pub url: String,
    pub size_bytes: u64,
    pub sha256: String,
}

impl HelperDownload {
    fn validated_url(&self, settings: &Settings) -> Result<url::Url, String> {
        let url = url::Url::parse(&self.url).map_err(|_| "bootstrap_url_invalid")?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !url
                .host_str()
                .is_some_and(|host| settings.allowed_download_hosts.contains(host))
            || self.size_bytes == 0
            || self.size_bytes > 16 * 1024 * 1024
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("bootstrap_descriptor_invalid".into());
        }
        Ok(url)
    }

    fn download(&self, settings: &Settings) -> Result<Vec<u8>, String> {
        // The fixed UAC worker has no async runtime. Reuse the async HTTP stack
        // instead of bringing reqwest's separate blocking adapter into the EXE.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "bootstrap_client_failed")?
            .block_on(self.download_async(settings))
    }

    async fn download_async(&self, settings: &Settings) -> Result<Vec<u8>, String> {
        let url = self.validated_url(settings)?;
        let builder = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(90));
        #[cfg(feature = "local-test-tls")]
        let builder = builder.add_root_certificate(
            reqwest::Certificate::from_pem(include_bytes!(concat!(
                env!("OUT_DIR"),
                "/local-ca.pem"
            )))
            .map_err(|_| "bootstrap_ca_invalid")?,
        );
        let client = builder.build().map_err(|_| "bootstrap_client_failed")?;
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|_| "bootstrap_download_failed")?;
        if response.status() != reqwest::StatusCode::OK
            || response
                .content_length()
                .is_some_and(|length| length != self.size_bytes)
        {
            return Err("bootstrap_response_invalid".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "bootstrap_download_failed")?
        {
            if chunk.len() as u64 > self.size_bytes - bytes.len() as u64 {
                return Err("bootstrap_content_invalid".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        // Descriptor is protected by the installer signature. Never execute a
        // different signed release or a response exceeding the pinned size.
        update_client::verify_content(&bytes, self.size_bytes, &self.sha256)
            .map_err(|_| "bootstrap_content_invalid")?;
        Ok(bytes)
    }
}

pub fn register() -> Result<(), String> {
    let settings = Settings::embedded()?;
    let policy = settings.policy()?.ok_or("service_policy_missing")?;
    let executable = std::env::current_exe().map_err(|_| "bootstrap_path_failed")?;
    verify_authenticode(&executable, &policy.publisher_certificate_sha256)
        .map_err(|_| "bootstrap_publisher_invalid")?;
    let bytes = settings
        .bootstrap_helper
        .as_ref()
        .ok_or("bootstrap_descriptor_missing")?
        .download(&settings)?;
    let directory = tempfile::Builder::new()
        .prefix(&format!("{}-bootstrap-", crate::product::APP_ID))
        .tempdir()
        .map_err(|_| "bootstrap_directory_failed")?;
    let helper = directory.path().join("update-service.exe");
    let policy_path = directory.path().join("policy.json");
    for (path, content) in [
        (&helper, bytes.clone()),
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
    let output = std::process::Command::new(&helper)
        .arg("register")
        .arg(&policy_path)
        .creation_flags(0x08000000)
        .output()
        .map_err(|_| "bootstrap_launch_failed")?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("service binary differs") {
            return Err("bootstrap_service_binary_differs".into());
        }
        if error.contains("existing service policy differs") {
            return Err("bootstrap_service_policy_differs".into());
        }
        return Err("bootstrap_register_failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_rejects_untrusted_urls_and_unbounded_size() {
        let mut settings = Settings::embedded().unwrap();
        settings
            .allowed_download_hosts
            .insert("example.test".into());
        let mut descriptor = HelperDownload {
            url: "https://example.test/helper.exe".into(),
            size_bytes: 100,
            sha256: "a".repeat(64),
        };
        assert!(descriptor.validated_url(&settings).is_ok());
        for url in [
            "http://example.test/helper.exe",
            "https://other.test/helper.exe",
            "https://user:pass@example.test/helper.exe",
            "https://example.test/helper.exe#fragment",
        ] {
            descriptor.url = url.into();
            assert!(descriptor.validated_url(&settings).is_err());
        }
        descriptor.url = "https://example.test/helper.exe".into();
        for size in [0, 16 * 1024 * 1024 + 1] {
            descriptor.size_bytes = size;
            assert!(descriptor.validated_url(&settings).is_err());
        }
    }

    #[cfg(feature = "local-test-tls")]
    #[test]
    fn local_https_helper_matches_pin_and_rejects_changed_content() {
        // Opt-in network acceptance check; regular CI remains self-contained.
        if std::env::var_os("BAYKUS_TEST_BOOTSTRAP_DOWNLOAD").is_none() {
            return;
        }
        let mut settings = Settings::embedded().unwrap();
        let mut descriptor = settings.bootstrap_helper.as_ref().unwrap().clone();
        // Same local HTTPS server/certificate, independent of guest DNS setup.
        let mut url = url::Url::parse(&descriptor.url).unwrap();
        url.set_host(Some("localhost")).unwrap();
        descriptor.url = url.to_string();
        settings.allowed_download_hosts.insert("localhost".into());
        let bytes = descriptor.download(&settings).unwrap();
        assert_eq!(bytes.len() as u64, descriptor.size_bytes);
        let mut corrupt = descriptor.clone();
        corrupt.sha256 = "0".repeat(64);
        assert_eq!(
            corrupt.download(&settings).unwrap_err(),
            "bootstrap_content_invalid"
        );
        corrupt = descriptor.clone();
        corrupt.size_bytes -= 1;
        assert_eq!(
            corrupt.download(&settings).unwrap_err(),
            "bootstrap_response_invalid"
        );
    }
}
