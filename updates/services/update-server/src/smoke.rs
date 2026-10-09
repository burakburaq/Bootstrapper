use crate::config::Config;
use anyhow::{Context, Result, ensure};
use reqwest::StatusCode;
use semver::Version;
use std::{collections::BTreeMap, time::Duration};
use update_protocol::{Lifecycle, ReleaseStatus, UpdateResponse, sha256, verify_package};
use url::Url;

/// Read-only deployment gate. Tokens are supplied separately and never printed.
pub async fn run(
    config: &Config,
    origin: &Url,
    current: &Version,
    require_artifacts: bool,
) -> Result<()> {
    ensure!(
        origin.scheme() == "https"
            || (origin.scheme() == "http"
                && matches!(origin.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
        "HTTPS or local probe required"
    );
    let tokens: BTreeMap<String, String> = serde_json::from_str(
        &std::env::var("UPDATE_SMOKE_TOKENS").unwrap_or_else(|_| "{}".into()),
    )?;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()?;
    let mut verified_apps = 0;
    for app in &config.apps {
        let mut verified_artifact = false;
        for (channel, scope) in &app.channels {
            for target in &app.targets {
                let path = format!(
                    "/v1/apps/{}/channels/{channel}/targets/{target}/updates?current_version={current}",
                    app.app_id
                );
                let unauthenticated = http.get(origin.join(&path)?).send().await?;
                if scope.private {
                    ensure!(
                        unauthenticated.status() == StatusCode::UNAUTHORIZED,
                        "private channel accessible without authentication"
                    );
                }
                let token = tokens.get(&format!("{}/{channel}", app.app_id));
                if scope.private && token.is_none() {
                    continue;
                }
                let authorize = |r: reqwest::RequestBuilder| match token {
                    Some(t) => r.bearer_auth(t),
                    None => r,
                };
                let response = authorize(http.get(origin.join(&path)?)).send().await?;
                if response.status() == StatusCode::NO_CONTENT {
                    continue;
                }
                ensure!(
                    response.status() == StatusCode::OK,
                    "{} {channel} {target} selection failed: {}",
                    app.app_id,
                    response.status()
                );
                let r: UpdateResponse =
                    serde_json::from_slice(&bounded(response, 2 * 1024 * 1024).await?)?;
                r.bundle.validate(&app.metadata_keys)?;
                let m = &r.bundle.manifest.body;
                ensure!(
                    m.app_id == app.app_id
                        && m.channel == *channel
                        && m.target == *target
                        && m.version > *current,
                    "wrong application/channel/target selection"
                );
                let prefix = format!(
                    "/v1/apps/{}/channels/{channel}/releases/{}/targets/{target}",
                    app.app_id, m.version
                );
                let status = authorize(http.get(origin.join(&format!("{prefix}/status"))?))
                    .send()
                    .await?
                    .error_for_status()?;
                let status: ReleaseStatus =
                    serde_json::from_slice(&bounded(status, 64 * 1024).await?)?;
                ensure!(
                    status.release_id == m.release_id
                        && status.install_allowed
                        && status.lifecycle == Lifecycle::Published,
                    "release unavailable"
                );
                let tauri = authorize(http.get(origin.join(&format!(
                    "/v1/tauri/{}/{channel}/{target}/{current}",
                    app.app_id
                ))?))
                .send()
                .await?
                .error_for_status()?;
                let tauri: serde_json::Value =
                    serde_json::from_slice(&bounded(tauri, 2 * 1024 * 1024).await?)?;
                ensure!(
                    tauri["version"] == m.version.to_string() && tauri["signature"] == r.signature,
                    "Tauri/full mismatch"
                );
                let a = r
                    .bundle
                    .artifact(&m.full_artifact_id)
                    .context("full artifact missing")?;
                let redirect =
                    authorize(http.get(
                        origin.join(&format!("{prefix}/artifacts/{}/download", a.artifact_id))?,
                    ))
                    .send()
                    .await?;
                ensure!(
                    redirect.status() == StatusCode::TEMPORARY_REDIRECT,
                    "artifact redirect failed"
                );
                let storage = Url::parse(
                    redirect
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .context("missing redirect")?
                        .to_str()?,
                )?;
                ensure!(
                    storage.scheme() == "https"
                        && storage.host_str() == config.r2_endpoint.host_str(),
                    "foreign storage endpoint"
                );
                let bytes = bounded(
                    http.get(storage).send().await?.error_for_status()?,
                    a.size_bytes,
                )
                .await?;
                ensure!(
                    bytes.len() as u64 == a.size_bytes && sha256(&bytes) == a.sha256,
                    "artifact size/hash mismatch"
                );
                verify_package(&bytes, &a.package_signature, &app.package_public_key)?;
                verified_artifact = true;
            }
            let target = app.targets.first().context("target required")?;
            let admin = origin.join(&format!(
                "/v1/admin/apps/{}/channels/{channel}/releases/smoke-probe?target={target}",
                app.app_id
            ))?;
            ensure!(
                http.get(admin).send().await?.status() == StatusCode::UNAUTHORIZED,
                "unauthenticated Admin API accessible"
            );
        }
        ensure!(
            !require_artifacts || verified_artifact,
            "{} requires a published accessible smoke artifact",
            app.app_id
        );
        if verified_artifact {
            verified_apps += 1;
        }
    }
    ensure!(
        !require_artifacts || verified_apps >= 2,
        "at least two applications with signed smoke artifacts required"
    );
    println!(
        "Smoke passed: {} applications, {verified_apps} verified artifact applications",
        config.apps.len()
    );
    Ok(())
}
async fn bounded(mut response: reqwest::Response, max: u64) -> Result<Vec<u8>> {
    ensure!(
        response.content_length().is_none_or(|n| n <= max),
        "probe body too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            (bytes.len() as u64).saturating_add(chunk.len() as u64) <= max,
            "probe body too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
