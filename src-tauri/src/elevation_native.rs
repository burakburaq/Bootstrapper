//! The WebView stays unelevated. Only the fixed, UI-less helper requests UAC.
//! The UAC worker registers only the fixed embedded signed update service.
use std::{ffi::OsStr, os::windows::ffi::OsStrExt, sync::Mutex};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        Threading::{
            GetCurrentProcess, INFINITE, OpenProcess, OpenProcessToken, PROCESS_SYNCHRONIZE,
            WaitForSingleObject,
        },
    },
    UI::{
        Shell::{
            SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
            ShellExecuteExW,
        },
        WindowsAndMessaging::SW_HIDE,
    },
};

// Store the handle as an integer so the mutex can safely move between threads.
// No handle is exposed to the WebView; Drop closes each owned handle once.
struct OwnedHandle(isize);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Result<Self, String> {
        if handle.is_null() {
            Err("elevation_handle_failed".into())
        } else {
            Ok(Self(handle as isize))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }

    fn running(&self) -> bool {
        // SAFETY: This object owns a valid process handle for the entire call.
        unsafe { WaitForSingleObject(self.raw(), 0) == WAIT_TIMEOUT }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: The non-null handle is uniquely owned and closed exactly once.
        unsafe { CloseHandle(self.raw()) };
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, String> {
        // SAFETY: No reserved pointer is supplied. This blocking task initializes
        // COM on its own thread and balances successful initialization in Drop.
        let result = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if result < 0 {
            Err("elevation_com_failed".into())
        } else {
            Ok(Self)
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: Initialization succeeded on this same thread.
        unsafe { CoUninitialize() };
    }
}

fn current_process_elevated() -> Result<bool, String> {
    let mut token = std::ptr::null_mut();
    // SAFETY: The pseudo process handle is valid; token points to writable storage.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("elevation_token_failed".into());
    }
    let token = OwnedHandle::new(token)?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut length = 0;
    // SAFETY: The owned token is valid and the output buffer has the required size.
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
    } == 0
    {
        return Err("elevation_token_failed".into());
    }
    Ok(elevation.TokenIsElevated != 0)
}

#[derive(Default)]
pub struct ElevationSession {
    worker: Mutex<Option<OwnedHandle>>,
}

impl ElevationSession {
    pub fn request(&self, owner: isize) -> Result<bool, String> {
        let mut worker = self.worker.lock().map_err(|_| "elevation_state_failed")?;
        if worker.as_ref().is_some_and(OwnedHandle::running) || current_process_elevated()? {
            return Ok(true);
        }
        *worker = None;
        let _com = ComApartment::initialize()?;
        let executable = std::env::current_exe().map_err(|_| "elevation_path_failed")?;
        let executable: Vec<u16> = executable
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let verb: Vec<u16> = OsStr::new("runas").encode_wide().chain(Some(0)).collect();
        let parameters = format!("--elevation-worker {}", std::process::id());
        let parameters: Vec<u16> = OsStr::new(&parameters)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut launch = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            hwnd: owner as _,
            lpVerb: verb.as_ptr(),
            lpFile: executable.as_ptr(),
            lpParameters: parameters.as_ptr(),
            nShow: SW_HIDE,
            ..Default::default()
        };
        // SAFETY: All UTF-16 strings are NUL-terminated and live through the call.
        // The target, verb and arguments are native-generated, never frontend input.
        if unsafe { ShellExecuteExW(&mut launch) } == 0 {
            // SAFETY: Read this thread's error immediately after the failed call.
            return Err(if unsafe { GetLastError() } == ERROR_CANCELLED {
                "elevation_cancelled"
            } else {
                "elevation_launch_failed"
            }
            .into());
        }
        let child = OwnedHandle::new(launch.hProcess)?;
        // SAFETY: ShellExecuteEx returned an owned process handle. Give immediate
        // startup failures time to exit; later exits are caught by approved().
        if unsafe { WaitForSingleObject(child.raw(), 250) } != WAIT_TIMEOUT {
            return Err("elevation_worker_failed".into());
        }
        // Successful runas guarantees the child was started with elevation.
        *worker = Some(child);
        Ok(true)
    }
}

pub fn run_worker(parent_pid: u32) -> Result<(), String> {
    if parent_pid == 0 || parent_pid == std::process::id() || !current_process_elevated()? {
        return Err("elevation_worker_invalid".into());
    }
    // SAFETY: PID is parsed as u32; only synchronization access is requested.
    let parent = OwnedHandle::new(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent_pid) })?;
    crate::service_bootstrap::register()?;
    // The worker owns no WebView or online download client.
    // SAFETY: The parent handle stays owned for the entire wait. The helper exits
    // when the UI process closes, including a crash or close during UAC.
    if unsafe { WaitForSingleObject(parent.raw(), INFINITE) } != WAIT_OBJECT_0 {
        return Err("elevation_parent_wait_failed".into());
    }
    Ok(())
}
