//! Local HTTPS smoke check; accepts public server config and the exported CA only.
use anyhow::{Context, Result, ensure};
use std::collections::BTreeSet;
use update_client::{ClientConfig, UpdateClient};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 3 || args.len() == 4,
        "usage: check-local <server-config.json> <root.crt> [https-origin]"
    );
    let config: serde_json::Value = serde_json::from_slice(&tokio::fs::read(&args[1]).await?)?;
    let app = config["apps"]
        .as_array()
        .and_then(|apps| apps.iter().find(|app| app["app_id"] == "baykuslig"))
        .context("baykuslig app missing")?;
    let origin = args
        .get(3)
        .map(String::as_str)
        .or_else(|| config["public_url"].as_str())
        .context("public_url missing")?;
    let endpoint: url::Url = config["r2_endpoint"]
        .as_str()
        .context("r2_endpoint missing")?
        .parse()?;
    let cache = tempfile::tempdir()?;
    let client = UpdateClient::new_with_local_ca(
        ClientConfig {
            server_url: origin.parse()?,
            app_id: "baykuslig".into(),
            channel: "production".into(),
            target: "windows-x86_64-nsis".into(),
            metadata_keys: serde_json::from_value(app["metadata_keys"].clone())?,
            package_public_key: app["package_public_key"]
                .as_str()
                .context("package_public_key missing")?
                .into(),
            channel_token: None,
            allowed_download_hosts: BTreeSet::from([endpoint
                .host_str()
                .context("R2 host missing")?
                .into()]),
            cache_dir: cache.path().join("cache"),
            max_artifact_bytes: 256 * 1024 * 1024,
            allow_loopback_http: false,
        },
        &tokio::fs::read(&args[2]).await?,
    )?;
    let result = client
        .check("0.0.0".parse()?, uuid::Uuid::new_v4().to_string(), None)
        .await?;
    match result {
        Some(update) => println!("HTTPS verified; update {}", update.version()),
        None => println!("HTTPS verified; no published update"),
    }
    Ok(())
}
