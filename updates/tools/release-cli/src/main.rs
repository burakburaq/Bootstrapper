use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use semver::Version;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use update_protocol::*;
use url::Url;
use uuid::Uuid;

#[derive(Parser)]
#[command(
    version,
    about = "Offline signing, immutable full/delta packages and scoped release administration"
)]
struct Args {
    /// Additional trusted CA for HTTPS administration (for example a local test server).
    #[arg(long, global = true)]
    ca_certificate: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Sign bootstrap policy with the offline release key embedded as a public root in the helper.
    SignWindowsPolicy {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        metadata_key: PathBuf,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Generate a stage/activate NSIS package script and inventory from FINAL signed payload files.
    WindowsPackage {
        /// Signed installer UI included in the payload, with --repair/--uninstall support.
        #[arg(long)]
        maintenance_executable: Option<String>,
        #[arg(long)]
        payload: PathBuf,
        #[arg(long)]
        app_id: String,
        #[arg(long)]
        product_directory: String,
        #[arg(long)]
        executable: String,
        #[arg(long)]
        version: Version,
        #[arg(long)]
        publisher_certificate_sha256: String,
        #[arg(long, default_value_t = 10240)]
        minimum_windows_build: u32,
        #[arg(long, default_value = "0.0.0.0")]
        minimum_webview2_version: String,
        #[arg(long)]
        out: PathBuf,
    },
    Keygen {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        key_id: String,
    },
    TokenHash {
        #[arg(long, env = "UPDATE_TOKEN")]
        token: String,
    },
    Prepare {
        #[arg(long)]
        app_id: String,
        #[arg(long)]
        channel: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        version: Version,
        #[arg(long)]
        sequence: u64,
        #[arg(long)]
        package: PathBuf,
        #[arg(long)]
        package_signature: PathBuf,
        #[arg(long)]
        package_public_key: PathBuf,
        #[arg(long)]
        metadata_key: PathBuf,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        base: Option<PathBuf>,
        #[arg(long, default_value = "")]
        notes: String,
        #[arg(long)]
        minimum_supported_version: Option<Version>,
        #[arg(long)]
        checkpoint: Vec<String>,
        #[arg(long)]
        build_program: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        build_arg: Vec<String>,
        /// Validated inventory and preflight budget, generated from final signed payload files.
        #[arg(long)]
        installation_contract: Option<PathBuf>,
    },
    Upload {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        files: PathBuf,
        #[arg(long)]
        bucket: String,
        #[arg(long, env = "R2_ENDPOINT")]
        endpoint: Url,
        #[arg(long, env = "R2_ACCESS_KEY_ID")]
        access_key: String,
        #[arg(long, env = "R2_SECRET_ACCESS_KEY")]
        secret_key: String,
        #[arg(long)]
        public_key: PathBuf,
    },
    Draft {
        #[arg(long)]
        server: Url,
        #[arg(long, env = "UPDATE_ADMIN_TOKEN")]
        token: String,
        #[arg(long)]
        bundle: PathBuf,
    },
    Action {
        #[arg(long)]
        server: Url,
        #[arg(long, env = "UPDATE_ADMIN_TOKEN")]
        token: String,
        #[arg(long)]
        app_id: String,
        #[arg(long)]
        channel: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        release_id: String,
        #[arg(long)]
        action: String,
        #[arg(long)]
        operation_id: String,
        #[arg(long)]
        percentage: Option<u8>,
        #[arg(long)]
        cohort_key: Option<String>,
        #[arg(long)]
        revision: Option<u64>,
        #[arg(long)]
        paused: bool,
    },
    Promote {
        #[arg(long)]
        server: Url,
        #[arg(long, env = "UPDATE_ADMIN_TOKEN")]
        token: String,
        #[arg(long)]
        source_bundle: PathBuf,
        #[arg(long)]
        channel: String,
        #[arg(long)]
        sequence: u64,
        #[arg(long)]
        release_id: Option<String>,
        #[arg(long)]
        metadata_key: PathBuf,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        checkpoint: Vec<String>,
        #[arg(long)]
        public_key: PathBuf,
    },
    Verify {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        public_key: PathBuf,
    },
}
async fn bundle(path: &Path) -> Result<ReleaseBundle> {
    Ok(serde_json::from_slice(&tokio::fs::read(path).await?)?)
}
async fn key(path: &Path) -> Result<SigningKey> {
    let value = tokio::fs::read_to_string(path).await?;
    let bytes: [u8; 32] = hex_decode(value.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("metadata key must be 32 bytes"))?;
    Ok(SigningKey::from_bytes(&bytes))
}
fn hex_decode(value: &str) -> Result<Vec<u8>> {
    ensure!(
        value.len().is_multiple_of(2) && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid hex"
    );
    (0..value.len())
        .step_by(2)
        .map(|i| Ok(u8::from_str_radix(&value[i..i + 2], 16)?))
        .collect()
}
fn key_ring(key_id: &str, public: &str) -> KeyRing {
    BTreeMap::from([(key_id.into(), public.trim().into())])
}
async fn save(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(&serde_json::to_vec_pretty(value)?).await?;
    file.sync_all().await?;
    Ok(())
}
fn control(server: &Url, path: &str) -> Result<Url> {
    ensure!(
        server.scheme() == "https" && server.username().is_empty() && server.password().is_none(),
        "HTTPS control server required"
    );
    Ok(server.join(path)?)
}
async fn admin(
    server: &Url,
    path: &str,
    token: &str,
    value: &impl serde::Serialize,
    ca_certificate: Option<&Path>,
) -> Result<serde_json::Value> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(600));
    if let Some(path) = ca_certificate {
        let certificate = reqwest::Certificate::from_pem(&tokio::fs::read(path).await?)?;
        builder = builder.add_root_certificate(certificate);
    }
    let client = builder.build()?;
    let response = client
        .post(control(server, path)?)
        .bearer_auth(token)
        .json(value)
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "admin API rejected operation: {}",
        response.status()
    );
    Ok(response.json().await?)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        Command::SignWindowsPolicy {
            policy,
            metadata_key,
            key_id,
            out,
        } => {
            #[derive(serde::Serialize)]
            #[serde(transparent)]
            struct Policy(serde_json::Value);
            impl Signable for Policy {
                const DOMAIN: &'static str = "windows-service-policy";
            }
            identifier(&key_id)?;
            let body: serde_json::Value = serde_json::from_slice(&tokio::fs::read(policy).await?)?;
            ensure!(body.is_object(), "policy must be a JSON object");
            let signing = key(&metadata_key).await?;
            save(&out, &sign(Policy(body), key_id, &signing)?).await?;
            println!("Signed Windows helper policy");
        }
        Command::WindowsPackage {
            maintenance_executable,
            payload,
            app_id,
            product_directory,
            executable,
            version,
            publisher_certificate_sha256,
            minimum_windows_build,
            minimum_webview2_version,
            out,
        } => {
            identifier(&app_id)?;
            identifier(&product_directory)?;
            relative_payload_path(&executable)?;
            ensure!(
                !product_directory.contains('.') && !executable.contains('/'),
                "simple fixed product/main EXE names required"
            );
            ensure!(
                version.major <= 65535
                    && version.minor <= 65535
                    && version.patch <= 65535
                    && version.build.is_empty(),
                "unsupported Windows version"
            );
            let runtime = minimum_webview2_version
                .split('.')
                .map(str::parse::<u32>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let runtime: [u32; 4] = runtime
                .try_into()
                .map_err(|_| anyhow::anyhow!("WebView2 version needs four components"))?;
            let payload = std::fs::canonicalize(payload)?;
            let files = update_client::inventory::build_inventory(&payload)?;
            let total = files.iter().try_fold(0u64, |n, f| {
                n.checked_add(f.size_bytes).context("payload too large")
            })?;
            let mut contract = InstallContract {
                schema_version: 1,
                executable,
                publisher_certificate_sha256,
                minimum_windows_build,
                minimum_webview2_version: runtime,
                staging_bytes: total,
                active_bytes: total,
                backup_bytes: total,
                files,
            };
            contract.validate()?;
            let maintenance_registration = if let Some(executable) = maintenance_executable {
                relative_payload_path(&executable)?;
                ensure!(
                    !executable.contains('/')
                        && executable.ends_with(".exe")
                        && contract.files.iter().any(|file| file.path == executable),
                    "maintenance EXE must be a root file in the signed payload inventory"
                );
                let maintenance_file = contract
                    .files
                    .iter()
                    .find(|file| file.path == executable)
                    .expect("validated above");
                contract.active_bytes = contract
                    .active_bytes
                    .checked_add(maintenance_file.size_bytes)
                    .context("maintenance disk budget overflow")?;
                let maintenance_hash = &maintenance_file.sha256;
                let maintenance_name = format!("maintenance-{maintenance_hash}.exe");
                format!(
                    r#"  IfFileExists "$PROGRAMFILES64\{product_directory} Update Service\{maintenance_name}" maintenance_ready
  CopyFiles /SILENT "$Active\{executable}" "$PROGRAMFILES64\{product_directory} Update Service\{maintenance_name}"
  IfErrors fail_activate
  maintenance_ready:
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\{app_id}" "ModifyPath" '$\"$PROGRAMFILES64\{product_directory} Update Service\{maintenance_name}$\" --repair'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\{app_id}" "UninstallString" '$\"$PROGRAMFILES64\{product_directory} Update Service\{maintenance_name}$\" --uninstall'
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\{app_id}" "NoModify" 0"#
                )
            } else {
                format!(
                    r#"  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\{app_id}" "UninstallString" '$\"$PROGRAMFILES64\{product_directory} Update Service\update-service.exe$\" uninstall'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\{app_id}" "ModifyPath" ""
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\{app_id}" "NoModify" 1"#
                )
            };
            contract.validate()?;
            ensure!(!out.exists(), "output directory must be new");
            std::fs::create_dir_all(&out)?;
            save(&out.join("contract.json"), &contract).await?;
            let mut extraction = String::new();
            for file in &contract.files {
                let relative = Path::new(&file.path);
                let directory = relative
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| format!("\\{}", p.display()))
                    .unwrap_or_default();
                let source = payload.join(relative);
                ensure!(
                    !source.to_string_lossy().contains(['$', '"', '\n', '\r']),
                    "unsupported NSIS source path"
                );
                extraction.push_str(&format!(
                    "  SetOutPath \"$Stage{directory}\"\n  File \"{}\"\n",
                    source.display()
                ));
            }
            let template = include_str!("windows-package.nsi");
            let script = template
                .replace("@APP_ID@", &app_id)
                .replace("@PRODUCT_DIRECTORY@", &product_directory)
                .replace("@EXECUTABLE@", &contract.executable)
                .replace("@MAINTENANCE_REGISTRATION@", &maintenance_registration)
                .replace("@VERSION@", &version.to_string())
                .replace(
                    "@VERSION_FIXED@",
                    &format!("{}.{}.{}.0", version.major, version.minor, version.patch),
                )
                .replace("@EXTRACTION@", &extraction);
            tokio::fs::write(out.join("package.nsi"), script).await?;
            println!(
                "Generated {}. Compile with makensis, Authenticode-sign package.exe, then updater-sign and prepare --installation-contract contract.json.",
                out.display()
            );
        }
        Command::Keygen { out, key_id } => {
            identifier(&key_id)?;
            let signing = SigningKey::generate(&mut OsRng);
            if let Some(parent) = out.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            let mut file = options.open(&out)?;
            for byte in signing.to_bytes() {
                write!(file, "{byte:02x}")?;
            }
            file.sync_all()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&key_ring(
                    &key_id,
                    &STANDARD.encode(signing.verifying_key().to_bytes())
                ))?
            );
        }
        Command::TokenHash { token } => println!("{}", sha256(token.as_bytes())),
        Command::Prepare {
            app_id,
            channel,
            target,
            version,
            sequence,
            package,
            package_signature,
            package_public_key,
            metadata_key,
            key_id,
            out,
            base,
            notes,
            minimum_supported_version,
            checkpoint,
            build_program,
            build_arg,
            installation_contract,
        } => {
            if let Some(program) = build_program {
                ensure!(
                    tokio::process::Command::new(program)
                        .args(build_arg)
                        .status()
                        .await?
                        .success(),
                    "build command failed"
                );
            }
            for id in [&app_id, &channel, &target, &key_id] {
                identifier(id)?;
            }
            let signing = key(&metadata_key).await?;
            let bytes = tokio::fs::read(&package).await?;
            let signature = tokio::fs::read_to_string(&package_signature)
                .await?
                .trim()
                .to_owned();
            verify_package(
                &bytes,
                &signature,
                &tokio::fs::read_to_string(&package_public_key).await?,
            )?;
            let digest = sha256(&bytes);
            let id = format!("full-{digest}");
            let descriptor = ArtifactDescriptor {
                schema_version: SCHEMA_VERSION,
                app_id: app_id.clone(),
                target: target.clone(),
                artifact_id: id.clone(),
                version: version.clone(),
                object_key: format!("{app_id}/{target}/{digest}.package"),
                sha256: digest.clone(),
                size_bytes: bytes.len() as u64,
                artifact: ArtifactKind::Full,
                package_signature: signature.clone(),
            };
            ensure!(
                !out.exists(),
                "output directory already exists; use a new immutable preparation directory"
            );
            tokio::fs::create_dir_all(&out).await?;
            tokio::fs::write(out.join(format!("{id}.bin")), &bytes).await?;
            let mut artifacts = vec![sign(descriptor, key_id.clone(), &signing)?];
            if let Some(base) = base {
                let base = tokio::fs::read(base).await?;
                let patch = update_client::delta::create(&base, &bytes)?;
                let result = update_client::delta::apply(
                    &base,
                    &patch,
                    bytes.len() as u64,
                    256 * 1024 * 1024,
                )?;
                ensure!(result == bytes, "delta roundtrip failed");
                if patch.len() < bytes.len() {
                    let hash = sha256(&patch);
                    let delta_id = format!("delta-{hash}");
                    let d = ArtifactDescriptor {
                        schema_version: SCHEMA_VERSION,
                        app_id: app_id.clone(),
                        target: target.clone(),
                        artifact_id: delta_id.clone(),
                        version: version.clone(),
                        object_key: format!("{app_id}/{target}/{hash}.zstd"),
                        sha256: hash,
                        size_bytes: patch.len() as u64,
                        package_signature: signature,
                        artifact: ArtifactKind::Delta {
                            base_sha256: sha256(&base),
                            base_size_bytes: base.len() as u64,
                            result_sha256: digest,
                            result_size_bytes: bytes.len() as u64,
                            full_artifact_id: id.clone(),
                        },
                    };
                    tokio::fs::write(out.join(format!("{delta_id}.bin")), patch).await?;
                    artifacts.push(sign(d, key_id.clone(), &signing)?);
                }
            }
            let required_checkpoints = checkpoint
                .into_iter()
                .map(|v| -> Result<Checkpoint> {
                    let (version, id) = v
                        .split_once(':')
                        .context("checkpoint syntax: version:release_id")?;
                    Ok(Checkpoint {
                        version: Version::parse(version)?,
                        release_id: id.into(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let references = artifacts
                .iter()
                .map(|a| Ok((a.body.artifact_id.clone(), sha256(&canonical(&a.body)?))))
                .collect::<Result<BTreeMap<_, _>>>()?;
            let manifest = ReleaseManifest {
                schema_version: SCHEMA_VERSION,
                app_id,
                channel,
                target,
                release_id: Uuid::new_v4().to_string(),
                version,
                sequence,
                published_at: Utc::now(),
                notes,
                minimum_supported_version,
                required_checkpoints,
                full_artifact_id: id,
                artifacts: references,
                installation: match installation_contract {
                    Some(path) => {
                        let contract: InstallContract =
                            serde_json::from_slice(&tokio::fs::read(path).await?)?;
                        contract.validate()?;
                        Some(contract)
                    }
                    None => None,
                },
            };
            let bundle = ReleaseBundle {
                manifest: sign(manifest, key_id.clone(), &signing)?,
                artifacts,
            };
            bundle.validate(&key_ring(
                &key_id,
                &STANDARD.encode(signing.verifying_key().to_bytes()),
            ))?;
            save(&out.join("bundle.json"), &bundle).await?;
            println!("Prepared {}", out.display());
        }
        Command::Verify {
            bundle: path,
            public_key,
        } => {
            let bundle = bundle(&path).await?;
            let keys: KeyRing = serde_json::from_slice(&tokio::fs::read(public_key).await?)?;
            bundle.validate(&keys)?;
            println!("Metadata verified");
        }
        Command::Upload {
            bundle: path,
            files,
            bucket,
            endpoint,
            access_key,
            secret_key,
            public_key,
        } => {
            ensure!(
                endpoint.scheme() == "https"
                    && endpoint
                        .host_str()
                        .is_some_and(|h| h.ends_with(".r2.cloudflarestorage.com")),
                "R2 S3 endpoint required"
            );
            let bundle = bundle(&path).await?;
            let keys: KeyRing = serde_json::from_slice(&tokio::fs::read(public_key).await?)?;
            bundle.validate(&keys)?;
            let credentials = s3::creds::Credentials::new(
                Some(&access_key),
                Some(&secret_key),
                None,
                None,
                None,
            )?;
            let bucket = s3::Bucket::new(
                &bucket,
                s3::Region::Custom {
                    region: "auto".into(),
                    endpoint: endpoint.to_string().trim_end_matches('/').into(),
                },
                credentials,
            )?
            .with_path_style();
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(600))
                .build()?;
            for signed in &bundle.artifacts {
                let a = &signed.body;
                let bytes = tokio::fs::read(files.join(format!("{}.bin", a.artifact_id))).await?;
                update_client::verify_content(&bytes, a.size_bytes, &a.sha256)?;
                // Conditional PUT is part of the upload request: no HEAD-then-overwrite race.
                let url = bucket.presign_put(&a.object_key, 600, None, None).await?;
                let response = client
                    .put(url)
                    .header("If-None-Match", "*")
                    .body(bytes.clone())
                    .send()
                    .await?;
                if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
                    let url = bucket.presign_get(&a.object_key, 600, None).await?;
                    let mut response = client.get(url).send().await?.error_for_status()?;
                    ensure!(
                        response.content_length().is_none_or(|n| n <= a.size_bytes),
                        "existing R2 object too large"
                    );
                    let mut existing = Vec::new();
                    while let Some(chunk) = response.chunk().await? {
                        ensure!(
                            (existing.len() as u64).saturating_add(chunk.len() as u64)
                                <= a.size_bytes,
                            "existing R2 object too large"
                        );
                        existing.extend_from_slice(&chunk);
                    }
                    update_client::verify_content(&existing, a.size_bytes, &a.sha256)?;
                } else {
                    ensure!(
                        response.status().is_success(),
                        "R2 immutable upload failed: {}",
                        response.status()
                    );
                }
                println!("Uploaded {}", a.artifact_id);
            }
            let key = format!("{}/health.txt", bundle.manifest.body.app_id);
            let url = bucket.presign_put(&key, 600, None, None).await?;
            let response = client
                .put(url)
                .header("If-None-Match", "*")
                .body("ok\n")
                .send()
                .await?;
            ensure!(
                response.status().is_success()
                    || response.status() == reqwest::StatusCode::PRECONDITION_FAILED,
                "health object upload failed"
            );
        }
        Command::Draft {
            server,
            token,
            bundle: path,
        } => {
            let bundle = bundle(&path).await?;
            let m = &bundle.manifest.body;
            let path = format!("/v1/admin/apps/{}/channels/{}/drafts", m.app_id, m.channel);
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &admin(
                        &server,
                        &path,
                        &token,
                        &DraftRequest { bundle },
                        args.ca_certificate.as_deref()
                    )
                    .await?
                )?
            );
        }
        Command::Action {
            server,
            token,
            app_id,
            channel,
            target,
            release_id,
            action,
            operation_id,
            percentage,
            cohort_key,
            revision,
            paused,
        } => {
            for id in [&app_id, &channel, &target, &release_id, &operation_id] {
                identifier(id)?;
            }
            ensure!(
                ["publish", "disable", "enable", "revoke", "rollout"].contains(&action.as_str()),
                "unknown action"
            );
            let policy = if action == "rollout" {
                Some(RolloutPolicy {
                    percentage: percentage.context("percentage required")?,
                    cohort_key: cohort_key.context("cohort key required")?,
                    revision: revision.context("revision required")?,
                    paused,
                })
            } else {
                None
            };
            if let Some(policy) = &policy {
                policy.validate()?;
            }
            let path = format!(
                "/v1/admin/apps/{app_id}/channels/{channel}/releases/{release_id}?target={target}"
            );
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &admin(
                        &server,
                        &path,
                        &token,
                        &AdminAction {
                            operation_id,
                            action,
                            policy
                        },
                        args.ca_certificate.as_deref(),
                    )
                    .await?
                )?
            );
        }
        Command::Promote {
            server,
            token,
            source_bundle,
            channel,
            sequence,
            release_id,
            metadata_key,
            key_id,
            out,
            checkpoint,
            public_key,
        } => {
            identifier(&channel)?;
            let source = bundle(&source_bundle).await?;
            let signing = key(&metadata_key).await?;
            let keys: KeyRing = serde_json::from_slice(&tokio::fs::read(public_key).await?)?;
            source.validate(&keys)?;
            let mut manifest = source.manifest.body.clone();
            let source_channel = manifest.channel.clone();
            let source_release_id = manifest.release_id.clone();
            manifest.channel = channel;
            manifest.sequence = sequence;
            manifest.release_id = release_id.unwrap_or_else(|| Uuid::new_v4().to_string());
            manifest.published_at = Utc::now();
            ensure!(
                manifest.required_checkpoints.is_empty() || !checkpoint.is_empty(),
                "provide target-channel checkpoints for promotion"
            );
            manifest.required_checkpoints = checkpoint
                .into_iter()
                .map(|v| -> Result<Checkpoint> {
                    let (version, id) = v
                        .split_once(':')
                        .context("checkpoint syntax: version:release_id")?;
                    Ok(Checkpoint {
                        version: Version::parse(version)?,
                        release_id: id.into(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let bundle = ReleaseBundle {
                manifest: sign(manifest, key_id.clone(), &signing)?,
                artifacts: source.artifacts,
            };
            bundle.validate(&keys)?;
            save(&out, &bundle).await?;
            let m = &bundle.manifest.body;
            let path = format!("/v1/admin/apps/{}/channels/{}/promote", m.app_id, m.channel);
            let request = serde_json::json!({"source_channel":source_channel,"source_release_id":source_release_id,"bundle":bundle});
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &admin(
                        &server,
                        &path,
                        &token,
                        &request,
                        args.ca_certificate.as_deref()
                    )
                    .await?
                )?
            );
        }
    }
    Ok(())
}
