//! Local named-pipe IPC. Package bytes cross the boundary; executable paths do not.
use super::native::*;
use crate::{PreparedUpdate, installation::*, inventory, progress::*};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    os::windows::io::AsRawHandle,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, ServerOptions},
};
use update_protocol::*;
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    System::{EventLog::*, Pipes::*, Services::*, Threading::*},
};

const FRAME_LIMIT: usize = 2 * 1024 * 1024;

fn allow_process_inspection() -> Result<()> {
    // The interactive client authenticates the LocalSystem pipe server by its
    // actual process image. Grant only that query right on this process, not
    // memory access, handle duplication, termination or access to service files.
    // Merge with the existing DACL so SYSTEM/admin/deny entries are preserved.
    unsafe {
        let process = GetCurrentProcess();
        let mut old_acl = null_mut();
        let mut descriptor = null_mut();
        let code = GetSecurityInfo(
            process,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut old_acl,
            null_mut(),
            &mut descriptor,
        );
        if code != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(code as i32))
                .context("service process DACL read failed");
        }
        let _descriptor = SecurityDescriptor(descriptor);
        ensure!(!old_acl.is_null(), "service process DACL missing");
        let mut sid = [0u32; SECURITY_MAX_SID_SIZE as usize / 4];
        let mut sid_size = std::mem::size_of_val(&sid) as u32;
        win(CreateWellKnownSid(
            WinInteractiveSid,
            null_mut(),
            sid.as_mut_ptr().cast(),
            &mut sid_size,
        ))?;
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: PROCESS_QUERY_LIMITED_INFORMATION,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                ptstrName: sid.as_mut_ptr().cast(),
            },
        };
        let mut acl = null_mut();
        let code = SetEntriesInAclW(1, &entry, old_acl, &mut acl);
        if code != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(code as i32))
                .context("service process query ACL merge failed");
        }
        let code = SetSecurityInfo(
            process,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        );
        LocalFree(acl.cast());
        if code != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(code as i32))
                .context("service process query ACL apply failed");
        }
        Ok(())
    }
}

fn report_ipc_error(source: &str, error: &anyhow::Error) {
    // Use the Windows Application log, not a writable sidecar. Never record IPC
    // requests, package bytes or authorization tokens.
    let message: String = format!("IPC request failed: {error:#}")
        .chars()
        .take(2048)
        .collect();
    unsafe {
        let handle = RegisterEventSourceW(null(), wide(source).as_ptr());
        if !handle.is_null() {
            let message = wide(message);
            let strings = [message.as_ptr()];
            ReportEventW(
                handle,
                EVENTLOG_ERROR_TYPE,
                0,
                1001,
                null_mut(),
                1,
                0,
                strings.as_ptr(),
                null(),
            );
            DeregisterEventSource(handle);
        }
    }
}

async fn wait_client_close<S: AsyncReadExt + Unpin>(stream: &mut S) {
    // DisconnectNamedPipe discards unread replies. Our client closes its handle
    // after reading the terminal reply; wait for that EOF, with a bounded wait
    // so an uncooperative client cannot retain a server slot indefinitely.
    let mut byte = [0u8];
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut byte)).await;
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Context,
    Health,
    Begin {
        operation_id: String,
        intent: InstallIntent,
        bundle: ReleaseBundle,
    },
    Commit {
        operation_id: String,
        authorization: Signed<InstallAuthorization>,
    },
    Query {
        operation_id: String,
    },
    Pending,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
enum Response {
    Health(bool),
    Context {
        installation_id: String,
        installed_version: Option<semver::Version>,
    },
    Begin(BeginResult),
    Event(ProgressEvent),
    Pending(Option<ProgressEvent>),
    ReadyForPackage,
    Error(String),
}
async fn write_frame<S: AsyncWriteExt + Unpin, T: Serialize>(
    stream: &mut S,
    value: &T,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= FRAME_LIMIT, "IPC frame too large");
    stream.write_u32_le(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}
async fn read_frame<S: AsyncReadExt + Unpin, T: DeserializeOwned>(stream: &mut S) -> Result<T> {
    let len = stream.read_u32_le().await? as usize;
    ensure!(len > 0 && len <= FRAME_LIMIT, "invalid IPC frame size");
    let mut bytes = vec![0; len];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn pipe_name(app_id: &str) -> Result<String> {
    identifier(app_id)?;
    Ok(format!("\\\\.\\pipe\\update-system-{app_id}-v1"))
}
fn process_path(pid: u32) -> Result<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("IPC OpenProcess failed (pid {pid})"));
        }
        let _process = Handle(process);
        let mut raw = vec![0u16; 32768];
        let mut len = raw.len() as u32;
        win(QueryFullProcessImageNameW(
            process,
            0,
            raw.as_mut_ptr(),
            &mut len,
        ))
        .context("IPC QueryFullProcessImageNameW failed")?;
        Ok(PathBuf::from(String::from_utf16(&raw[..len as usize])?))
    }
}
/// Native code owns this client. UI supplies operation IDs and consent only.
pub struct WindowsServiceClient {
    pub policy: WindowsPolicy,
}
impl WindowsServiceClient {
    pub async fn installed_is_healthy(&self) -> Result<bool> {
        match self.request(&Request::Health).await? {
            Response::Health(healthy) => Ok(healthy),
            _ => anyhow::bail!("unexpected health response"),
        }
    }
    async fn connect(&self) -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
        self.policy.validate()?;
        let pipe = ClientOptions::new()
            .open(pipe_name(&self.policy.app_id)?)
            .context("IPC pipe open failed")?;
        let mut pid = 0;
        unsafe {
            win(GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid))
                .context("IPC server PID lookup failed")?;
        }
        let expected_path = self.policy.service_executable()?;
        let publisher = self.policy.publisher_certificate_sha256.clone();
        // Authenticode is synchronous; never block the async readiness timeout.
        tokio::task::spawn_blocking(move || {
            let path = process_path(pid).context("IPC server process lookup failed")?;
            ensure!(path == expected_path, "unexpected named-pipe server binary");
            verify_authenticode(&path, &publisher)
                .context("IPC server Authenticode verification failed")
        })
        .await
        .context("IPC server verification task failed")??;
        Ok(pipe)
    }
    async fn request(&self, request: &Request) -> Result<Response> {
        let mut pipe = self.connect().await?;
        write_frame(&mut pipe, request)
            .await
            .context("IPC request write failed")?;
        let response: Response =
            tokio::time::timeout(std::time::Duration::from_secs(30), read_frame(&mut pipe))
                .await
                .context("IPC response timed out")?
                .context("IPC response read failed")?;
        match response {
            Response::Error(error) => anyhow::bail!(error),
            response => Ok(response),
        }
    }
    pub async fn installation_context(&self) -> Result<(Option<semver::Version>, String)> {
        match self.request(&Request::Context).await? {
            Response::Context {
                installed_version,
                installation_id,
            } => Ok((installed_version, installation_id)),
            _ => anyhow::bail!("unexpected context response"),
        }
    }
    pub async fn begin(
        &self,
        update: &PreparedUpdate,
        operation_id: &str,
        intent: InstallIntent,
    ) -> Result<BeginResult> {
        match self
            .request(&Request::Begin {
                operation_id: operation_id.into(),
                intent,
                bundle: update.bundle().clone(),
            })
            .await?
        {
            Response::Begin(result) => Ok(result),
            _ => anyhow::bail!("unexpected begin response"),
        }
    }
    pub async fn query(&self, operation_id: &str) -> Result<Option<ProgressEvent>> {
        match self
            .request(&Request::Query {
                operation_id: operation_id.into(),
            })
            .await?
        {
            Response::Pending(result) => Ok(result),
            _ => anyhow::bail!("unexpected query response"),
        }
    }
    pub async fn pending(&self) -> Result<Option<ProgressEvent>> {
        match self.request(&Request::Pending).await? {
            Response::Pending(result) => Ok(result),
            _ => anyhow::bail!("unexpected pending response"),
        }
    }
    pub async fn commit(
        &self,
        update: &PreparedUpdate,
        operation_id: &str,
        authorization: Signed<InstallAuthorization>,
        callback: ProgressCallback<'_>,
    ) -> Result<ProgressEvent> {
        let mut pipe = self.connect().await?;
        write_frame(
            &mut pipe,
            &Request::Commit {
                operation_id: operation_id.into(),
                authorization,
            },
        )
        .await?;
        match read_frame::<_, Response>(&mut pipe).await? {
            Response::ReadyForPackage => (),
            Response::Event(event) => return Ok(event),
            Response::Error(error) => anyhow::bail!(error),
            _ => anyhow::bail!("unexpected package response"),
        }
        let mut file = tokio::fs::File::open(update.package_path()).await?;
        pipe.write_u64_le(file.metadata().await?.len()).await?;
        tokio::io::copy(&mut file, &mut pipe).await?;
        pipe.flush().await?;
        loop {
            match read_frame::<_, Response>(&mut pipe).await? {
                Response::Event(event) => {
                    callback(event.clone());
                    if matches!(event.stage, Stage::Completed | Stage::NoOp | Stage::Failed) {
                        return Ok(event);
                    }
                }
                Response::Error(error) => anyhow::bail!(error),
                _ => anyhow::bail!("unexpected installation response"),
            }
        }
    }
}

async fn handle_request(
    pipe: &mut tokio::net::windows::named_pipe::NamedPipeServer,
    engine: &std::sync::Arc<tokio::sync::Mutex<InstallEngine>>,
    platform: &WindowsInstaller,
    state_dir: &std::path::Path,
) -> Result<()> {
    let request: Request =
        tokio::time::timeout(std::time::Duration::from_secs(15), read_frame(pipe))
            .await
            .context("IPC client request timed out")?
            .context("IPC client request read failed")?;
    let mut pid = 0;
    let mut session = 0;
    unsafe {
        win(GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid))?;
        win(GetNamedPipeClientSessionId(
            pipe.as_raw_handle(),
            &mut session,
        ))?;
    }
    ensure!(session != 0, "interactive client required");
    let path = process_path(pid).context("IPC client process lookup failed")?;
    verify_authenticode(&path, &platform.policy.publisher_certificate_sha256)
        .context("IPC client Authenticode verification failed")?;
    // Read-only queries remain available while a different pipe installs/uploads.
    let query_path = match &request {
        Request::Query { operation_id } => {
            identifier(operation_id)?;
            Some(
                state_dir
                    .join("operations")
                    .join(format!("{operation_id}.json")),
            )
        }
        Request::Pending => Some(state_dir.join("install.json")),
        _ => None,
    };
    if let Some(path) = query_path {
        let event = if path.exists() {
            crate::InstallJournal::load(&path)
                .await?
                .operation
                .map(|o| o.progress)
        } else {
            None
        };
        return write_frame(pipe, &Response::Pending(event)).await;
    }
    let mut engine = engine.lock().await;
    // A successful uninstall retires the machine's rollout identity. Reload on
    // the next mutating/context request and discard challenges for the old ID.
    let context = bootstrap_context(&platform.policy)?;
    if context.installation_id != engine.context().installation_id {
        *engine = InstallEngine::new(
            context,
            platform.policy.metadata_keys.clone(),
            platform.policy.authorization_keys.clone(),
            platform.policy.package_public_key.clone(),
        )?;
        engine.reconcile(platform).await?;
    }
    match request {
        Request::Health => {
            let healthy = engine.installed_is_healthy(platform).await?;
            write_frame(pipe, &Response::Health(healthy)).await
        }
        Request::Context => {
            let installed_version = engine.installed_version(platform).await?;
            write_frame(
                pipe,
                &Response::Context {
                    installed_version,
                    installation_id: engine.context().installation_id.clone(),
                },
            )
            .await
        }
        Request::Begin {
            operation_id,
            intent,
            bundle,
        } => {
            let result = engine
                .begin(&operation_id, intent, bundle, platform)
                .await?;
            write_frame(pipe, &Response::Begin(result)).await
        }
        Request::Query { operation_id } => {
            write_frame(pipe, &Response::Pending(engine.query(&operation_id).await?)).await
        }
        Request::Pending => {
            write_frame(
                pipe,
                &Response::Pending(
                    engine
                        .pending_operation()
                        .await?
                        .and_then(|j| j.operation.map(|o| o.progress)),
                ),
            )
            .await
        }
        Request::Commit {
            operation_id,
            authorization,
        } => {
            identifier(&operation_id)?;
            if let Some(event) = engine.query(&operation_id).await?
                && matches!(event.stage, Stage::Completed | Stage::NoOp)
            {
                return write_frame(pipe, &Response::Event(event)).await;
            }
            engine
                .validate_authorization(&operation_id, &authorization)
                .await?;
            let (destination, size) = engine.package_destination(&operation_id).await?;
            let dir = destination.parent().context("package parent")?;
            // Keep a crash-recovery backup until the new installation verifies.
            if dir.exists() {
                verify_protected(dir)?;
                inventory::build_inventory(dir)?;
                let payload = dir.join("payload");
                if payload.exists() {
                    std::fs::remove_dir_all(payload)?;
                }
                if destination.exists() {
                    std::fs::remove_file(&destination)?;
                }
            }
            secure_directory(dir)?;
            let _pins = pin_path(dir)?;
            write_frame(pipe, &Response::ReadyForPackage).await?;
            let copy = async {
                ensure!(pipe.read_u64_le().await? == size, "unexpected package size");
                let mut file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                    .await?;
                let copied = tokio::io::copy(&mut (&mut *pipe).take(size), &mut file).await?;
                ensure!(copied == size, "truncated package upload");
                file.sync_all().await?;
                Ok::<_, anyhow::Error>(())
            };
            tokio::time::timeout(std::time::Duration::from_secs(240), copy).await??;
            verify_protected(&destination)?;
            // The protected staging root remains pinned for the service lifetime.
            // Release this operation-directory pin so verified-success cleanup can remove it.
            drop(_pins);
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let callback = |event| {
                let _ = tx.send(event);
            };
            let install = engine.commit(&operation_id, authorization, platform, &callback);
            tokio::pin!(install);
            loop {
                tokio::select! {
                    result = &mut install => {
                        while let Ok(event) = rx.try_recv() { let _ = write_frame(pipe, &Response::Event(event)).await; }
                        let event = result?;
                        return write_frame(pipe, &Response::Event(event)).await;
                    },
                    Some(event) = rx.recv() => { let _ = write_frame(pipe, &Response::Event(event)).await; },
                }
            }
        }
    }
}

static POLICY: OnceLock<WindowsPolicy> = OnceLock::new();
static STOP: AtomicBool = AtomicBool::new(false);
static CHECKPOINT: AtomicU32 = AtomicU32::new(0);
static STATUS: OnceLock<usize> = OnceLock::new();
fn report(state: u32, error: u32) {
    if let Some(handle) = STATUS.get() {
        unsafe {
            let status = SERVICE_STATUS {
                dwServiceType: SERVICE_WIN32_OWN_PROCESS,
                dwCurrentState: state,
                dwControlsAccepted: if state == SERVICE_RUNNING {
                    SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
                } else {
                    0
                },
                dwWin32ExitCode: error,
                dwServiceSpecificExitCode: 0,
                dwCheckPoint: if matches!(state, SERVICE_START_PENDING | SERVICE_STOP_PENDING) {
                    CHECKPOINT.fetch_add(1, Ordering::Relaxed) + 1
                } else {
                    0
                },
                dwWaitHint: if state == SERVICE_STOP_PENDING {
                    300000
                } else {
                    0
                },
            };
            SetServiceStatus(*handle as SERVICE_STATUS_HANDLE, &status);
        }
    }
}
unsafe extern "system" fn control(
    control: u32,
    _: u32,
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
) -> u32 {
    if control == SERVICE_CONTROL_STOP || control == SERVICE_CONTROL_SHUTDOWN {
        STOP.store(true, Ordering::SeqCst);
        report(SERVICE_STOP_PENDING, 0);
    }
    0
}
unsafe extern "system" fn service_main(_: u32, _: *mut *mut u16) {
    let policy = POLICY.get().expect("policy").clone();
    let event_source = policy.service_name.clone();
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(wide(&policy.service_name).as_ptr(), Some(control), null())
    };
    if handle.is_null() {
        return;
    }
    let _ = STATUS.set(handle as usize);
    report(SERVICE_START_PENDING, 0);
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|rt| rt.block_on(serve(policy)));
    if let Err(error) = &result {
        report_ipc_error(&event_source, error);
    }
    report(
        SERVICE_STOPPED,
        if result.is_ok() {
            0
        } else {
            ERROR_SERVICE_SPECIFIC_ERROR
        },
    );
}
async fn serve(policy: WindowsPolicy) -> Result<()> {
    allow_process_inspection()?;
    let context = bootstrap_context(&policy)?;
    let _stage_pins = pin_path(&context.staging_root)?;
    let _state_pins = pin_path(&context.state_dir)?;
    let _active_parent_pins = pin_path(context.active_dir.parent().context("active parent")?)?;
    let engine = InstallEngine::new(
        context,
        policy.metadata_keys.clone(),
        policy.authorization_keys.clone(),
        policy.package_public_key.clone(),
    )?;
    let platform = WindowsInstaller {
        policy: policy.clone(),
    };
    engine.reconcile(&platform).await?;
    let security = SecurityDescriptor::new(
        "D:P(D;;GA;;;AN)(D;;GA;;;NU)(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;IU)",
    )?;
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .max_instances(16);
    let pipe = unsafe {
        options.create_with_security_attributes_raw(
            pipe_name(&policy.app_id)?,
            (&security.attributes() as *const _ as *mut std::ffi::c_void).cast(),
        )?
    };
    report(SERVICE_RUNNING, 0);
    let state_dir = engine.context().state_dir.clone();
    let engine = std::sync::Arc::new(tokio::sync::Mutex::new(engine));
    let platform = std::sync::Arc::new(platform);
    let mut tasks = tokio::task::JoinSet::new();
    // Create the next listener before releasing any connected instance: the name
    // is never unowned, and FIRST_PIPE_INSTANCE prevents startup pipe squatting.
    let mut pipe = pipe;
    while !STOP.load(Ordering::SeqCst) {
        match tokio::time::timeout(std::time::Duration::from_secs(1), pipe.connect()).await {
            Err(_) => continue,
            Ok(result) => result?,
        }
        while tasks.len() >= 15 {
            let _ = tasks.join_next().await;
        }
        options.first_pipe_instance(false);
        let next = unsafe {
            options.create_with_security_attributes_raw(
                pipe_name(&policy.app_id)?,
                (&security.attributes() as *const _ as *mut std::ffi::c_void).cast(),
            )?
        };
        let mut connected = std::mem::replace(&mut pipe, next);
        let engine = engine.clone();
        let platform = platform.clone();
        let state_dir = state_dir.clone();
        tasks.spawn(async move {
            if let Err(error) = handle_request(&mut connected, &engine, &platform, &state_dir).await
            {
                report_ipc_error(&platform.policy.service_name, &error);
                let diagnostic: String = format!("{error:#}").chars().take(2048).collect();
                let _ = write_frame(&mut connected, &Response::Error(diagnostic)).await;
            }
            wait_client_close(&mut connected).await;
            let _ = connected.disconnect();
        });
        while tasks.try_join_next().is_some() {}
    }
    while !tasks.is_empty() {
        tokio::select! { _ = tasks.join_next() => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => report(SERVICE_STOP_PENDING, 0) }
    }
    Ok(())
}
/// Policy is loaded from the protected service directory, not a user-selected path.
pub fn run_registered_service() -> Result<()> {
    let exe = std::env::current_exe()?;
    let file = exe
        .parent()
        .context("service directory")?
        .join("policy.json");
    verify_protected(&file)?;
    let signed: Signed<WindowsPolicy> = serde_json::from_slice(&std::fs::read(file)?)?;
    let policy = verified_policy(&signed)?;
    ensure!(
        exe == policy.service_executable()?,
        "service launched from unexpected location"
    );
    verify_authenticode(&exe, &policy.publisher_certificate_sha256)?;
    POLICY
        .set(policy.clone())
        .map_err(|_| anyhow::anyhow!("service already initialized"))?;
    let mut name = wide(&policy.service_name);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    unsafe { win(StartServiceCtrlDispatcherW(table.as_ptr())) }
}
/// Bootstrap requires administrator rights. Subsequent installs use the offline service.
pub fn register_service(signed: Signed<WindowsPolicy>) -> Result<()> {
    let policy = verified_policy(&signed)?;
    let source = std::env::current_exe()?;
    let _source_pins = pin_path(source.parent().context("source directory")?)?;
    let _source_guard = inventory::package_guard(&source)?;
    verify_authenticode(&source, &policy.publisher_certificate_sha256)?;
    let destination = policy.service_executable()?;
    let product_root = policy.product_root()?;
    let vendor_root = product_root.parent().context("vendor directory")?;
    let _program_files_pin = pin_path(vendor_root.parent().context("Program Files")?)?;
    secure_public_directory(vendor_root)?;
    secure_public_directory(&product_root)?;
    let _destination_parent_pins = pin_path(&product_root)?;
    secure_public_directory(destination.parent().context("service directory")?)?;
    let _destination_pins = pin_path(destination.parent().context("service directory")?)?;
    if destination.exists() {
        ensure!(
            inventory::file_digest(&destination)? == inventory::file_digest(&source)?,
            "service binary differs; an explicit service maintenance deployment is required"
        );
    } else {
        std::fs::copy(&source, &destination)?;
        set_new_file_owner(&destination)?;
    }
    verify_protected(&destination)?;
    verify_authenticode(&destination, &policy.publisher_certificate_sha256)?;
    let config = destination
        .parent()
        .context("directory")?
        .join("policy.json");
    if config.exists() {
        verify_protected(&config)?;
        ensure!(
            std::fs::read(&config)? == serde_json::to_vec_pretty(&signed)?,
            "existing service policy differs"
        );
    } else {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&config)?;
        file.write_all(&serde_json::to_vec_pretty(&signed)?)?;
        file.sync_all()?;
        drop(file);
        set_new_file_owner(&config)?;
    }
    bootstrap_context(&policy)?;
    unsafe {
        let scm = OpenSCManagerW(
            null(),
            null(),
            SC_MANAGER_CREATE_SERVICE | SC_MANAGER_CONNECT,
        );
        ensure!(
            !scm.is_null(),
            "administrator permission required to register service"
        );
        let binary = wide(format!("\"{}\" run", destination.display()));
        let mut service = CreateServiceW(
            scm,
            wide(&policy.service_name).as_ptr(),
            wide(&policy.display_name).as_ptr(),
            SERVICE_START | SERVICE_QUERY_STATUS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            binary.as_ptr(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
        ); // LocalSystem, narrow verified IPC only
        if service.is_null() && GetLastError() == ERROR_SERVICE_EXISTS {
            service = OpenServiceW(
                scm,
                wide(&policy.service_name).as_ptr(),
                SERVICE_START | SERVICE_QUERY_STATUS,
            );
        }
        if service.is_null() {
            CloseServiceHandle(scm);
            anyhow::bail!("service registration failed");
        }
        let start = StartServiceW(service, 0, null());
        let error = GetLastError();
        CloseServiceHandle(service);
        CloseServiceHandle(scm);
        ensure!(
            start != 0 || error == ERROR_SERVICE_ALREADY_RUNNING,
            "service start failed: {error}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_process_query_does_not_grant_process_control() {
        allow_process_inspection().unwrap();
        // A restricted token must pass a second access check against just IU.
        // This proves that existing owner/admin rights do not mask a broad grant.
        unsafe {
            let mut token = null_mut();
            win(OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &mut token,
            ))
            .unwrap();
            let token = Handle(token);
            let mut sid = [0u32; SECURITY_MAX_SID_SIZE as usize / 4];
            let mut size = std::mem::size_of_val(&sid) as u32;
            win(CreateWellKnownSid(
                WinInteractiveSid,
                null_mut(),
                sid.as_mut_ptr().cast(),
                &mut size,
            ))
            .unwrap();
            let restriction = SID_AND_ATTRIBUTES {
                Sid: sid.as_mut_ptr().cast(),
                Attributes: 0,
            };
            let mut restricted = null_mut();
            win(CreateRestrictedToken(
                token.0,
                DISABLE_MAX_PRIVILEGE,
                0,
                null(),
                0,
                null(),
                1,
                &restriction,
                &mut restricted,
            ))
            .unwrap();
            let restricted = Handle(restricted);
            win(ImpersonateLoggedOnUser(restricted.0)).unwrap();
            struct Revert;
            impl Drop for Revert {
                fn drop(&mut self) {
                    unsafe {
                        RevertToSelf();
                    }
                }
            }
            let _revert = Revert;
            let query = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, GetCurrentProcessId());
            assert!(
                !query.is_null(),
                "limited query must be available: {}",
                std::io::Error::last_os_error()
            );
            let _query = Handle(query);
            for right in [
                PROCESS_VM_READ,
                PROCESS_VM_WRITE,
                PROCESS_CREATE_THREAD,
                PROCESS_DUP_HANDLE,
                PROCESS_TERMINATE,
                windows_sys::Win32::Storage::FileSystem::WRITE_DAC,
                PROCESS_ALL_ACCESS,
            ] {
                let handle = OpenProcess(right, 0, GetCurrentProcessId());
                let code = GetLastError();
                if !handle.is_null() {
                    CloseHandle(handle);
                }
                assert!(
                    handle.is_null(),
                    "unexpected process control right: {right:#x}"
                );
                assert_eq!(code, ERROR_ACCESS_DENIED);
            }
        }
    }

    #[test]
    fn unavailable_process_preserves_windows_error() {
        let error = process_path(0).unwrap_err();
        assert!(format!("{error:#}").contains("IPC OpenProcess failed"));
        assert!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error()
                .is_some()
        );
    }

    #[tokio::test]
    async fn terminal_replies_survive_delayed_client_reads() {
        for reply in [
            Response::Pending(None),
            Response::Error("client verification failed".into()),
        ] {
            let name = format!("\\\\.\\pipe\\baykuslig-ipc-test-{}", uuid::Uuid::new_v4());
            let mut server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)
                .unwrap();
            let (written_tx, written_rx) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(async move {
                server.connect().await.unwrap();
                write_frame(&mut server, &reply).await.unwrap();
                written_tx.send(()).unwrap();
                wait_client_close(&mut server).await;
                server.disconnect().unwrap();
            });
            let mut client = ClientOptions::new().open(&name).unwrap();
            written_rx.await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            assert!(
                !task.is_finished(),
                "server disconnected before the reply was consumed"
            );
            let reply: Response = read_frame(&mut client).await.unwrap();
            assert!(matches!(
                reply,
                Response::Pending(None) | Response::Error(_)
            ));
            drop(client);
            tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn unresponsive_client_cannot_hold_close_wait_forever() {
        let (mut server, _client) = tokio::io::duplex(64);
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            wait_client_close(&mut server),
        )
        .await
        .unwrap();
    }
}
