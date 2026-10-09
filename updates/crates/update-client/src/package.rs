use anyhow::{Result, ensure};

pub fn verify_package(bytes: &[u8], signature: &str, public_key: &str) -> Result<()> {
    Ok(update_protocol::verify_package(
        bytes, signature, public_key,
    )?)
}
pub fn verify_content(bytes: &[u8], size: u64, hash: &str) -> Result<()> {
    ensure!(bytes.len() as u64 == size, "artifact size mismatch");
    ensure!(
        update_protocol::sha256(bytes) == hash,
        "artifact SHA-256 mismatch"
    );
    Ok(())
}
