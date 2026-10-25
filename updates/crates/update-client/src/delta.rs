use anyhow::{Result, ensure};

/// Zstandard raw-reference patch: both ends use the exact previous full artifact.
pub fn create(base: &[u8], result: &[u8]) -> Result<Vec<u8>> {
    let mut compressor = zstd::bulk::Compressor::with_dictionary(10, base)?;
    compressor.include_checksum(true)?;
    Ok(compressor.compress(result)?)
}
pub fn apply(base: &[u8], patch: &[u8], result_size: u64, max_size: u64) -> Result<Vec<u8>> {
    ensure!(
        result_size <= max_size && base.len() as u64 <= max_size,
        "delta exceeds resource limit"
    );
    let mut decompressor = zstd::bulk::Decompressor::with_dictionary(base)?;
    decompressor.window_log_max(28)?;
    let result = decompressor.decompress(patch, usize::try_from(result_size)?)?;
    ensure!(
        result.len() as u64 == result_size,
        "delta result size mismatch"
    );
    Ok(result)
}
