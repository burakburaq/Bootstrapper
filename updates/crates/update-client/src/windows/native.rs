use crate::{installation::*, inventory};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs::File,
    io::{Read, Seek, SeekFrom},
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use update_protocol::*;
use uuid::Uuid;
use windows_sys::{
    Win32::{
        Foundation::*,
        Security::{Authorization::*, WinTrust::*, *},
        Storage::FileSystem::*,
        System::{
            Com::CoTaskMemFree, Diagnostics::ToolHelp::*, JobObjects::*, Registry::*, Services::*,
            SystemInformation::*, Threading::*,
        },
        UI::Shell::*,
    },
    core::GUID,
};

fn volume_name(path: &Path) -> Result<Vec<u16>> {
    let mut mount = vec![0u16; 32768];
    let mut name = vec![0u16; 32768];
    unsafe {
        win(GetVolumePathNameW(
            wide(path).as_ptr(),
            mount.as_mut_ptr(),
            mount.len() as u32,
        ))?;
        win(GetVolumeNameForVolumeMountPointW(
            mount.as_ptr(),
            name.as_mut_ptr(),
            name.len() as u32,
        ))?;
    }
    name.truncate(
        name.iter()
            .position(|v| *v == 0)
            .context("volume name missing terminator")?,
    );
    Ok(name)
}

pub(crate) fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
pub(crate) fn win(ok: i32) -> Result<()> {
    ensure!(
        ok != 0,
        "Windows error: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}
pub(crate) struct Handle(pub HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsPolicy {
    pub app_id: String,
    pub channel: String,
    pub target: String,
    pub product_directory: String,
    pub executable: String,
    pub service_name: String,
    pub display_name: String,
    pub publisher_certificate_sha256: String,
    pub metadata_keys: KeyRing,
    pub authorization_keys: KeyRing,
    pub package_public_key: String,
}
impl Signable for WindowsPolicy {
    const DOMAIN: &'static str = "windows-service-policy";
}
pub(crate) fn verified_policy(signed: &Signed<WindowsPolicy>) -> Result<WindowsPolicy> {
    // Compile-time public roots are protected by the helper's Authenticode signature.
    // A writable sidecar must never be allowed to supply its own verification keys.
    let roots = option_env!("UPDATE_SERVICE_BOOTSTRAP_KEYS")
        .context("helper was built without UPDATE_SERVICE_BOOTSTRAP_KEYS; rebuild with release public roots before signing")?;
    let keys: KeyRing = serde_json::from_str(roots)?;
    verify(signed, &keys)?;
    signed.body.validate()?;
    Ok(signed.body.clone())
}
impl WindowsPolicy {
    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.app_id,
            &self.channel,
            &self.target,
            &self.product_directory,
            &self.service_name,
        ] {
            identifier(id)?;
        }
        relative_payload_path(&self.executable)?;
        relative_payload_path(&self.product_directory)?;
        validate_trust(&self.metadata_keys, &self.package_public_key)?;
        validate_trust(&self.authorization_keys, &self.package_public_key)?;
        ensure!(
            self.authorization_keys
                .values()
                .all(|key| !self.metadata_keys.values().any(|v| v == key)),
            "separate install authorization keys required"
        );
        ensure!(
            self.executable.to_ascii_lowercase().ends_with(".exe")
                && !self.display_name.contains('\0')
                && self.display_name.len() <= 128,
            "invalid product/service display identity"
        );
        ensure!(
            !self.product_directory.contains('.') && !self.executable.contains('/'),
            "fixed product directory/main EXE required"
        );
        hash(&self.publisher_certificate_sha256)?;
        ensure!(
            matches!(
                self.target.as_str(),
                "windows-x86_64"
                    | "windows-aarch64"
                    | "windows-x86_64-nsis"
                    | "windows-aarch64-nsis"
            ),
            "unsupported Windows target"
        );
        Ok(())
    }
    pub fn product_root(&self) -> Result<PathBuf> {
        Ok(known(&FOLDERID_ProgramFiles)?
            .join("TalayLabs")
            .join(&self.product_directory))
    }
    pub fn application_directory(&self) -> Result<PathBuf> {
        self.product_root()
    }
    pub fn service_directory(&self) -> Result<PathBuf> {
        Ok(self.product_root()?.join("Service"))
    }
    pub fn service_executable(&self) -> Result<PathBuf> {
        Ok(self.service_directory()?.join("update-service.exe"))
    }
}
fn known(id: &GUID) -> Result<PathBuf> {
    unsafe {
        let mut raw = null_mut();
        ensure!(
            SHGetKnownFolderPath(id, 0, null_mut(), &mut raw) >= 0,
            "known-folder resolution failed"
        );
        let mut n = 0;
        while *raw.add(n) != 0 {
            n += 1;
        }
        use std::os::windows::ffi::OsStringExt;
        let result = PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(
            raw, n,
        )));
        CoTaskMemFree(raw.cast());
        ensure!(
            result.is_absolute() && !result.as_os_str().to_string_lossy().starts_with("\\\\"),
            "local known folder required"
        );
        Ok(result)
    }
}
pub(crate) struct SecurityDescriptor(pub PSECURITY_DESCRIPTOR);
impl SecurityDescriptor {
    pub(crate) fn new(sddl: &str) -> Result<Self> {
        unsafe {
            let mut ptr = null_mut();
            win(ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(sddl).as_ptr(),
                1,
                &mut ptr,
                null_mut(),
            ))?;
            Ok(Self(ptr))
        }
    }
    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}
impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
/// Reject pre-existing attacker-owned directories rather than changing their ACL:
/// an attacker might retain an already-open write handle after an ACL change.
pub(crate) fn verify_protected(path: &Path) -> Result<()> {
    inventory::regular_metadata(path, path.is_dir())?;
    unsafe {
        let mut owner = null_mut();
        let mut acl = null_mut();
        let mut sd = null_mut();
        ensure!(
            GetNamedSecurityInfoW(
                wide(path).as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut acl,
                null_mut(),
                &mut sd
            ) == 0,
            "cannot read protected ACL"
        );
        let _sd = SecurityDescriptor(sd);
        ensure!(
            !owner.is_null()
                && (IsWellKnownSid(owner, WinLocalSystemSid) != 0
                    || IsWellKnownSid(owner, WinBuiltinAdministratorsSid) != 0),
            "protected path has untrusted owner"
        );
        ensure!(!acl.is_null(), "NULL DACL rejected");
        let mut info: ACL_SIZE_INFORMATION = zeroed();
        win(GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        ))?;
        ensure!(info.AceCount > 0, "empty protected ACL");
        for i in 0..info.AceCount {
            let mut ace = null_mut();
            win(GetAce(acl, i, &mut ace))?;
            let header = &*(ace as *const ACE_HEADER);
            if header.AceType == 0 {
                // ACCESS_ALLOWED_ACE_TYPE
                let allowed = &*(ace as *const ACCESS_ALLOWED_ACE);
                let sid = (&allowed.SidStart as *const u32).cast_mut().cast();
                let trusted = IsWellKnownSid(sid, WinLocalSystemSid) != 0
                    || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0;
                let mutation = GENERIC_ALL
                    | GENERIC_WRITE
                    | WRITE_DAC
                    | WRITE_OWNER
                    | DELETE
                    | FILE_WRITE_DATA
                    | FILE_APPEND_DATA
                    | FILE_WRITE_EA
                    | FILE_WRITE_ATTRIBUTES
                    | FILE_DELETE_CHILD;
                ensure!(
                    trusted || allowed.Mask & mutation == 0,
                    "protected path grants mutation to an untrusted SID"
                );
            } else {
                ensure!(header.AceType == 1, "unsupported protected ACL entry");
            }
        }
    }
    Ok(())
}
pub(crate) fn secure_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        let sd = SecurityDescriptor::new("O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")?;
        unsafe {
            win(CreateDirectoryW(wide(path).as_ptr(), &sd.attributes()))?;
        }
    }
    verify_protected(path)
}
pub(crate) fn secure_public_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        let sd = SecurityDescriptor::new(
            "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)",
        )?;
        unsafe {
            win(CreateDirectoryW(wide(path).as_ptr(), &sd.attributes()))?;
        }
    }
    verify_protected(path)?;
    // Public release metadata is readable by applications; mutation stays privileged.
    // Also upgrade trusted existing directories created by an older helper.
    unsafe {
        let sd =
            SecurityDescriptor::new("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)")?;
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = null_mut();
        win(GetSecurityDescriptorDacl(
            sd.0,
            &mut present,
            &mut acl,
            &mut defaulted,
        ))?;
        ensure!(
            present != 0 && !acl.is_null(),
            "public directory ACL unavailable"
        );
        ensure!(
            SetNamedSecurityInfoW(
                wide(path).as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null()
            ) == 0,
            "cannot set public read-only ACL"
        );
    }
    verify_protected(path)
}
pub(crate) fn set_new_file_owner(path: &Path) -> Result<()> {
    unsafe {
        let sd = SecurityDescriptor::new("O:BAG:BA")?;
        let mut owner = null_mut();
        let mut defaulted = 0;
        win(GetSecurityDescriptorOwner(sd.0, &mut owner, &mut defaulted))?;
        ensure!(
            SetNamedSecurityInfoW(
                wide(path).as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                owner,
                null_mut(),
                null(),
                null()
            ) == 0,
            "cannot set protected file owner"
        );
    }
    verify_protected(path)
}
/// Pin ancestors against rename/delete and refuse all reparse components.
pub(crate) fn pin_path(path: &Path) -> Result<Vec<File>> {
    let mut pins = Vec::new();
    for part in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        inventory::regular_metadata(part, true)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(part)?;
        use std::os::windows::fs::MetadataExt;
        ensure!(
            file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "reparse ancestor rejected"
        );
        pins.push(file);
    }
    Ok(pins)
}
pub fn bootstrap_context(policy: &WindowsPolicy) -> Result<InstallContext> {
    policy.validate()?;
    let program_data = known(&FOLDERID_ProgramData)?;
    let _pins = pin_path(&program_data)?;
    let state_root = program_data.join(format!("{} Update", policy.product_directory));
    secure_public_directory(&state_root)?;
    secure_directory(&state_root.join("operations"))?;
    let identity_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state_root.join("install.json.lock"))?;
    identity_lock
        .try_lock()
        .context("another installation/uninstall is changing the machine identity")?;
    let runtime_gate = state_root.join("runtime.lock");
    if !runtime_gate.exists() {
        let sd = SecurityDescriptor::new("O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FR;;;BU)")?;
        unsafe {
            let handle = CreateFileW(
                wide(&runtime_gate).as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &sd.attributes(),
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            );
            ensure!(handle != INVALID_HANDLE_VALUE, "cannot create runtime gate");
            drop(Handle(handle));
        }
    }
    verify_protected(&runtime_gate)?;
    let id_path = state_root.join("installation-id");
    let mut id = if id_path.exists() {
        verify_protected(&id_path)?;
        std::fs::read_to_string(&id_path)?
    } else {
        let id = Uuid::new_v4().to_string();
        write_identity(&id_path, &id)?;
        id
    };
    identifier(&id)?;
    let head = state_root.join("install.json");
    if head.exists() {
        verify_protected(&head)?;
        let journal: crate::InstallJournal = serde_json::from_slice(&std::fs::read(head)?)?;
        ensure!(
            journal.app_id == policy.app_id,
            "foreign protected identity journal"
        );
        if identity_is_retired(&journal, &id)? {
            id = Uuid::new_v4().to_string();
            write_identity(&id_path, &id)?;
        }
    }
    // Temp may be shared; keep its handle pinned for the lifetime of the service.
    let temp = program_data.join("Temp");
    if !temp.exists() {
        secure_directory(&temp)?;
    }
    let _temp_pin = pin_path(&temp)?;
    let staging_root = temp.join("Installer");
    secure_directory(&staging_root)?;
    let app_stage = staging_root.join(&policy.app_id);
    secure_directory(&app_stage)?;
    Ok(InstallContext {
        app_id: policy.app_id.clone(),
        channel: policy.channel.clone(),
        target: policy.target.clone(),
        installation_id: id,
        active_dir: policy.application_directory()?,
        staging_root: app_stage,
        state_dir: state_root,
    })
}
fn identity_is_retired(journal: &crate::InstallJournal, id: &str) -> Result<bool> {
    let Some(retired) = &journal.retired_installation_id else {
        return Ok(false);
    };
    ensure!(
        !journal.uninstall_pending
            && journal.phase == crate::InstallPhase::Failed
            && journal
                .operation
                .as_ref()
                .is_some_and(|op| op.progress.stage == crate::progress::Stage::Uninstalled),
        "identity retirement requires a completed full uninstall"
    );
    Ok(retired == id)
}
fn write_identity(path: &Path, id: &str) -> Result<()> {
    use std::io::Write;
    let temp = path.with_file_name(format!(".identity-{}.tmp", Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(id.as_bytes())?;
    file.sync_all()?;
    drop(file);
    set_new_file_owner(&temp)?;
    unsafe {
        if MoveFileExW(
            wide(&temp).as_ptr(),
            wide(path).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        ) == 0
        {
            let error = std::io::Error::last_os_error();
            let _ = std::fs::remove_file(&temp);
            return Err(error.into());
        }
    }
    verify_protected(path)
}
pub fn verify_authenticode(path: &Path, expected_certificate: &str) -> Result<()> {
    hash(expected_certificate)?;
    let _guard = inventory::package_guard(path)?;
    unsafe {
        let name = wide(path);
        let mut file: WINTRUST_FILE_INFO = zeroed();
        file.cbStruct = size_of::<WINTRUST_FILE_INFO>() as u32;
        file.pcwszFilePath = name.as_ptr();
        let mut data: WINTRUST_DATA = zeroed();
        data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous.pFile = &mut file;
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        data.dwProvFlags =
            WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE | WTD_DISABLE_MD2_MD4;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = WinVerifyTrust(
            null_mut(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        let result = (|| -> Result<()> {
            ensure!(
                status == 0,
                "offline Authenticode verification failed: {status:#x}"
            );
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            ensure!(!provider.is_null(), "Authenticode provider missing");
            let signer = WTHelperGetProvSignerFromChain(provider, 0, 0, 0);
            ensure!(!signer.is_null(), "Authenticode signer missing");
            let cert = WTHelperGetProvCertFromChain(signer, 0);
            ensure!(
                !cert.is_null() && !(*cert).pCert.is_null(),
                "Authenticode certificate missing"
            );
            let cert = &*(*cert).pCert;
            let der = std::slice::from_raw_parts(cert.pbCertEncoded, cert.cbCertEncoded as usize);
            ensure!(
                sha256(der) == expected_certificate,
                "unexpected Authenticode publisher certificate"
            );
            Ok(())
        })();
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(
            null_mut(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        result
    }
}
pub fn read_product_version(path: &Path) -> Result<Version> {
    let _guard = inventory::package_guard(path)?;
    unsafe {
        let path = wide(path);
        let size = GetFileVersionInfoSizeW(path.as_ptr(), null_mut());
        ensure!(
            size > 0 && size < 1024 * 1024,
            "missing/oversized product version resource"
        );
        let mut bytes = vec![0u8; size as usize];
        win(GetFileVersionInfoW(
            path.as_ptr(),
            0,
            size,
            bytes.as_mut_ptr().cast(),
        ))?;
        let mut value = null_mut();
        let mut len = 0;
        win(VerQueryValueW(
            bytes.as_ptr().cast(),
            wide("\\").as_ptr(),
            &mut value,
            &mut len,
        ))?;
        ensure!(
            len as usize >= size_of::<VS_FIXEDFILEINFO>(),
            "invalid version resource"
        );
        let v = std::ptr::read_unaligned(value as *const VS_FIXEDFILEINFO);
        ensure!(
            v.dwSignature == 0xfeef04bd && v.dwProductVersionLS & 0xffff == 0,
            "unsupported product version revision"
        );
        let fixed = Version::new(
            (v.dwProductVersionMS >> 16) as u64,
            (v.dwProductVersionMS & 0xffff) as u64,
            (v.dwProductVersionLS >> 16) as u64,
        );
        let mut translations = null_mut();
        let mut translations_len = 0;
        if VerQueryValueW(
            bytes.as_ptr().cast(),
            wide("\\VarFileInfo\\Translation").as_ptr(),
            &mut translations,
            &mut translations_len,
        ) != 0
        {
            ensure!(
                translations_len <= 1024 && translations_len.is_multiple_of(4),
                "invalid version translations"
            );
            for entry in std::slice::from_raw_parts(
                translations.cast::<u16>(),
                translations_len as usize / 2,
            )
            .chunks_exact(2)
            {
                let field = format!(
                    "\\StringFileInfo\\{:04x}{:04x}\\ProductVersion",
                    entry[0], entry[1]
                );
                let mut text = null_mut();
                let mut count = 0;
                if VerQueryValueW(
                    bytes.as_ptr().cast(),
                    wide(field).as_ptr(),
                    &mut text,
                    &mut count,
                ) != 0
                    && count > 1
                    && count < 256
                {
                    let string = String::from_utf16(std::slice::from_raw_parts(
                        text.cast::<u16>(),
                        count as usize - 1,
                    ))?;
                    if let Ok(semver) = Version::parse(&string) {
                        ensure!(
                            (semver.major, semver.minor, semver.patch)
                                == (fixed.major, fixed.minor, fixed.patch),
                            "conflicting product version resources"
                        );
                        return Ok(semver);
                    }
                }
            }
        }
        Ok(fixed)
    }
}
fn inspect_architecture(path: &Path, target: &str) -> Result<()> {
    let mut file = inventory::package_guard(path)?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos)?;
    ensure!(&dos[..2] == b"MZ", "not PE");
    let offset = u32::from_le_bytes(dos[60..64].try_into()?);
    ensure!(
        (64..=16 * 1024 * 1024).contains(&offset),
        "invalid PE offset"
    );
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut pe = [0u8; 6];
    file.read_exact(&mut pe)?;
    ensure!(&pe[..4] == b"PE\0\0", "invalid PE signature");
    let expected = if target.contains("aarch64") {
        0xaa64
    } else {
        0x8664
    };
    ensure!(
        u16::from_le_bytes(pe[4..6].try_into()?) == expected,
        "wrong product architecture"
    );
    Ok(())
}
pub(crate) fn registry_string(key: &str, name: &str) -> Result<String> {
    unsafe {
        let mut size = 0u32;
        let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY;
        ensure!(
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                wide(key).as_ptr(),
                wide(name).as_ptr(),
                flags,
                null_mut(),
                null_mut(),
                &mut size
            ) == 0
                && size <= 65536,
            "required registry value unavailable"
        );
        let mut raw = vec![0u16; (size as usize).div_ceil(2)];
        ensure!(
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                wide(key).as_ptr(),
                wide(name).as_ptr(),
                flags,
                null_mut(),
                raw.as_mut_ptr().cast(),
                &mut size
            ) == 0,
            "registry read failed"
        );
        Ok(String::from_utf16(
            &raw[..raw.iter().position(|v| *v == 0).unwrap_or(raw.len())],
        )?)
    }
}
pub struct WindowsInstaller {
    pub policy: WindowsPolicy,
}
#[async_trait]
impl PlatformInstaller for WindowsInstaller {
    async fn acquire_activation_guard(
        &self,
        context: &InstallContext,
    ) -> Result<Box<dyn Send + Sync>> {
        let path = context.state_dir.join("runtime.lock");
        verify_protected(&path)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        // ClosingApplications is delivered before this wait. The initiating app
        // can save its work and exit; the service owns the accepted transaction
        // even if that app's pipe disconnects. Other sessions are never killed.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Box::new(file)),
                Err(std::fs::TryLockError::WouldBlock)
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(error) => {
                    return Err(error)
                        .context("close application in every Windows session before installing");
                }
            }
        }
    }
    async fn installed_version(&self, context: &InstallContext) -> Result<Option<Version>> {
        let exe = context.active_dir.join(&self.policy.executable);
        if !exe.exists() {
            return Ok(None);
        }
        if verify_authenticode(&exe, &self.policy.publisher_certificate_sha256).is_err() {
            return Ok(None);
        }
        inspect_architecture(&exe, &context.target)?;
        Ok(Some(read_product_version(&exe)?))
    }
    async fn preflight(
        &self,
        context: &InstallContext,
        contract: &InstallContract,
        package_bytes: u64,
    ) -> Result<PreflightReport> {
        self.policy.validate()?;
        contract.validate()?;
        ensure!(
            contract.executable == self.policy.executable
                && contract.publisher_certificate_sha256
                    == self.policy.publisher_certificate_sha256,
            "contract differs from trusted product identity"
        );
        let build: u32 = registry_string(
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
            "CurrentBuildNumber",
        )?
        .parse()?;
        ensure!(
            build >= contract.minimum_windows_build,
            "Windows build below minimum"
        );
        unsafe {
            let mut info: SYSTEM_INFO = zeroed();
            GetNativeSystemInfo(&mut info);
            let arch = info.Anonymous.Anonymous.wProcessorArchitecture;
            ensure!(
                (arch == 9 && context.target.contains("x86_64"))
                    || (arch == 12 && context.target.contains("aarch64")),
                "native Windows architecture mismatch"
            );
        }
        let mut webview2_version = None;
        if contract.minimum_webview2_version != [0; 4] {
            let version = registry_string("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}", "pv")
                .or_else(|_| registry_string("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}", "pv"))?;
            let parts = version
                .split('.')
                .map(str::parse::<u32>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ensure!(
                parts.len() == 4
                    && parts.as_slice() >= contract.minimum_webview2_version.as_slice(),
                "WebView2 runtime below minimum"
            );
            webview2_version = Some(
                parts
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("invalid WebView2 version"))?,
            );
        }
        verify_protected(&context.staging_root)?;
        verify_protected(&context.state_dir)?;
        let _pins = pin_path(&context.staging_root)?;
        let parent = context.active_dir.parent().context("active parent")?;
        let _active_pins = pin_path(parent)?;
        // NSIS moves app entries individually to backup, preserving Service.
        // These renames cannot cross volumes; reject before any mutation.
        ensure!(
            volume_name(&context.staging_root)? == volume_name(parent)?,
            "staging and application directories must be on the same volume"
        );
        let previous_bytes = if context.active_dir.exists() {
            inventory::installed_inventory(&context.active_dir)?
                .iter()
                .try_fold(0u64, |n, f| {
                    n.checked_add(f.size_bytes)
                        .context("existing footprint overflow")
                })?
        } else {
            0
        };
        let required = contract
            .staging_bytes
            .checked_add(contract.active_bytes)
            .and_then(|v| v.checked_add(contract.backup_bytes.max(previous_bytes)))
            .and_then(|v| v.checked_add(package_bytes))
            .context("disk budget overflow")?;
        let mut available_bytes = Vec::new();
        for path in [&context.staging_root, parent] {
            let mut available = 0;
            unsafe {
                win(GetDiskFreeSpaceExW(
                    wide(path).as_ptr(),
                    &mut available,
                    null_mut(),
                    null_mut(),
                ))?;
            }
            ensure!(
                available >= required,
                "insufficient space for staging, active files, package and backup"
            );
            available_bytes.push(available);
            let probe = path.join(format!(".update-write-probe-{}", Uuid::new_v4()));
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&probe)?;
            drop(file);
            std::fs::remove_file(probe)?;
        }
        Ok(PreflightReport {
            required_total_bytes: required,
            available_staging_bytes: available_bytes[0],
            available_active_bytes: available_bytes[1],
            native_target: context.target.clone(),
            windows_build: Some(build),
            webview2_version,
            paths_writable: true,
        })
    }
    async fn inspect_package(
        &self,
        package: &Path,
        contract: &InstallContract,
        version: &Version,
        _: &str,
    ) -> Result<()> {
        verify_protected(package)?;
        verify_authenticode(package, &contract.publisher_certificate_sha256)?;
        ensure!(
            read_product_version(package)? == *version,
            "NSIS product version differs from signed release"
        );
        Ok(())
    }
    async fn stage(&self, package: &Path, destination: &Path) -> Result<()> {
        ensure!(!destination.exists(), "stage directory exists");
        run_nsis(package, "stage", destination, None).await
    }
    async fn ensure_applications_closed(&self, context: &InstallContext) -> Result<()> {
        // Enumerates all sessions. We never force-kill a process or discard user work.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            ensure!(
                snapshot != INVALID_HANDLE_VALUE,
                "process enumeration failed"
            );
            let _snapshot = Handle(snapshot);
            let mut entry: PROCESSENTRY32W = zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry);
            while more != 0 {
                let n = entry
                    .szExeFile
                    .iter()
                    .position(|v| *v == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..n]);
                let process =
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                if !process.is_null() {
                    let _process = Handle(process);
                    let mut raw = vec![0u16; 32768];
                    let mut len = raw.len() as u32;
                    if QueryFullProcessImageNameW(process, 0, raw.as_mut_ptr(), &mut len) != 0 {
                        let path = PathBuf::from(String::from_utf16_lossy(&raw[..len as usize]));
                        ensure!(
                            !path.starts_with(&context.active_dir)
                                || path.starts_with(self.policy.service_directory()?),
                            "close application in every Windows session before installing"
                        );
                    } else {
                        ensure!(
                            !name.eq_ignore_ascii_case(&self.policy.executable),
                            "cannot inspect running product process"
                        );
                    }
                } else {
                    ensure!(
                        !name.eq_ignore_ascii_case(&self.policy.executable),
                        "cannot inspect running product process"
                    );
                }
                more = Process32NextW(snapshot, &mut entry);
            }
            ensure!(
                GetLastError() == ERROR_NO_MORE_FILES,
                "process enumeration interrupted"
            );
        }
        Ok(())
    }
    async fn activate(
        &self,
        package: &Path,
        staging: &Path,
        context: &InstallContext,
    ) -> Result<()> {
        let _pins = pin_path(staging)?;
        run_nsis(package, "activate", staging, Some(&context.active_dir)).await
    }
    async fn verify_registration(&self, context: &InstallContext, version: &Version) -> Result<()> {
        let exe = context.active_dir.join(&self.policy.executable);
        verify_authenticode(&exe, &self.policy.publisher_certificate_sha256)?;
        inspect_architecture(&exe, &context.target)?;
        let key = format!(
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{}",
            self.policy.app_id
        );
        ensure!(
            registry_string(&key, "DisplayVersion")? == version.to_string(),
            "program registration version mismatch"
        );
        ensure!(
            Path::new(&registry_string(&key, "InstallLocation")?) == context.active_dir,
            "program registration path mismatch"
        );
        if let Ok(command) = registry_string(&key, "ModifyPath")
            && !command.is_empty()
        {
            let helper = self.policy.service_executable()?;
            let directory = helper.parent().context("service directory unavailable")?;
            let prefix = format!("\"{}\\maintenance-", directory.display());
            let hash = command
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".exe\" --repair"))
                .context("unexpected maintenance command")?;
            ensure!(
                hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid maintenance hash"
            );
            let maintenance = directory.join(format!("maintenance-{hash}.exe"));
            let _guard = inventory::package_guard(&maintenance)?;
            verify_protected(&maintenance)?;
            verify_authenticode(&maintenance, &self.policy.publisher_certificate_sha256)?;
            ensure!(
                inventory::file_digest(&maintenance)?.1 == hash,
                "maintenance file differs from its immutable identity"
            );
            ensure!(
                registry_string(&key, "UninstallString")?
                    == format!("\"{}\" --uninstall", maintenance.display()),
                "unexpected uninstall UI command"
            );
        }
        unsafe {
            let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
            ensure!(!scm.is_null(), "SCM unavailable");
            let service = OpenServiceW(
                scm,
                wide(&self.policy.service_name).as_ptr(),
                SERVICE_QUERY_CONFIG,
            );
            if service.is_null() {
                CloseServiceHandle(scm);
                anyhow::bail!("update service registration missing");
            }
            let mut needed = 0;
            QueryServiceConfigW(service, null_mut(), 0, &mut needed);
            let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
            let ok = QueryServiceConfigW(service, buffer.as_mut_ptr().cast(), needed, &mut needed);
            CloseServiceHandle(service);
            CloseServiceHandle(scm);
            win(ok)?;
            let config = &*(buffer.as_ptr() as *const QUERY_SERVICE_CONFIGW);
            let mut n = 0;
            while *config.lpBinaryPathName.add(n) != 0 {
                n += 1;
            }
            let command =
                String::from_utf16_lossy(std::slice::from_raw_parts(config.lpBinaryPathName, n));
            ensure!(
                command == format!("\"{}\" run", self.policy.service_executable()?.display()),
                "unexpected update service binary"
            );
        }
        Ok(())
    }
}
/// Fixed UI entry point: the signed helper remains responsible for removal.
pub fn launch_uninstall(policy: &WindowsPolicy) -> Result<()> {
    policy.validate()?;
    let executable = policy.service_executable()?;
    verify_protected(&executable)?;
    verify_authenticode(&executable, &policy.publisher_certificate_sha256)?;
    unsafe {
        let executable = wide(executable);
        let verb = wide("runas");
        let arguments = wide("uninstall");
        let mut launch: SHELLEXECUTEINFOW = zeroed();
        launch.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        launch.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        launch.lpVerb = verb.as_ptr();
        launch.lpFile = executable.as_ptr();
        launch.lpParameters = arguments.as_ptr();
        launch.nShow = 0;
        win(ShellExecuteExW(&mut launch))?;
        ensure!(!launch.hProcess.is_null(), "uninstall process unavailable");
        let _process = Handle(launch.hProcess);
        ensure!(
            WaitForSingleObject(launch.hProcess, INFINITE) == WAIT_OBJECT_0,
            "uninstaller wait failed"
        );
        let mut code = 0;
        win(GetExitCodeProcess(launch.hProcess, &mut code))?;
        ensure!(code == 0, "uninstall failed: {code}");
    }
    Ok(())
}

pub fn launch_product(policy: &WindowsPolicy) -> Result<()> {
    policy.validate()?;
    let executable = policy.application_directory()?.join(&policy.executable);
    verify_protected(&executable)?;
    verify_authenticode(&executable, &policy.publisher_certificate_sha256)?;
    std::process::Command::new(executable).spawn()?;
    Ok(())
}

/// The program registration invokes this elevated entry point. It shares the
/// install lock and removes fixed product files only, preserving all user data.
pub async fn uninstall_product() -> Result<()> {
    let source = std::env::current_exe()?;
    let file = source
        .parent()
        .context("service directory")?
        .join("policy.json");
    verify_protected(&file)?;
    let signed: Signed<WindowsPolicy> = serde_json::from_slice(&std::fs::read(file)?)?;
    let policy = verified_policy(&signed)?;
    ensure!(
        source == policy.service_executable()?,
        "uninstaller launched from unexpected location"
    );
    verify_authenticode(&source, &policy.publisher_certificate_sha256)?;
    if elevate_uninstaller_if_needed(&source)? {
        return Ok(());
    }
    let context = bootstrap_context(&policy)?;
    let _state_pins = pin_path(&context.state_dir)?;
    let _stage_pins = pin_path(&context.staging_root)?;
    let head = context.state_dir.join("install.json");
    let _lock = crate::journal::lock(&head).await?;
    let mut journal = crate::InstallJournal::load(&head).await?;
    ensure!(
        !matches!(
            journal.phase,
            crate::InstallPhase::Installing | crate::InstallPhase::AwaitingRestart
        ),
        "reconcile pending install before uninstalling"
    );
    let files = journal.uninstall_files(&policy.metadata_keys)?;
    let manifest = &journal
        .operation
        .as_ref()
        .context("operation unavailable")?
        .bundle
        .manifest
        .body;
    let contract = manifest
        .installation
        .as_ref()
        .context("contract unavailable")?;
    ensure!(
        manifest.app_id == policy.app_id
            && manifest.channel == policy.channel
            && manifest.target == policy.target
            && contract.executable == policy.executable
            && contract.publisher_certificate_sha256 == policy.publisher_certificate_sha256,
        "uninstall inventory differs from trusted product"
    );
    let platform = WindowsInstaller {
        policy: policy.clone(),
    };
    let _runtime_guard = platform.acquire_activation_guard(&context).await?;
    platform.ensure_applications_closed(&context).await?;
    let _active_parent_pins = pin_path(context.active_dir.parent().context("product parent")?)?;
    // Preserve the previous Completed journal before recording a destructive step.
    if journal.last_verified_release.is_none() && journal.phase == crate::InstallPhase::Completed {
        journal.last_verified_release = journal.operation.as_ref().map(|op| op.bundle.clone());
    }
    journal.uninstall_pending = true;
    journal.phase = crate::InstallPhase::Failed;
    journal.updated_at = chrono::Utc::now();
    if let Some(op) = &mut journal.operation {
        op.progress.stage = crate::progress::Stage::Interrupted;
        op.progress.error =
            Some("Uninstall interrupted or in progress; run uninstall again".into());
    }
    save_uninstall_state(&journal, &context).await?;
    remove_known_product_files(&context.active_dir, &files)?;
    unsafe {
        let status = RegDeleteTreeW(
            HKEY_LOCAL_MACHINE,
            wide(format!(
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{}",
                policy.app_id
            ))
            .as_ptr(),
        );
        ensure!(
            status == 0 || status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND,
            "cannot remove program registration; administrator rights required"
        );
    }
    journal.phase = crate::InstallPhase::Failed;
    journal.uninstall_pending = false;
    journal.retired_installation_id = Some(context.installation_id.clone());
    journal.last_verified_release = None;
    journal.updated_at = chrono::Utc::now();
    if let Some(op) = &mut journal.operation {
        op.progress.stage = crate::progress::Stage::Uninstalled;
        op.progress.error = None;
    }
    save_uninstall_state(&journal, &context).await?;
    // Keep the helper and monotonic floor. The retired identity rotates before
    // reinstall; removed files no longer supply any application launch proof.
    Ok(())
}
async fn save_uninstall_state(
    journal: &crate::InstallJournal,
    context: &InstallContext,
) -> Result<()> {
    // As in InstallEngine, save the protected head/floor before the operation
    // archive; repeating uninstall repairs an interrupted archive write.
    journal
        .save(&context.state_dir.join("install.json"))
        .await?;
    if let Some(op) = &journal.operation {
        identifier(&op.operation_id)?;
        journal
            .save(
                &context
                    .state_dir
                    .join("operations")
                    .join(format!("{}.json", op.operation_id)),
            )
            .await?;
    }
    Ok(())
}
/// The normal ARP entry point always elevates this same signed helper with one
/// fixed command. No caller-controlled path, arguments or shell string crosses UAC.
fn elevate_uninstaller_if_needed(source: &Path) -> Result<bool> {
    unsafe {
        let mut token = null_mut();
        win(OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY,
            &mut token,
        ))?;
        let _token = Handle(token);
        let mut elevation: TOKEN_ELEVATION = zeroed();
        let mut returned = 0;
        win(GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        ))?;
        if elevation.TokenIsElevated != 0 {
            return Ok(false);
        }
        let executable = wide(source);
        let verb = wide("runas");
        let arguments = wide("uninstall");
        let mut launch: SHELLEXECUTEINFOW = zeroed();
        launch.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        launch.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        launch.lpVerb = verb.as_ptr();
        launch.lpFile = executable.as_ptr();
        launch.lpParameters = arguments.as_ptr();
        launch.nShow = 0;
        win(ShellExecuteExW(&mut launch))?;
        ensure!(
            !launch.hProcess.is_null(),
            "elevated uninstall process unavailable"
        );
        let _process = Handle(launch.hProcess);
        ensure!(
            WaitForSingleObject(launch.hProcess, INFINITE) == WAIT_OBJECT_0,
            "uninstaller wait failed"
        );
        let mut code = 0;
        win(GetExitCodeProcess(launch.hProcess, &mut code))?;
        ensure!(code == 0, "elevated uninstall failed: {code}");
        Ok(true)
    }
}
fn remove_known_product_files(root: &Path, files: &[String]) -> Result<()> {
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    let _root_pins = pin_path(root)?;
    verify_protected(root)?;
    let known: std::collections::BTreeSet<_> =
        files.iter().map(|p| p.to_ascii_lowercase()).collect();
    // Walk and reject every reparse component before deleting anything. Intersect
    // actual regular files with both signed inventories, handling file/directory
    // transitions between releases without ever recursively deleting user data.
    let actual = inventory::installed_inventory(root)?;
    for file in actual
        .iter()
        .filter(|f| known.contains(&f.path.to_ascii_lowercase()))
    {
        let path = root.join(&file.path);
        let _parent_pins = pin_path(path.parent().context("payload parent")?)?;
        inventory::regular_metadata(&path, false)?;
        std::fs::remove_file(path)?;
    }
    let mut directories = std::collections::BTreeSet::new();
    for file in files {
        for parent in Path::new(file)
            .ancestors()
            .skip(1)
            .filter(|p| !p.as_os_str().is_empty())
        {
            directories.insert(parent.to_path_buf());
        }
    }
    let mut directories: Vec<_> = directories.into_iter().collect();
    directories.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for directory in directories {
        let _ = std::fs::remove_dir(root.join(directory));
    }
    drop(_root_pins);
    let _ = std::fs::remove_dir(root); // unexpected/user files remain
    Ok(())
}
/// Hold for the entire application lifetime, before opening user databases/windows.
/// Compare the compiled version after waiting: a process started during activation
/// must restart as a normal user if its loaded executable has become stale.
pub fn application_guard(policy: &WindowsPolicy, compiled_version: &Version) -> Result<File> {
    policy.validate()?;
    let state = known(&FOLDERID_ProgramData)?.join(format!("{} Update", policy.product_directory));
    let path = state.join("runtime.lock");
    verify_protected(&path)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    file.lock_shared()?;
    verify_protected(&state)?;
    let head = state.join("install.json");
    verify_protected(&head)?;
    let journal: crate::InstallJournal = serde_json::from_slice(&std::fs::read(&head)?)?;
    ensure!(
        !journal.uninstall_pending && journal.app_id == policy.app_id,
        "installation is being uninstalled or belongs to another product"
    );
    let bundle = journal
        .last_verified_release
        .as_ref()
        .or_else(|| {
            (journal.phase == crate::InstallPhase::Completed)
                .then(|| journal.operation.as_ref().map(|op| &op.bundle))
                .flatten()
        })
        .context("installation has not passed final verification; repair it before launching")?;
    bundle.validate(&policy.metadata_keys)?;
    let manifest = &bundle.manifest.body;
    let contract = manifest
        .installation
        .as_ref()
        .context("signed inventory unavailable")?;
    ensure!(
        manifest.app_id == policy.app_id
            && manifest.channel == policy.channel
            && manifest.target == policy.target
            && manifest.version == *compiled_version
            && contract.executable == policy.executable
            && contract.publisher_certificate_sha256 == policy.publisher_certificate_sha256,
        "application does not match the last verified installation"
    );
    inventory::verify_installed_inventory(&policy.application_directory()?, contract)?;
    let active = policy.application_directory()?.join(&policy.executable);
    ensure!(
        read_product_version(&active)? == *compiled_version,
        "application changed while starting; restart as the current user"
    );
    verify_authenticode(&active, &policy.publisher_certificate_sha256)?;
    Ok(file)
}
async fn run_nsis(package: &Path, mode: &str, staging: &Path, active: Option<&Path>) -> Result<()> {
    let _guard = inventory::package_guard(package)?;
    ensure!(
        matches!(mode, "stage" | "activate"),
        "invalid fixed NSIS mode"
    );
    // NSIS GetOptions expects quotes around the value, not the entire option.
    let mut command = format!(
        "\"{}\" /S /BAYKUS_MODE={mode} /BAYKUS_STAGE=\"{}\"",
        package.display(),
        staging.display()
    );
    if let Some(active) = active {
        command.push_str(&format!(" /BAYKUS_ACTIVE=\"{}\"", active.display()));
    }
    let package = package.to_path_buf();
    tokio::task::spawn_blocking(move || run_owned_process(&package, &command)).await??;
    Ok(())
}
fn run_owned_process(executable: &Path, command: &str) -> Result<()> {
    unsafe {
        let job = CreateJobObjectW(null(), null());
        ensure!(!job.is_null(), "cannot create installer job");
        let _job = Handle(job);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        win(SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ))?;
        let mut startup: STARTUPINFOW = zeroed();
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = zeroed();
        let mut command = wide(command);
        win(CreateProcessW(
            wide(executable).as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_SUSPENDED | CREATE_NO_WINDOW,
            null(),
            null(),
            &startup,
            &mut process,
        ))?;
        let _process = Handle(process.hProcess);
        let _thread = Handle(process.hThread);
        // The child executes no instruction until it belongs to a kill-on-close job.
        // A service crash closes the job and kills NSIS and all its descendants.
        if AssignProcessToJobObject(job, process.hProcess) == 0 {
            TerminateProcess(process.hProcess, 1);
            anyhow::bail!("cannot bind NSIS lifetime to the service");
        }
        ensure!(
            ResumeThread(process.hThread) != u32::MAX,
            "cannot resume installer"
        );
        ensure!(
            WaitForSingleObject(process.hProcess, INFINITE) == WAIT_OBJECT_0,
            "installer wait failed"
        );
        let mut code = 0;
        win(GetExitCodeProcess(process.hProcess, &mut code))?;
        ensure!(code == 0, "NSIS operation failed: {code}");
    }
    Ok(())
}

#[cfg(test)]
mod process_tests {
    use super::*;
    #[test]
    fn identity_rotates_only_once_after_a_completed_full_uninstall() {
        let mut journal = crate::InstallJournal {
            app_id: "alpha".into(),
            release_id: "release-2".into(),
            target_version: "2.0.0".parse().unwrap(),
            sequence: 2,
            package_sha256: "a".repeat(64),
            package_path: PathBuf::from("package.exe"),
            phase: crate::InstallPhase::Failed,
            updated_at: chrono::Utc::now(),
            last_verified_release: None,
            uninstall_pending: false,
            retired_installation_id: Some("old-installation".into()),
            operation: Some(OperationDetails {
                operation_id: "operation-1".into(),
                intent: InstallIntent::Install,
                progress: crate::progress::ProgressEvent::stage(
                    "operation-1",
                    InstallIntent::Install,
                    crate::progress::Stage::Uninstalled,
                ),
                bundle: update_protocol::fixtures::bundle(
                    "alpha", "stable", "2.0.0", 2, b"package", 1,
                ),
                installed_version_at_start: None,
            }),
        };
        assert!(identity_is_retired(&journal, "old-installation").unwrap());
        assert!(!identity_is_retired(&journal, "new-installation").unwrap());
        journal.uninstall_pending = true;
        assert!(identity_is_retired(&journal, "old-installation").is_err());
        journal.uninstall_pending = false;
        journal.operation.as_mut().unwrap().progress.stage = crate::progress::Stage::Failed;
        assert!(identity_is_retired(&journal, "old-installation").is_err());
        journal.retired_installation_id = None;
        assert!(!identity_is_retired(&journal, "old-installation").unwrap());
    }
    #[test]
    fn owned_native_process_checks_the_real_child_exit_status() {
        let mut directory = vec![0u16; 32768];
        let size = unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) };
        assert!(size > 0 && size < directory.len() as u32);
        let executable =
            PathBuf::from(String::from_utf16(&directory[..size as usize]).unwrap()).join("cmd.exe");
        let command = |code| format!("\"{}\" /D /C exit {code}", executable.display());
        run_owned_process(&executable, &command(0)).unwrap();
        assert!(run_owned_process(&executable, &command(7)).is_err());
    }
}
