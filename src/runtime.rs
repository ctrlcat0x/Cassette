//! Windows distributions carry compressed, unmodified playback binaries.
#[cfg(all(target_os = "windows", feature = "bundled-runtime"))]
mod embedded {
    use anyhow::{Context, Result, bail};
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        io::{Read, Write},
        path::PathBuf,
        sync::OnceLock,
    };
    include!(concat!(env!("OUT_DIR"), "/runtime.rs"));
    static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

    pub fn prepare() -> Result<()> {
        if DIRECTORY.get().is_some() {
            return Ok(());
        }
        let root = crate::library::data_dir().join("runtime");
        fs::create_dir_all(&root)?;
        crate::auth::storage::restrict(&root)?;
        let destination = root.join(VERSION);
        let complete = destination.join(".complete");
        let completed = fs::metadata(&complete)
            .and_then(|metadata| metadata.modified())
            .ok();
        let ready = completed.is_some()
            && FILES.iter().all(|(name, size, _)| {
                fs::symlink_metadata(destination.join(name)).is_ok_and(|metadata| {
                    metadata.is_file()
                        && metadata.len() == *size
                        && metadata
                            .modified()
                            .is_ok_and(|modified| Some(modified) <= completed)
                })
            });
        if ready {
            let _ = DIRECTORY.set(destination);
            return Ok(());
        }
        // Stage everything before publishing. A failed write never exposes a
        // partial Python runtime or replaces a binary used by a running player.
        let stage = tempfile::Builder::new()
            .prefix(".extract-")
            .tempdir_in(&root)?;
        crate::auth::storage::restrict(stage.path())?;
        let mut source = flate2::read::GzDecoder::new(PAYLOAD);
        let mut buffer = [0u8; 64 * 1024];
        for (name, size, expected) in FILES {
            let path = stage.path().join(name);
            fs::create_dir_all(path.parent().context("Invalid bundled path")?)?;
            let mut output = fs::File::create(&path)?;
            let mut hash = Sha256::new();
            let mut remaining = *size;
            while remaining > 0 {
                let count = remaining.min(buffer.len() as u64) as usize;
                source.read_exact(&mut buffer[..count])?;
                output.write_all(&buffer[..count])?;
                hash.update(&buffer[..count]);
                remaining -= count as u64;
            }
            if <[u8; 32]>::from(hash.finalize()) != *expected {
                bail!("Bundled playback file failed integrity verification: {name}");
            }
            output.sync_all()?;
        }
        // Reading to EOF also validates gzip's checksum before marking ready.
        if source.read(&mut buffer[..1])? != 0 {
            bail!("Unexpected bundled playback data");
        }
        fs::write(stage.path().join(".complete"), VERSION)?;
        if destination.exists() {
            // Quarantine a damaged cache once; subsequent launches reuse the repair.
            let damaged = root.join(format!(".damaged-{VERSION}-{:x}", rand::random::<u64>()));
            fs::rename(&destination, damaged)?;
        }
        fs::rename(stage.path(), &destination)?;
        let _ = DIRECTORY.set(destination);
        Ok(())
    }

    pub fn executable(name: &str) -> Option<PathBuf> {
        let root = DIRECTORY.get()?;
        Some(match name {
            "yt-dlp" => root.join("yt-dlp/yt-dlp.exe"),
            "mpv" | "deno" => root.join(format!("{name}.exe")),
            _ => return None,
        })
    }
}

pub fn prepare() -> anyhow::Result<()> {
    #[cfg(all(target_os = "windows", feature = "bundled-runtime"))]
    embedded::prepare()?;
    Ok(())
}

pub(crate) fn executable(name: &str) -> Option<std::path::PathBuf> {
    #[cfg(all(target_os = "windows", feature = "bundled-runtime"))]
    return embedded::executable(name);
    #[cfg(not(all(target_os = "windows", feature = "bundled-runtime")))]
    {
        let _ = name;
        None
    }
}
