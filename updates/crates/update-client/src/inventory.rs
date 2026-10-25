use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};
use update_protocol::{InstallContract, PayloadFile, relative_payload_path};

pub(crate) fn regular_metadata(path: &Path, directory: bool) -> Result<std::fs::Metadata> {
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        !m.file_type().is_symlink() && m.is_dir() == directory && (directory || m.is_file()),
        "unexpected file type: {}",
        path.display()
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(m.file_attributes() & 0x400 == 0, "reparse point rejected");
    }
    Ok(m)
}
pub(crate) fn file_digest(path: &Path) -> Result<(u64, String)> {
    regular_metadata(path, false)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000); // READ only; OPEN_REPARSE_POINT
    }
    let mut file = options.open(path)?;
    let m = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(m.file_attributes() & 0x400 == 0, "reparse point rejected");
    }
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        size += n as u64;
    }
    ensure!(size == m.len(), "file changed while hashing");
    Ok((size, format!("{:x}", hash.finalize())))
}
fn walk(root: &Path, path: &Path, files: &mut Vec<PayloadFile>, installed: bool) -> Result<()> {
    regular_metadata(path, true)?;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.parent() == Some(root)
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("Service")
        {
            ensure!(installed, "Service is reserved for the product updater");
            regular_metadata(&path, true)?;
            continue;
        }
        let m = std::fs::symlink_metadata(&path)?;
        if m.is_dir() {
            walk(root, &path, files, installed)?;
        } else {
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-UTF8 payload path"))?
                .replace('\\', "/");
            relative_payload_path(&relative)?;
            let (size_bytes, sha256) = file_digest(&path)?;
            files.push(PayloadFile {
                path: relative,
                size_bytes,
                sha256,
            });
            ensure!(files.len() <= 8192, "too many payload files");
        }
    }
    Ok(())
}
/// Run only after signing EXEs/DLLs. Mutating a payload afterwards invalidates its inventory.
pub fn build_inventory(root: &Path) -> Result<Vec<PayloadFile>> {
    let mut files = Vec::new();
    walk(root, root, &mut files, false)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}
/// Installed application files, excluding only the reserved top-level Service directory.
pub fn installed_inventory(root: &Path) -> Result<Vec<PayloadFile>> {
    let mut files = Vec::new();
    walk(root, root, &mut files, true)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}
pub fn verify_inventory(root: &Path, contract: &InstallContract) -> Result<()> {
    verify_files(build_inventory(root)?, contract)
}
pub fn verify_installed_inventory(root: &Path, contract: &InstallContract) -> Result<()> {
    verify_files(installed_inventory(root)?, contract)
}
fn verify_files(files: Vec<PayloadFile>, contract: &InstallContract) -> Result<()> {
    contract.validate()?;
    let actual: BTreeSet<_> = files.iter().map(|f| f.path.as_str()).collect();
    let expected: BTreeSet<_> = contract.files.iter().map(|f| f.path.as_str()).collect();
    ensure!(
        actual == expected,
        "payload contains missing or unexpected files"
    );
    for file in &contract.files {
        let actual = files
            .iter()
            .find(|f| f.path == file.path)
            .expect("checked set");
        ensure!(actual == file, "payload hash/size mismatch: {}", file.path);
    }
    Ok(())
}
/// Keep a verified package handle open until NSIS exits, denying write/delete sharing.
pub(crate) fn package_guard(path: &Path) -> Result<File> {
    regular_metadata(path, false)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    Ok(options.open(path)?)
}
