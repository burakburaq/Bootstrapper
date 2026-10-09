use crate::{
    config::{AppConfig, Config},
    repository::SharedRepository,
    storage::SharedStorage,
};
use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use semver::Version;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use update_protocol::*;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub repository: SharedRepository,
    pub storage: SharedStorage,
    pub draining: Arc<AtomicBool>,
    pub inflight: Arc<AtomicUsize>,
    pub instance: String,
}
pub fn router(state: AppState) -> Router {
    router_with_authorization(state, None)
}
/// This operational key signs nonce-bound install permits, never release metadata.
pub struct AuthorizationSigner {
    pub key_id: String,
    pub key: ed25519_dalek::SigningKey,
}
pub fn router_with_authorization(
    state: AppState,
    signer: Option<Arc<AuthorizationSigner>>,
) -> Router {
    Router::new()
        .route("/health/live",get(live)).route("/health/ready",get(ready)).route("/health/inflight",get(inflight))
        .route("/v1/update/check",post(check))
        .route("/v1/install/resolve",post(install_resolve))
        .route("/v1/install/authorize",post(install_authorize))
        .route("/v1/apps/{app}/channels/{channel}/targets/{target}/updates",get(common_get))
        .route("/v1/tauri/{app}/{channel}/{target}/{current}",get(tauri_get))
        .route("/v1/apps/{app}/channels/{channel}/releases/{version}/targets/{target}/download",get(full_download))
        .route("/v1/apps/{app}/channels/{channel}/releases/{version}/targets/{target}/artifacts/{artifact}/download",get(download))
        .route("/v1/apps/{app}/channels/{channel}/releases/{version}/targets/{target}/status",get(status))
        .route("/v1/admin/apps/{app}/channels/{channel}/drafts",post(draft))
        .route("/v1/admin/apps/{app}/channels/{channel}/promote",post(promote))
        .route("/v1/admin/apps/{app}/channels/{channel}/releases/{release}",get(admin_get).post(action))
        .layer(DefaultBodyLimit::max(2*1024*1024))
        .layer(middleware::from_fn_with_state(state.clone(),track))
        .layer(Extension(signer))
        .with_state(state)
}
#[derive(Debug)]
pub struct HttpError(StatusCode, &'static str);
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(ApiError {
                code: self.1.into(),
                message: self.1.into(),
            }),
        )
            .into_response()
    }
}
type HttpResult<T> = std::result::Result<T, HttpError>;
fn bad() -> HttpError {
    HttpError(StatusCode::BAD_REQUEST, "invalid_request")
}
fn conflict() -> HttpError {
    HttpError(StatusCode::CONFLICT, "release_conflict")
}
fn unavailable() -> HttpError {
    HttpError(
        StatusCode::SERVICE_UNAVAILABLE,
        "storage_or_catalog_unavailable",
    )
}
fn mutation_error(error: anyhow::Error) -> HttpError {
    if let Some(sql) = error.downcast_ref::<sqlx::Error>() {
        if let sqlx::Error::Database(db) = sql
            && (db.is_unique_violation() || db.is_check_violation())
        {
            return conflict();
        }
        return unavailable();
    }
    conflict()
}
fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
fn scope<'a>(
    s: &'a AppState,
    app: &str,
    channel: &str,
    target: Option<&str>,
    h: &HeaderMap,
    admin: bool,
) -> HttpResult<&'a AppConfig> {
    let a = s
        .config
        .app(app)
        .ok_or(HttpError(StatusCode::NOT_FOUND, "unknown_application"))?;
    if !a.channels.contains_key(channel) || target.is_some_and(|t| !a.targets.contains(t)) {
        return Err(HttpError(
            StatusCode::NOT_FOUND,
            "unknown_channel_or_target",
        ));
    }
    let allowed = if admin {
        a.can_admin(channel, token(h))
    } else {
        a.can_read(channel, token(h))
    };
    if !allowed {
        return Err(HttpError(
            if token(h).is_some() {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::UNAUTHORIZED
            },
            "access_denied",
        ));
    }
    Ok(a)
}
async fn records(
    s: &AppState,
    a: &AppConfig,
    channel: &str,
    target: &str,
) -> HttpResult<Vec<ReleaseRecord>> {
    let records = s
        .repository
        .list(&a.app_id, channel, target)
        .await
        .map_err(|_| unavailable())?;
    for r in &records {
        let m = &r.bundle.manifest.body;
        if m.app_id != a.app_id || m.channel != channel || m.target != target {
            return Err(unavailable());
        }
        r.bundle
            .validate(&a.metadata_keys)
            .map_err(|_| unavailable())?;
        r.policy.validate().map_err(|_| unavailable())?;
    }
    Ok(records)
}
fn response(s: &AppState, r: &ReleaseRecord, selected: String) -> HttpResult<UpdateResponse> {
    let m = &r.bundle.manifest.body;
    let full = r
        .bundle
        .artifact(&m.full_artifact_id)
        .ok_or_else(unavailable)?;
    let prefix = format!(
        "/v1/apps/{}/channels/{}/releases/{}/targets/{}",
        m.app_id, m.channel, m.version, m.target
    );
    let mut urls = BTreeMap::new();
    for a in &r.bundle.artifacts {
        let path = format!("{prefix}/artifacts/{}/download", a.body.artifact_id);
        urls.insert(
            a.body.artifact_id.clone(),
            s.config
                .public_url
                .join(&path)
                .map_err(|_| unavailable())?
                .to_string(),
        );
    }
    Ok(UpdateResponse {
        schema_version: SCHEMA_VERSION,
        bundle: r.bundle.clone(),
        selected_artifact_id: selected,
        download_url: urls
            .get(&m.full_artifact_id)
            .ok_or_else(unavailable)?
            .clone(),
        download_urls: urls,
        status_url: s
            .config
            .public_url
            .join(&format!("{prefix}/status"))
            .map_err(|_| unavailable())?
            .to_string(),
        app_id: m.app_id.clone(),
        channel: m.channel.clone(),
        target: m.target.clone(),
        release_id: m.release_id.clone(),
        version: m.version.clone(),
        published_at: m.published_at,
        notes: m.notes.clone(),
        signature: full.package_signature.clone(),
        signing_key_id: r.bundle.manifest.key_id.clone(),
        sha256: full.sha256.clone(),
        size_bytes: full.size_bytes,
        minimum_supported_version: m.minimum_supported_version.clone(),
    })
}
async fn resolve_response(
    s: &AppState,
    h: &HeaderMap,
    request: CheckRequest,
) -> HttpResult<Response> {
    request.validate().map_err(|_| bad())?;
    let a = scope(
        s,
        &request.app_id,
        &request.channel,
        Some(&request.target),
        h,
        false,
    )?;
    let records = records(s, a, &request.channel, &request.target).await?;
    s.storage.ready(a).await.map_err(|_| unavailable())?;
    match resolve(&records, &request).map_err(|_| conflict())? {
        None => Ok(StatusCode::NO_CONTENT.into_response()),
        Some((r, selected)) => Ok(Json(response(s, r, selected)?).into_response()),
    }
}
async fn check(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(request): Json<CheckRequest>,
) -> HttpResult<Response> {
    resolve_response(&s, &h, request).await
}
async fn install_resolve(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(request): Json<InstallRequest>,
) -> HttpResult<Response> {
    request.validate().map_err(|_| bad())?;
    let a = scope(
        &s,
        &request.app_id,
        &request.channel,
        Some(&request.target),
        &h,
        false,
    )?;
    let catalog = records(&s, a, &request.channel, &request.target).await?;
    s.storage.ready(a).await.map_err(|_| unavailable())?;
    match resolve_install(&catalog, &request).map_err(|_| conflict())? {
        None => Ok(StatusCode::NO_CONTENT.into_response()),
        Some((r, selected)) => Ok(Json(response(&s, r, selected)?).into_response()),
    }
}
async fn install_authorize(
    State(s): State<AppState>,
    h: HeaderMap,
    Extension(signer): Extension<Option<Arc<AuthorizationSigner>>>,
    Json(request): Json<AuthorizationRequest>,
) -> HttpResult<Json<Signed<InstallAuthorization>>> {
    request.installation.validate().map_err(|_| bad())?;
    identifier(&request.operation_id).map_err(|_| bad())?;
    identifier(&request.release_id).map_err(|_| bad())?;
    hash(&request.nonce).map_err(|_| bad())?;
    let i = &request.installation;
    let a = scope(&s, &i.app_id, &i.channel, Some(&i.target), &h, false)?;
    let catalog = records(&s, a, &i.channel, &i.target).await?;
    // Resolve the exact requested release; newly published destinations must not
    // invalidate a still-published prepared package, but all checkpoint rules apply.
    let candidate = catalog
        .iter()
        .find(|r| r.bundle.manifest.body.release_id == request.release_id)
        .ok_or_else(conflict)?;
    let exact = [candidate.clone()];
    let (resolved, _) = resolve_install(&exact, i)
        .map_err(|_| conflict())?
        .ok_or_else(conflict)?;
    let m = &resolved.bundle.manifest.body;
    let full = resolved
        .bundle
        .artifact(&m.full_artifact_id)
        .ok_or_else(unavailable)?;
    let signer = signer.ok_or_else(unavailable)?;
    let permit = InstallAuthorization {
        schema_version: 1,
        app_id: m.app_id.clone(),
        channel: m.channel.clone(),
        target: m.target.clone(),
        release_id: m.release_id.clone(),
        version: m.version.clone(),
        sequence: m.sequence,
        package_sha256: full.sha256.clone(),
        installation_id: i.installation_id.clone(),
        operation_id: request.operation_id,
        intent: i.intent,
        nonce: request.nonce,
        valid_for_seconds: 300,
    };
    permit.validate().map_err(|_| bad())?;
    Ok(Json(
        sign(permit, signer.key_id.clone(), &signer.key).map_err(|_| unavailable())?,
    ))
}
#[derive(Deserialize)]
pub struct VersionQuery {
    current_version: Version,
}
fn get_request(
    app: String,
    channel: String,
    target: String,
    current_version: Version,
) -> CheckRequest {
    CheckRequest {
        app_id: app,
        channel,
        target,
        current_version,
        installation_id: None,
        supports_delta: false,
        base_sha256: None,
        base_size_bytes: None,
        schema_version: SCHEMA_VERSION,
    }
}
async fn common_get(
    State(s): State<AppState>,
    Path((a, c, t)): Path<(String, String, String)>,
    Query(q): Query<VersionQuery>,
    h: HeaderMap,
) -> HttpResult<Response> {
    resolve_response(&s, &h, get_request(a, c, t, q.current_version)).await
}
async fn tauri_get(
    State(s): State<AppState>,
    Path((a, c, t, v)): Path<(String, String, String, String)>,
    h: HeaderMap,
) -> HttpResult<Response> {
    let request = get_request(a, c, t, Version::parse(&v).map_err(|_| bad())?);
    let app = scope(
        &s,
        &request.app_id,
        &request.channel,
        Some(&request.target),
        &h,
        false,
    )?;
    let catalog = records(&s, app, &request.channel, &request.target).await?;
    s.storage.ready(app).await.map_err(|_| unavailable())?;
    match resolve(&catalog, &request).map_err(|_| conflict())? {
        None => Ok(StatusCode::NO_CONTENT.into_response()),
        Some((r, id)) => {
            let response = response(&s, r, id)?;
            Ok(Json(serde_json::json!({"version":response.version,"url":response.download_url,"signature":response.signature,"notes":response.notes,"pub_date":response.published_at})).into_response())
        }
    }
}
async fn find_release(
    s: &AppState,
    h: &HeaderMap,
    app: &str,
    channel: &str,
    target: &str,
    version: &str,
) -> HttpResult<ReleaseRecord> {
    let a = scope(s, app, channel, Some(target), h, false)?;
    let version = Version::parse(version).map_err(|_| bad())?;
    records(s, a, channel, target)
        .await?
        .into_iter()
        .find(|r| r.bundle.manifest.body.version == version)
        .ok_or(HttpError(StatusCode::NOT_FOUND, "unknown_release"))
}
async fn redirect(s: &AppState, r: ReleaseRecord, artifact: &str) -> HttpResult<Response> {
    if r.lifecycle != Lifecycle::Published || r.policy.paused {
        return Err(conflict());
    }
    let a = s
        .config
        .app(&r.bundle.manifest.body.app_id)
        .ok_or_else(unavailable)?;
    let descriptor = r
        .bundle
        .artifact(artifact)
        .ok_or(HttpError(StatusCode::NOT_FOUND, "unknown_artifact"))?;
    s.storage.ready(a).await.map_err(|_| unavailable())?;
    let url = s
        .storage
        .presign(a, descriptor)
        .await
        .map_err(|_| unavailable())?;
    let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
    response
        .headers_mut()
        .insert(header::LOCATION, url.parse().map_err(|_| unavailable())?);
    Ok(response)
}
async fn download(
    State(s): State<AppState>,
    Path((a, c, v, t, id)): Path<(String, String, String, String, String)>,
    h: HeaderMap,
) -> HttpResult<Response> {
    redirect(&s, find_release(&s, &h, &a, &c, &t, &v).await?, &id).await
}
async fn full_download(
    State(s): State<AppState>,
    Path((a, c, v, t)): Path<(String, String, String, String)>,
    h: HeaderMap,
) -> HttpResult<Response> {
    let r = find_release(&s, &h, &a, &c, &t, &v).await?;
    let id = r.bundle.manifest.body.full_artifact_id.clone();
    redirect(&s, r, &id).await
}
async fn status(
    State(s): State<AppState>,
    Path((a, c, v, t)): Path<(String, String, String, String)>,
    h: HeaderMap,
) -> HttpResult<Json<ReleaseStatus>> {
    let r = find_release(&s, &h, &a, &c, &t, &v).await?;
    Ok(Json(ReleaseStatus {
        release_id: r.bundle.manifest.body.release_id.clone(),
        install_allowed: r.lifecycle == Lifecycle::Published && !r.policy.paused,
        lifecycle: r.lifecycle,
    }))
}
async fn draft(
    State(s): State<AppState>,
    Path((a, c)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<DraftRequest>,
) -> HttpResult<Json<ReleaseRecord>> {
    let app = scope(
        &s,
        &a,
        &c,
        Some(&request.bundle.manifest.body.target),
        &h,
        true,
    )?;
    if request.bundle.manifest.body.app_id != a || request.bundle.manifest.body.channel != c {
        return Err(bad());
    }
    request
        .bundle
        .validate(&app.metadata_keys)
        .map_err(|_| bad())?;
    if request
        .bundle
        .artifacts
        .iter()
        .any(|a| a.body.size_bytes > s.config.max_artifact_bytes)
    {
        return Err(bad());
    }
    Ok(Json(
        s.repository
            .draft(request.bundle)
            .await
            .map_err(mutation_error)?,
    ))
}
#[derive(Deserialize)]
pub struct TargetQuery {
    target: String,
}
async fn admin_get(
    State(s): State<AppState>,
    Path((a, c, r)): Path<(String, String, String)>,
    Query(q): Query<TargetQuery>,
    h: HeaderMap,
) -> HttpResult<Json<ReleaseRecord>> {
    let app = scope(&s, &a, &c, Some(&q.target), &h, true)?;
    Ok(Json(
        records(&s, app, &c, &q.target)
            .await?
            .into_iter()
            .find(|x| x.bundle.manifest.body.release_id == r)
            .ok_or(HttpError(StatusCode::NOT_FOUND, "unknown_release"))?,
    ))
}
async fn action(
    State(s): State<AppState>,
    Path((a, c, r)): Path<(String, String, String)>,
    Query(q): Query<TargetQuery>,
    h: HeaderMap,
    Json(action): Json<AdminAction>,
) -> HttpResult<Json<ReleaseRecord>> {
    let app = scope(&s, &a, &c, Some(&q.target), &h, true)?;
    identifier(&action.operation_id).map_err(|_| bad())?;
    let catalog = records(&s, app, &c, &q.target).await?;
    let record = catalog
        .iter()
        .find(|x| x.bundle.manifest.body.release_id == r)
        .ok_or(HttpError(StatusCode::NOT_FOUND, "unknown_release"))?;
    if action.action == "publish" || action.action == "enable" {
        for checkpoint in &record.bundle.manifest.body.required_checkpoints {
            if !catalog.iter().any(|x| {
                x.bundle.manifest.body.release_id == checkpoint.release_id
                    && x.bundle.manifest.body.version == checkpoint.version
                    && x.lifecycle == Lifecycle::Published
            }) {
                return Err(conflict());
            }
        }
        for artifact in &record.bundle.artifacts {
            s.storage
                .verify(app, &artifact.body)
                .await
                .map_err(|_| unavailable())?;
        }
    }
    Ok(Json(
        s.repository
            .act(&a, &c, &r, &action)
            .await
            .map_err(mutation_error)?,
    ))
}
#[derive(Deserialize)]
pub struct PromoteRequest {
    pub source_channel: String,
    pub source_release_id: String,
    pub bundle: ReleaseBundle,
}
async fn promote(
    State(s): State<AppState>,
    Path((a, c)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<PromoteRequest>,
) -> HttpResult<Json<ReleaseRecord>> {
    let target = &request.bundle.manifest.body.target;
    let app = scope(&s, &a, &c, Some(target), &h, true)?;
    scope(&s, &a, &request.source_channel, Some(target), &h, true)?;
    request
        .bundle
        .validate(&app.metadata_keys)
        .map_err(|_| bad())?;
    let source = records(&s, app, &request.source_channel, target)
        .await?
        .into_iter()
        .find(|r| r.bundle.manifest.body.release_id == request.source_release_id)
        .ok_or(HttpError(StatusCode::NOT_FOUND, "unknown_source_release"))?;
    if source.lifecycle == Lifecycle::Revoked
        || source.lifecycle == Lifecycle::Draft
        || a != request.bundle.manifest.body.app_id
        || c != request.bundle.manifest.body.channel
        || source.bundle.artifacts != request.bundle.artifacts
        || source.bundle.manifest.body.version != request.bundle.manifest.body.version
    {
        return Err(conflict());
    }
    Ok(Json(
        s.repository
            .draft(request.bundle)
            .await
            .map_err(mutation_error)?,
    ))
}
async fn live(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"instance":s.instance}))
}
async fn ready(State(s): State<AppState>) -> HttpResult<Json<serde_json::Value>> {
    if s.draining.load(Ordering::Relaxed) {
        return Err(unavailable());
    }
    s.repository.ready().await.map_err(|_| unavailable())?;
    for app in &s.config.apps {
        s.storage.ready(app).await.map_err(|_| unavailable())?;
    }
    Ok(Json(
        serde_json::json!({"ready":true,"instance":s.instance,"version":env!("CARGO_PKG_VERSION")}),
    ))
}
async fn inflight(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"inflight":s.inflight.load(Ordering::Relaxed)}))
}
struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
async fn track(State(s): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let counter = if !path.starts_with("/health/") {
        s.inflight.fetch_add(1, Ordering::Relaxed);
        Some(Guard(s.inflight.clone()))
    } else {
        None
    };
    let method = request.method().clone();
    let started = std::time::Instant::now();
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    // Never log bearer tokens, query strings or presigned Location values.
    tracing::info!(%method,%path,status=response.status().as_u16(),elapsed_ms=started.elapsed().as_millis(),instance=%s.instance,"request");
    drop(counter);
    response
}
