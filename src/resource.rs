//! Bounds apply to decoded HTTP bytes, including decompressed chunked responses.
use anyhow::{Result, bail};
use std::io::{BufReader, Read};
pub const PAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const LYRIC_BYTES: u64 = 2 * 1024 * 1024;
pub fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("Response exceeds the resource size limit");
    }
    Ok(bytes)
}
pub fn json<T: serde::de::DeserializeOwned>(reader: impl Read, limit: u64) -> Result<T> {
    // Parse as bytes arrive instead of keeping a second, full response buffer.
    let mut reader = BufReader::new(reader.take(limit.saturating_add(1)));
    let value = serde_json::from_reader(&mut reader);
    if reader.get_ref().limit() == 0 {
        bail!("Response exceeds the resource size limit");
    }
    Ok(value?)
}
