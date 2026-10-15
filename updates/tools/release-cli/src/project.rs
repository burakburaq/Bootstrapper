use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Subcommand;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    env,
    path::{Path, PathBuf},
};
use tokio::fs;
use update_protocol::{KeyRing, ReleaseBundle, identifier, sha256};
use url::Url;

#[derive(Clone, Copy, Subcommand)]
pub enum Action {
    Validate,
    Prepare,
    Verify,
    Upload,
    Draft,
    Publish,
}

pub struct Options {
    pub product: PathBuf,
    pub config: PathBuf,
    pub version: Option<Version>,
    pub sequence: Option<u64>,
    pub package: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub base: Option<PathBuf>,
    pub notes: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Product {
    pub schema_version: u16,
    pub app_id: String,
    pub display_name: String,
    pub publisher: String,
    pub product_directory: String,
    pub executable: String,
    pub icon: PathBuf,
    pub description: String,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    TalaylabsV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub schema_version: u16,
    pub protocol: Protocol,
    pub server_url: Option<Url>,
    pub bucket: Option<String>,
    pub channel: String,
    pub target: String,
    pub package: PathBuf,
    pub output: PathBuf,
    pub package_signature: Option<PathBuf>,
    pub installation_contract: Option<PathBuf>,
    pub metadata_key_id: Option<String>,
}

pub struct Project {
    pub product: Product,
    pub config: Config,
    pub version: Version,
    pub sequence: Option<u64>,
    pub package: PathBuf,
    pub output: PathBuf,
    pub signature: Option<PathBuf>,
    pub contract: Option<PathBuf>,
    pub base: Option<PathBuf>,
    pub notes: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    app_id: String,
    protocol: Protocol,
    bundle_sha256: String,
    server_url: Option<Url>,
    bucket: Option<String>,
    release_id: String,
}

impl Product {
    fn validate(&self, parent: &Path) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported product schemaVersion"
        );
        ensure!(
            self.app_id.starts_with(|c: char| c.is_ascii_lowercase())
                && self.app_id.len() <= 64
                && self
                    .app_id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
            "invalid product appId"
        );
        for text in [&self.display_name, &self.publisher, &self.description] {
            ensure!(
                !text.trim().is_empty() && !text.chars().any(char::is_control),
                "invalid product text"
            );
        }
        ensure!(
            self.display_name.len() <= 128
                && self.publisher.len() <= 128
                && self.description.chars().count() <= 512,
            "product text is too long"
        );
        let dir = self.product_directory.to_ascii_lowercase();
        ensure!(
            !dir.is_empty()
                && dir.len() <= 64
                && dir
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
            "unsafe productDirectory"
        );
        ensure!(
            !["con", "prn", "aux", "nul"].contains(&dir.as_str())
                && !(dir.len() == 4
                    && (dir.starts_with("com") || dir.starts_with("lpt"))
                    && matches!(dir.as_bytes()[3], b'1'..=b'9')),
            "reserved productDirectory"
        );
        let name = self
            .executable
            .strip_suffix(".exe")
            .context("executable must end in .exe")?;
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
            "unsafe executable filename"
        );
        ensure!(
            matches!(
                self.icon.extension().and_then(|s| s.to_str()),
                Some("webp" | "png")
            ),
            "icon must be PNG or WebP"
        );
        ensure!(
            parent.join(&self.icon).is_file(),
            "product icon does not exist relative to product.json"
        );
        Ok(())
    }
}

pub fn validate_origin(url: &Url, allow_local_http: bool) -> Result<()> {
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    ensure!(
        url.scheme() == "https" || (allow_local_http && local && url.scheme() == "http"),
        "HTTPS required; explicit local HTTP is only allowed on loopback"
    );
    ensure!(
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "server must be a plain origin without credentials or query parameters"
    );
    Ok(())
}

fn resolve(parent: &Path, value: &Path, app: &str, version: &str) -> Result<PathBuf> {
    let text = value
        .to_str()
        .context("configuration paths must be Unicode")?
        .replace("{appId}", app)
        .replace("{version}", version);
    ensure!(
        !text.contains(['{', '}']),
        "unknown path template; only {{appId}} and {{version}} are supported"
    );
    Ok(parent.join(text))
}

impl Project {
    async fn load(options: Options, action: Action) -> Result<Self> {
        let product_path = fs::canonicalize(&options.product)
            .await
            .context("cannot open product configuration")?;
        let config_path = fs::canonicalize(&options.config)
            .await
            .context("cannot open release configuration")?;
        let product: Product = serde_json::from_slice(&fs::read(&product_path).await?)?;
        let config: Config = serde_json::from_slice(&fs::read(&config_path).await?)?;
        product.validate(product_path.parent().context("product directory missing")?)?;
        ensure!(
            config.schema_version == 1,
            "unsupported release schemaVersion"
        );
        identifier(&config.channel)?;
        identifier(&config.target)?;
        if let Some(bucket) = &config.bucket {
            ensure!(
                !bucket.trim().is_empty() && !bucket.contains(['/', '\\']),
                "invalid bucket"
            );
        }
        if let Some(server) = &config.server_url {
            validate_origin(server, false)?;
        }
        identifier(
            config
                .metadata_key_id
                .as_deref()
                .context("metadataKeyId is required")?,
        )?;
        if !matches!(action, Action::Validate) {
            ensure!(
                options.version.is_some(),
                "--version is required for release operations"
            );
        }
        let version = options.version.unwrap_or_else(|| Version::new(0, 0, 0));
        let parent = config_path.parent().context("release directory missing")?;
        let expand = |v: &Path| resolve(parent, v, &product.app_id, &version.to_string());
        let package = match options.package {
            Some(p) => env::current_dir()?.join(p),
            None => expand(&config.package)?,
        };
        let output = match options.output {
            Some(p) => env::current_dir()?.join(p),
            None => expand(&config.output)?,
        };
        let signature = config
            .package_signature
            .as_deref()
            .map(expand)
            .transpose()?;
        let contract = config
            .installation_contract
            .as_deref()
            .map(expand)
            .transpose()?;
        Ok(Self {
            product,
            config,
            version,
            sequence: options.sequence,
            package,
            output,
            signature,
            contract,
            base: options
                .base
                .map(|p| env::current_dir().map(|cwd| cwd.join(p)))
                .transpose()?,
            notes: options.notes,
        })
    }
    pub fn bundle_path(&self) -> PathBuf {
        self.output.join("bundle.json")
    }
    pub fn server(&self) -> Result<&Url> {
        self.config
            .server_url
            .as_ref()
            .context("set serverUrl in release/config.json before draft/publish")
    }
    pub fn bucket(&self) -> Result<&str> {
        self.config
            .bucket
            .as_deref()
            .context("set bucket in release/config.json before upload")
    }
    pub async fn native_bundle(&self) -> Result<ReleaseBundle> {
        let b: ReleaseBundle = serde_json::from_slice(&fs::read(self.bundle_path()).await?)?;
        b.validate(&public_keys().await?)?;
        let m = &b.manifest.body;
        ensure!(
            m.app_id == self.product.app_id
                && m.channel == self.config.channel
                && m.target == self.config.target
                && m.version == self.version,
            "prepared bundle belongs to another product, channel, target or version"
        );
        if let Some(sequence) = self.sequence {
            ensure!(
                m.sequence == sequence,
                "prepared sequence differs from requested sequence"
            );
        }
        if let Some(contract) = &m.installation {
            ensure!(
                contract.executable == self.product.executable,
                "installation contract differs from product.json"
            );
        }
        Ok(b)
    }
    async fn verify(&self) -> Result<()> {
        let b = self.native_bundle().await?;
        for a in b.artifacts {
            let bytes = fs::read(self.output.join(format!("{}.bin", a.body.artifact_id))).await?;
            update_client::verify_content(&bytes, a.body.size_bytes, &a.body.sha256)?;
        }
        Ok(())
    }
    async fn release_id(&self) -> Result<String> {
        Ok(self.native_bundle().await?.manifest.body.release_id)
    }
    async fn receipt(&self) -> Result<Receipt> {
        Ok(Receipt {
            app_id: self.product.app_id.clone(),
            protocol: self.config.protocol,
            bundle_sha256: sha256(&fs::read(self.bundle_path()).await?),
            server_url: self.config.server_url.clone(),
            bucket: self.config.bucket.clone(),
            release_id: self.release_id().await?,
        })
    }
    async fn check_receipt(&self, name: &str) -> Result<()> {
        let old: Receipt = serde_json::from_slice(
            &fs::read(self.output.join(name))
                .await
                .context("required previous release stage receipt is missing")?,
        )?;
        let current = self.receipt().await?;
        ensure!(
            old.app_id == current.app_id
                && old.protocol == current.protocol
                && old.bundle_sha256 == current.bundle_sha256
                && old.server_url == current.server_url
                && old.bucket == current.bucket
                && old.release_id == current.release_id,
            "stage receipt differs from this product, environment or prepared artifact"
        );
        Ok(())
    }
    async fn record(&self, name: &str) -> Result<()> {
        let path = self.output.join(name);
        if path.exists() {
            self.check_receipt(name).await
        } else {
            crate::save(&path, &self.receipt().await?).await
        }
    }
}

pub fn secret(name: &str) -> Result<String> {
    let value = if let Ok(path) = env::var(format!("{name}_FILE")) {
        std::fs::read_to_string(path).with_context(|| format!("cannot read {name}_FILE"))?
    } else {
        env::var(name).with_context(|| format!("set {name} or {name}_FILE"))?
    };
    ensure!(!value.trim().is_empty(), "{name} is empty");
    Ok(value.trim().to_owned())
}
pub fn key_path(name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(env::var(name).with_context(|| {
        format!("set {name} to a local key file")
    })?))
}
pub async fn public_keys() -> Result<KeyRing> {
    let bytes = fs::read(key_path("RELEASE_METADATA_PUBLIC_KEY_FILE")?).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
pub async fn signing_key(project: &Project) -> Result<(String, ed25519_dalek::SigningKey)> {
    let key = crate::key(&key_path("RELEASE_METADATA_KEY_FILE")?).await?;
    let id = project
        .config
        .metadata_key_id
        .clone()
        .context("metadataKeyId is required")?;
    ensure!(
        public_keys().await?.get(&id) == Some(&STANDARD.encode(key.verifying_key().to_bytes())),
        "signing key does not match the trusted metadata public key"
    );
    Ok((id, key))
}

struct StageLock(PathBuf);
impl Drop for StageLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn stage_lock(output: &Path) -> Result<StageLock> {
    let path = output.join("operation.lock");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .context(
            "another release operation is active; inspect any stale operation.lock before retrying",
        )?;
    use std::io::Write;
    writeln!(file, "{}", std::process::id())?;
    Ok(StageLock(path))
}

pub async fn run(options: Options, action: Action, ca: Option<&Path>) -> Result<()> {
    let project = Project::load(options, action).await?;
    if matches!(action, Action::Validate) {
        println!(
            "{}",
            serde_json::json!({"appId":project.product.app_id,"applicationIdentifier":format!("com.talaylabs.{}",project.product.app_id),
            "protocol":project.config.protocol,"channel":project.config.channel,"target":project.config.target,
            "publishingConfigured":project.config.server_url.is_some() && project.config.bucket.is_some()})
        );
        return Ok(());
    }
    if matches!(action, Action::Prepare) {
        ensure!(
            !project.output.exists(),
            "output already exists; use a new immutable preparation directory"
        );
        let (id, _) = signing_key(&project).await?;
        let sequence = project
            .sequence
            .context("--sequence is required for native prepare")?;
        if let Some(path) = &project.contract {
            let contract: update_protocol::InstallContract =
                serde_json::from_slice(&fs::read(path).await?)?;
            ensure!(
                contract.executable == project.product.executable,
                "installation contract differs from product.json"
            );
        }
        crate::execute(
            crate::Command::Prepare {
                app_id: project.product.app_id.clone(),
                channel: project.config.channel.clone(),
                target: project.config.target.clone(),
                version: project.version.clone(),
                sequence,
                package: project.package.clone(),
                package_signature: project
                    .signature
                    .clone()
                    .context("packageSignature is required")?,
                package_public_key: key_path("RELEASE_PACKAGE_PUBLIC_KEY_FILE")?,
                metadata_key: key_path("RELEASE_METADATA_KEY_FILE")?,
                key_id: id,
                out: project.output.clone(),
                base: project.base.clone(),
                notes: project.notes.clone(),
                minimum_supported_version: None,
                checkpoint: vec![],
                build_program: None,
                build_arg: vec![],
                installation_contract: project.contract.clone(),
            },
            ca,
        )
        .await?;
        project.verify().await?;
        return Ok(());
    }
    project.verify().await?;
    if matches!(action, Action::Verify) {
        println!("Prepared metadata and package bytes verified");
        return Ok(());
    }
    let _lock = stage_lock(&project.output)?;
    match action {
        Action::Upload => {
            let _ = project.server()?;
            crate::execute(
                crate::Command::Upload {
                    bundle: project.bundle_path(),
                    files: project.output.clone(),
                    bucket: project.bucket()?.into(),
                    endpoint: secret("R2_ENDPOINT")?.parse()?,
                    access_key: secret("R2_ACCESS_KEY_ID")?,
                    secret_key: secret("R2_SECRET_ACCESS_KEY")?,
                    public_key: key_path("RELEASE_METADATA_PUBLIC_KEY_FILE")?,
                },
                ca,
            )
            .await?;
            project.record("upload.json").await?;
        }
        Action::Draft => {
            let _ = project.server()?;
            project.check_receipt("upload.json").await?;
            if project.output.join("draft.json").exists() {
                project.check_receipt("draft.json").await?;
                return Ok(());
            }
            crate::execute(
                crate::Command::Draft {
                    server: project.server()?.clone(),
                    token: secret("UPDATE_ADMIN_TOKEN")?,
                    bundle: project.bundle_path(),
                },
                ca,
            )
            .await?;
            project.record("draft.json").await?;
        }
        Action::Publish => {
            project.check_receipt("draft.json").await?;
            let id = project.release_id().await?;
            crate::execute(
                crate::Command::Action {
                    server: project.server()?.clone(),
                    token: secret("UPDATE_ADMIN_TOKEN")?,
                    app_id: project.product.app_id.clone(),
                    channel: project.config.channel.clone(),
                    target: project.config.target.clone(),
                    release_id: id.clone(),
                    action: "publish".into(),
                    operation_id: format!("publish-{id}"),
                    percentage: None,
                    cohort_key: None,
                    revision: None,
                    paused: false,
                },
                ca,
            )
            .await?;
            project.record("publish.json").await?;
        }
        _ => bail!("unexpected release stage"),
    }
    Ok(())
}
