//! Bounded disk cache. Complete audio is published atomically; URLs/cookies are never saved.
use crate::{
    Track,
    catalog::validate_video_id,
    playback::{self, StreamQuality},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicU64,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const AUDIO_BUDGET: u64 = 2 * 1024 * 1024 * 1024;
pub const TEMPORARY_TTL: u64 = 48 * 60 * 60;
const MAX_TRACK_BYTES: u64 = 96 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedAudio {
    #[serde(default)]
    pub pinned: bool,
    pub track: Track,
    pub bytes: u64,
    pub last_used: u64,
    pub expires: Option<u64>,
    pub quality: StreamQuality,
}
#[derive(Default, Serialize, Deserialize)]
pub struct AudioIndex {
    pub entries: HashMap<String, CachedAudio>,
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn metadata_fresh(path: &Path, ttl: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < ttl)
}
pub fn key(value: &str) -> String {
    format!("{:x}", Sha1::digest(value.as_bytes()))
}
pub fn artwork_path(url: &str) -> PathBuf {
    crate::library::data_dir()
        .join("artwork")
        .join(format!("{}.jpg", key(crate::artwork::identity(url))))
}
pub fn audio_directory() -> PathBuf {
    crate::library::data_dir().join("audio")
}
fn audio_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.media"))
}
fn read_index(directory: &Path) -> Result<AudioIndex> {
    let path = directory.join("index.json");
    if !path.exists() {
        return Ok(AudioIndex::default());
    }
    serde_json::from_slice(&fs::read(path)?)
        .context("Audio cache index is damaged; original files were kept")
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::create_dir_all(path.parent().context("Missing cache directory")?)?;
    let staging = path.with_extension(format!("{:x}.tmp", rand::random::<u64>()));
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut writer = std::io::BufWriter::new(fs::File::create(&staging)?);
        serde_json::to_writer(&mut writer, value)?;
        writer.flush()?;
        drop(writer);
        fs::rename(&staging, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}
/// Bound replaceable page/lyric metadata independently of the user's library.
/// Called only by workers. The current result survives quota cleanup.
pub fn maintain_metadata(current: &Path) -> Result<()> {
    static LAST: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, Instant>>> =
        std::sync::OnceLock::new();
    let mut last = LAST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let directory = current.parent().context("Missing metadata directory")?;
    if last
        .get(directory)
        .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
    {
        return Ok(());
    }
    prune_metadata(
        current.parent().context("Missing metadata directory")?,
        32 * 1024 * 1024,
        current,
    )?;
    if last.len() >= 16 {
        last.clear();
    }
    last.insert(directory.to_owned(), Instant::now());
    Ok(())
}
fn prune_metadata(directory: &Path, budget: u64, current: &Path) -> Result<()> {
    let mut files: Vec<_> = fs::read_dir(directory)?
        .flatten()
        .filter_map(|file| {
            let path = file.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                return None;
            }
            let metadata = file.metadata().ok()?;
            Some((path, metadata.len(), metadata.modified().ok()?))
        })
        .collect();
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut total: u64 = files.iter().map(|(_, bytes, _)| bytes).sum();
    for (path, bytes, _) in files {
        if total <= budget {
            break;
        }
        if path != current {
            fs::remove_file(path)?;
            total = total.saturating_sub(bytes);
        }
    }
    Ok(())
}
pub fn cached_audio(id: &str) -> Option<(PathBuf, StreamQuality)> {
    validate_video_id(id).ok()?;
    let directory = audio_directory();
    let index = read_index(&directory).ok()?;
    let entry = index.entries.get(id)?;
    let path = audio_path(&directory, id);
    (valid(entry, &path, now())).then(|| (path, entry.quality.clone()))
}
fn valid(entry: &CachedAudio, path: &Path, time: u64) -> bool {
    entry.expires.is_none_or(|expiry| expiry > time)
        && entry.bytes > 0
        && fs::metadata(path).is_ok_and(|m| m.len() == entry.bytes)
}
pub fn offline_tracks() -> Vec<Track> {
    let directory = audio_directory();
    let Ok(index) = read_index(&directory) else {
        return vec![];
    };
    let time = now();
    let mut entries: Vec<_> = index
        .entries
        .values()
        .filter(|e| valid(e, &audio_path(&directory, &e.track.id), time))
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.last_used));
    entries.into_iter().map(|e| e.track.clone()).collect()
}
pub fn store_audio(track: Track, persistent: bool, ytdlp: &str, cookies: &str) -> Result<()> {
    store_audio_controlled(
        track,
        persistent,
        false,
        ytdlp,
        cookies,
        0,
        &AtomicU64::new(0),
    )
}
static WRITE_AUDIO: std::sync::Mutex<()> = std::sync::Mutex::new(());
static BUDGET: AtomicU64 = AtomicU64::new(AUDIO_BUDGET);
static OFFLINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DOWNLOAD_QUALITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
static PLAYBACK_PRIORITY: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

/// Reserve bandwidth for startup/seeking. The lease expires even if the engine fails.
pub(crate) fn prioritize_playback(duration: Duration) {
    *PLAYBACK_PRIORITY.lock().unwrap_or_else(|e| e.into_inner()) =
        Instant::now().checked_add(duration);
}

fn wait_for_playback(generation: u64, cancel: &AtomicU64, deadline: Instant) -> Result<()> {
    loop {
        if cancel.load(std::sync::atomic::Ordering::Acquire) != generation {
            bail!("Download cancelled");
        }
        if offline_only() {
            bail!("Audio downloading is unavailable in offline mode.");
        }
        if Instant::now() >= deadline {
            bail!("Audio download timed out; retry playback");
        }
        let remaining = PLAYBACK_PRIORITY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|until| until.saturating_duration_since(Instant::now()))
            .unwrap_or_default();
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}
pub fn configure_download_quality(quality: crate::audio_settings::Quality) {
    DOWNLOAD_QUALITY.store(quality.level(), std::sync::atomic::Ordering::Relaxed);
}
pub fn configure(budget: u64, offline: bool) {
    BUDGET.store(
        budget.clamp(256 * 1024 * 1024, 32 * 1024 * 1024 * 1024),
        std::sync::atomic::Ordering::Relaxed,
    );
    OFFLINE.store(offline, std::sync::atomic::Ordering::Relaxed);
}
pub fn offline_only() -> bool {
    OFFLINE.load(std::sync::atomic::Ordering::Relaxed)
}
pub fn audio_usage() -> (u64, usize) {
    let index = read_index(&audio_directory()).unwrap_or_default();
    (
        index.entries.values().map(|e| e.bytes).sum(),
        index.entries.values().filter(|e| e.pinned).count(),
    )
}
pub fn remove_audio(id: Option<&str>, temporary_only: bool) -> Result<()> {
    let _guard = WRITE_AUDIO.lock().unwrap();
    let directory = audio_directory();
    let mut index = read_index(&directory)?;
    let keys: Vec<_> = index
        .entries
        .iter()
        .filter(|(key, entry)| {
            id.is_none_or(|id| id == key.as_str()) && (!temporary_only || !entry.pinned)
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in keys {
        let _ = fs::remove_file(audio_path(&directory, &key));
        index.entries.remove(&key);
    }
    write_json(&directory.join("index.json"), &index)
}
pub fn store_audio_controlled(
    track: Track,
    persistent: bool,
    pinned: bool,
    ytdlp: &str,
    cookies: &str,
    generation: u64,
    cancel: &AtomicU64,
) -> Result<()> {
    let _guard = WRITE_AUDIO.lock().unwrap();
    if cancel.load(std::sync::atomic::Ordering::Acquire) != generation {
        bail!("Download cancelled");
    }
    validate_video_id(&track.id)?;
    let directory = audio_directory();
    fs::create_dir_all(&directory)?;
    let mut index = read_index(&directory)?;
    let budget = BUDGET.load(std::sync::atomic::Ordering::Relaxed);
    maintain(&directory, &mut index, budget, now(), &track.id)?;
    if let Some(entry) = index.entries.get_mut(&track.id)
        && valid(entry, &audio_path(&directory, &track.id), now())
    {
        entry.last_used = now();
        entry.track = track;
        entry.pinned |= pinned;
        if persistent || pinned {
            entry.expires = None;
        } else if entry.expires.is_some() {
            entry.expires = Some(now() + TEMPORARY_TTL);
        }
        return write_json(&directory.join("index.json"), &index);
    }
    let quality = crate::audio_settings::Quality::from_level(
        DOWNLOAD_QUALITY.load(std::sync::atomic::Ordering::Relaxed),
    );
    let deadline = Instant::now() + Duration::from_secs(300);
    wait_for_playback(generation, cancel, deadline)?;
    let mut source =
        playback::resolve_with_quality(&track, ytdlp, cookies, generation, cancel, quality)?;
    if !source.url.starts_with("https://")
        || source.quality.protocol.contains("m3u8")
        || source.quality.protocol.contains("dash_segments")
    {
        bail!("This stream format is currently available for online playback only");
    }
    static HTTP: std::sync::OnceLock<Result<reqwest::blocking::Client, reqwest::Error>> =
        std::sync::OnceLock::new();
    let client = HTTP
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!("Audio cache connection: {error}"))?;
    let staging = directory.join(format!("{}.part", track.id));
    let mut result =
        download_audio_controlled(client, &source, &staging, generation, cancel, deadline);
    if result.as_ref().is_err_and(|error| {
        error
            .downcast_ref::<AudioRejected>()
            .is_some_and(|status| matches!(status.0, 403 | 410))
    }) {
        // Signed media addresses can expire between resolution and downloading.
        // Resolve once again; authentication denials and throttling never loop.
        playback::forget_source(&track.id);
        result =
            playback::resolve_with_quality(&track, ytdlp, cookies, generation, cancel, quality)
                .and_then(|fresh| {
                    if !fresh.url.starts_with("https://")
                        || fresh.quality.protocol.contains("m3u8")
                        || fresh.quality.protocol.contains("dash_segments")
                    {
                        bail!("This stream format is currently available for online playback only");
                    }
                    source = fresh;
                    download_audio_controlled(
                        client, &source, &staging, generation, cancel, deadline,
                    )
                });
    }
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = fs::remove_file(&staging);
            playback::forget_source(&track.id);
            return Err(error);
        }
    };
    if cancel.load(std::sync::atomic::Ordering::Acquire) != generation {
        let _ = fs::remove_file(&staging);
        bail!("Download cancelled");
    }
    if let Err(error) = maintain(
        &directory,
        &mut index,
        budget.saturating_sub(bytes),
        now(),
        &track.id,
    ) {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }
    if index
        .entries
        .values()
        .filter(|e| e.track.id != track.id)
        .map(|e| e.bytes)
        .sum::<u64>()
        + bytes
        > budget
    {
        let _ = fs::remove_file(&staging);
        bail!("Offline storage is full. Remove a download or increase the storage limit.");
    }
    fs::rename(&staging, audio_path(&directory, &track.id))?;
    index.entries.insert(
        track.id.clone(),
        CachedAudio {
            pinned,
            track,
            bytes,
            last_used: now(),
            expires: (!(persistent || pinned)).then(|| now() + TEMPORARY_TTL),
            quality: source.quality,
        },
    );
    write_json(&directory.join("index.json"), &index)
}
const AUDIO_CHUNK_BYTES: u64 = 256 * 1024;

#[derive(Debug)]
struct AudioRejected(u16);
impl std::fmt::Display for AudioRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Audio cache request was rejected (HTTP {}). Retry playback.",
            self.0
        )
    }
}
impl std::error::Error for AudioRejected {}

fn download_audio_controlled(
    client: &reqwest::blocking::Client,
    source: &playback::AudioSource,
    staging: &Path,
    generation: u64,
    cancel: &AtomicU64,
    deadline: Instant,
) -> Result<u64> {
    let mut output = fs::File::create(staging)?;
    let mut offset = 0;
    let mut total = None;
    let mut failures = 0;
    static POLICY: std::sync::OnceLock<crate::request_policy::RequestPolicy> =
        std::sync::OnceLock::new();
    let policy = POLICY.get_or_init(Default::default);
    loop {
        // Yield between bounded chunks, never while holding the playback engine.
        wait_for_playback(generation, cancel, deadline)?;
        let end = (offset + AUDIO_CHUNK_BYTES - 1).min(MAX_TRACK_BYTES - 1);
        let mut request = client
            .get(&source.url)
            .header("User-Agent", &source.user_agent);
        for header in &source.headers {
            if let Some((key, value)) = header.split_once(": ")
                && !key.eq_ignore_ascii_case("range")
                && !key.eq_ignore_ascii_case("accept-encoding")
            {
                request = request.header(key, value);
            }
        }
        // Explicit small ranges avoid YouTube's unbounded-request throttling.
        policy.before()?;
        // Playback may have started while provider pacing was sleeping.
        wait_for_playback(generation, cancel, deadline)?;
        let result = request
            .timeout(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(30)),
            )
            .header("Accept-Encoding", "identity")
            .header("Range", format!("bytes={offset}-{end}"))
            .send();
        if let Ok(response) = &result {
            policy.observe(response.status(), response.headers());
        }
        let transient = match &result {
            Ok(response) => matches!(response.status().as_u16(), 502..=504),
            Err(error) => error.is_timeout() || error.is_connect(),
        };
        if transient && failures < 2 {
            failures += 1;
            std::thread::sleep(Duration::from_millis(400));
            continue;
        }
        let response = result
            .and_then(|response| response.error_for_status())
            .map_err(|error| {
                // reqwest's display includes the signed URL; never surface it.
                if let Some(status) = error.status() {
                    anyhow::Error::new(AudioRejected(status.as_u16()))
                } else if error.is_timeout() {
                    anyhow::anyhow!("Audio cache request timed out. Check your connection.")
                } else {
                    anyhow::anyhow!("Audio cache connection failed. Check your connection.")
                }
            })?;
        let partial = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.starts_with("text/") || value.starts_with("application/json")
            })
        {
            bail!("The audio server returned a page instead of audio");
        }
        let expected = if partial {
            let range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|header| header.to_str().ok())
                .context("Missing audio byte range")?;
            let (length, size) = audio_range(range, offset, end)?;
            if total.is_some_and(|previous| previous != size) {
                bail!("Audio size changed during download");
            }
            total = Some(size);
            Some(length)
        } else {
            if offset != 0 || response.status() != reqwest::StatusCode::OK {
                bail!("Audio server did not honour the byte range");
            }
            response.content_length()
        };
        if expected.is_some_and(|size| size > MAX_TRACK_BYTES) {
            bail!("Track exceeds the offline per-track limit");
        }
        let limit = expected
            .unwrap_or(MAX_TRACK_BYTES)
            .min(MAX_TRACK_BYTES - offset);
        let copied = std::io::copy(&mut response.take(limit + 1), &mut output);
        let incomplete = copied
            .as_ref()
            .is_ok_and(|bytes| expected.is_some_and(|length| length != *bytes));
        if (copied.is_err() || incomplete) && failures < 2 {
            output.set_len(offset)?;
            output.seek(SeekFrom::Start(offset))?;
            failures += 1;
            std::thread::sleep(Duration::from_millis(400));
            continue;
        }
        let bytes = copied.context("Audio download was interrupted; retry playback")?;
        if bytes == 0 || bytes > limit || expected.is_some_and(|length| length != bytes) {
            bail!("Audio download was incomplete");
        }
        offset += bytes;
        failures = 0;
        if !partial || total == Some(offset) {
            output.flush()?;
            output.sync_all()?;
            return Ok(offset);
        }
    }
}
fn audio_range(header: &str, offset: u64, requested_end: u64) -> Result<(u64, u64)> {
    let parsed = (|| {
        let (range, total) = header.strip_prefix("bytes ")?.split_once('/')?;
        let (first, last) = range.split_once('-')?;
        Some((
            first.parse::<u64>().ok()?,
            last.parse::<u64>().ok()?,
            total.parse::<u64>().ok()?,
        ))
    })();
    let Some((first, last, total)) = parsed else {
        bail!("Invalid audio byte range");
    };
    if first != offset
        || last < first
        || last != requested_end.min(total.saturating_sub(1))
        || last >= total
        || total > MAX_TRACK_BYTES
    {
        bail!("Invalid audio byte range or track exceeds the offline limit");
    }
    Ok((last - first + 1, total))
}
fn maintain(
    directory: &Path,
    index: &mut AudioIndex,
    budget: u64,
    time: u64,
    protected: &str,
) -> Result<()> {
    let mut entries: Vec<_> = index.entries.values().collect();
    // Temporary discoveries are evicted before frequently played offline tracks.
    entries.sort_by_key(|e| (e.expires.is_none(), e.last_used));
    let mut total: u64 = entries.iter().map(|e| e.bytes).sum();
    let mut removed = Vec::new();
    for entry in entries {
        if entry.track.id == protected || entry.pinned {
            continue;
        }
        let path = audio_path(directory, &entry.track.id);
        if !valid(entry, &path, time) || total > budget {
            if path.exists() {
                fs::remove_file(&path)?;
            }
            removed.push(entry.track.id.clone());
            total = total.saturating_sub(entry.bytes);
        }
    }
    for id in removed {
        index.entries.remove(&id);
    }
    Ok(())
}
pub fn maintain_audio() -> Result<()> {
    let _guard = WRITE_AUDIO.lock().unwrap();
    let directory = audio_directory();
    let mut index = read_index(&directory)?;
    let previous_count = index.entries.len();
    maintain(
        &directory,
        &mut index,
        BUDGET.load(std::sync::atomic::Ordering::Relaxed),
        now(),
        "",
    )?;
    // Clean abandoned downloads and orphaned media after a failed index publish.
    if let Ok(files) = fs::read_dir(&directory) {
        for file in files.flatten() {
            let path = file.path();
            let orphan = path.extension().is_some_and(|ext| {
                ext == "part"
                    || ext == "tmp"
                    || (ext == "media"
                        && path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|id| !index.entries.contains_key(id)))
            });
            if orphan {
                fs::remove_file(path)?;
            }
        }
    }
    if index.entries.len() != previous_count {
        write_json(&directory.join("index.json"), &index)?;
    }
    Ok(())
}
pub fn maintain_artwork() -> Result<()> {
    // Called from artwork workers, with a throttle to keep hot disk hits cheap.
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if last.is_some_and(|time| time.elapsed() < Duration::from_secs(60)) {
        return Ok(());
    }
    *last = Some(std::time::Instant::now());
    // Claim the maintenance interval before I/O; other artwork workers can
    // continue serving covers while this worker scans the disk cache.
    drop(last);
    let library = crate::library::Library::load(&crate::library::data_dir().join("library.json"))?;
    let mut protected = HashSet::new();
    for track in offline_tracks()
        .iter()
        .chain(&library.liked)
        .chain(library.playlists.iter().flat_map(|p| &p.tracks))
    {
        if let Some(url) = &track.artwork {
            let path = artwork_path(url);
            protected.insert(path.with_extension("soft.jpg"));
            protected.insert(path.with_extension("hero.jpg"));
            protected.insert(path.with_extension("hero-v2.jpg"));
            protected.insert(path.with_extension("128.jpg"));
            protected.insert(path.with_extension("256.jpg"));
            protected.insert(path.with_extension("palette.json"));
            protected.insert(path);
        }
    }
    prune_artwork(&protected)
}
pub fn prune_artwork(protected: &HashSet<PathBuf>) -> Result<()> {
    let directory = crate::library::data_dir().join("artwork");
    let Ok(files) = fs::read_dir(directory) else {
        return Ok(());
    };
    let mut files: Vec<_> = files
        .flatten()
        .filter_map(|f| {
            let metadata = f.metadata().ok()?;
            Some((f.path(), metadata.len(), metadata.modified().ok()?))
        })
        .collect();
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    for (path, size, _) in files {
        if total <= 256 * 1024 * 1024 {
            break;
        }
        if !protected.contains(&path) {
            fs::remove_file(path)?;
            total = total.saturating_sub(size);
        }
    }
    Ok(())
}
