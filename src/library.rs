use crate::{ItemKind, Track};
use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LocalAlbumKey {
    Tagged(String, String),
    Folder(PathBuf, String),
}

impl LocalAlbumKey {
    pub fn for_track(track: &Track) -> Self {
        let title = track.album.trim().to_lowercase();
        if !title.is_empty()
            && let Some(artist) = track
                .local_metadata
                .as_ref()
                .and_then(|tag| tag.album_artist.as_deref())
        {
            return Self::Tagged(title, artist.to_lowercase());
        }
        let folder = Path::new(&track.id)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        Self::Folder(folder.to_owned(), title)
    }
}

pub struct LocalAlbum {
    pub key: LocalAlbumKey,
    pub title: String,
    pub artist: String,
    pub artwork: Option<String>,
    pub track_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub smart: Option<crate::organization::SmartRule>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistView {
    CompactGrid,
    ArtworkGrid,
    #[default]
    #[serde(other)]
    List,
}

impl PlaylistView {
    pub fn next(self) -> Self {
        match self {
            Self::List => Self::CompactGrid,
            Self::CompactGrid => Self::ArtworkGrid,
            Self::ArtworkGrid => Self::List,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Library {
    pub lyrics: crate::lyrics::Preferences,
    pub language: String,
    pub ui_scale: f32,
    pub folders: Vec<crate::organization::Folder>,
    pub playlist_metadata: std::collections::HashMap<String, crate::organization::PlaylistMetadata>,
    pub active_folder: Option<String>,
    pub playlist_order: Vec<String>,
    pub liked: Vec<Track>,
    pub history: Vec<Track>,
    pub local: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub volume: f64,
    pub sidebar: bool,
    pub playlist_view: PlaylistView,
    pub light: bool,
    pub theme: Option<String>,
    pub accent: Option<u32>,
    pub magic_accent: bool,
    pub autoplay: bool,
    pub reduce_motion: bool,
    pub close_to_tray: bool,
    pub start_at_login: bool,
    pub immersive_artwork: bool,
    pub ytdlp_path: String,
    pub cookies_path: String,
    pub stats: crate::stats::ListeningStats,
    pub session: crate::queue::Session,
    pub audio: crate::audio_settings::AudioSettings,
    pub local_folders: Vec<PathBuf>,
    pub hidden: Vec<String>,
    pub offline_only: bool,
    pub audio_budget: u64,
    pub discovery_mode: usize,
    pub sync_history: bool,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            lyrics: Default::default(),
            language: "en".into(),
            ui_scale: 1.,
            folders: vec![],
            playlist_metadata: Default::default(),
            active_folder: None,
            playlist_order: vec![],
            liked: vec![],
            history: vec![],
            local: vec![],
            playlists: vec![],
            volume: 70.,
            sidebar: true,
            playlist_view: PlaylistView::default(),
            light: false,
            theme: None,
            accent: None,
            magic_accent: false,
            autoplay: false,
            reduce_motion: false,
            close_to_tray: false,
            start_at_login: false,
            immersive_artwork: true,
            ytdlp_path: String::new(),
            cookies_path: String::new(),
            stats: crate::stats::ListeningStats::default(),
            session: Default::default(),
            audio: Default::default(),
            local_folders: vec![],
            hidden: vec![],
            offline_only: false,
            audio_budget: crate::cache::AUDIO_BUDGET,
            discovery_mode: 0,
            sync_history: true,
        }
    }
}

impl Library {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut library: Self = crate::resource::json(fs::File::open(path)?, 64 * 1024 * 1024)
            .context("Library could not be read. The original file has been kept.")?;
        library.ui_scale = if library.ui_scale.is_finite() {
            library.ui_scale.clamp(0.75, 1.75)
        } else {
            1.
        };
        library.lyrics.text_scale = if library.lyrics.text_scale.is_finite() {
            library.lyrics.text_scale.clamp(0.75, 1.75)
        } else {
            1.
        };
        if !["en", "hi", "es"].contains(&library.language.as_str()) {
            library.language = "en".into();
        }
        library.validate_organization();
        library.volume = if library.volume.is_finite() {
            library.volume.clamp(0., 100.)
        } else {
            70.
        };
        if library.audio.output != "auto" && !library.audio.output.starts_with("native:") {
            library.audio.output = "auto".into();
        }
        library.audio.validate();
        library.session.validate();
        library.discovery_mode = library.discovery_mode.min(2);
        library.audio_budget = library
            .audio_budget
            .clamp(256 * 1024 * 1024, 32 * 1024 * 1024 * 1024);
        Ok(library)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let mut staging = tempfile::NamedTempFile::new_in(parent)?;
        {
            let mut writer = std::io::BufWriter::new(staging.as_file_mut());
            serde_json::to_writer(&mut writer, self)?;
            writer.flush()?;
        }
        staging.as_file().sync_all()?;
        staging
            .persist(path)
            .context("Could not save the library")?;
        Ok(())
    }

    pub fn liked(&self, id: &str) -> bool {
        self.liked.iter().any(|t| t.id == id)
    }
    pub fn toggle_like(&mut self, track: Track) {
        if self.liked(&track.id) {
            self.liked.retain(|t| t.id != track.id);
        } else {
            self.liked.insert(0, track);
        }
    }

    pub fn played(&mut self, track: Track) {
        self.history.retain(|t| t.id != track.id);
        self.history.insert(0, track);
        self.history.truncate(200);
    }

    pub fn create_playlist(&mut self, name: &str) -> Result<String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            bail!("Enter a playlist name between 1 and 100 characters.");
        }
        let id = format!("local-{:016x}", rand::random::<u64>());
        self.playlists.push(Playlist {
            id: id.clone(),
            name: name.into(),
            tracks: vec![],
            smart: None,
        });
        Ok(id)
    }

    pub fn add_to_playlist(&mut self, playlist_id: &str, track: Track) -> Result<()> {
        let playlist = self
            .playlists
            .iter_mut()
            .find(|p| p.id == playlist_id)
            .context("Playlist no longer exists")?;
        if playlist.smart.is_some() {
            bail!("Smart playlists update automatically. Edit their rules instead.");
        }
        if !playlist.tracks.iter().any(|t| t.id == track.id) {
            playlist.tracks.push(track);
        }
        Ok(())
    }
    pub fn rename_playlist(&mut self, playlist_id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            bail!("Enter a playlist name between 1 and 100 characters.");
        }
        self.playlists
            .iter_mut()
            .find(|p| p.id == playlist_id)
            .context("Playlist no longer exists")?
            .name = name.into();
        Ok(())
    }
    pub fn remove_from_playlist(&mut self, playlist_id: &str, track_id: &str) {
        if let Some(playlist) = self
            .playlists
            .iter_mut()
            .find(|p| p.id == playlist_id && p.smart.is_none())
        {
            playlist.tracks.retain(|t| t.id != track_id);
        }
    }
    pub fn remove_playlist(&mut self, playlist_id: &str) {
        self.playlists.retain(|p| p.id != playlist_id);
    }
    pub fn export_playlist(&self, id: &str, path: &Path) -> Result<()> {
        let playlist = self
            .playlists
            .iter()
            .find(|p| p.id == id)
            .context("Playlist no longer exists")?;
        let mut portable = playlist.clone();
        portable
            .tracks
            .iter_mut()
            .for_each(Track::clear_account_state);
        fs::write(path, serde_json::to_vec_pretty(&portable)?)?;
        Ok(())
    }
    pub fn import_playlist(&mut self, path: &Path) -> Result<String> {
        let file = std::fs::File::open(path)?;
        if file.metadata()?.len() > 10_000_000 {
            bail!("Playlist files must be smaller than 10 MB.");
        }
        let mut playlist: Playlist = crate::resource::json(file, 10_000_000)
            .context("This is not a Cassette playlist file")?;
        if playlist.tracks.len() > 10_000 {
            bail!("A playlist can contain at most 10,000 songs.");
        }
        playlist
            .tracks
            .iter_mut()
            .for_each(Track::clear_account_state);
        let id = self.create_playlist(&playlist.name)?;
        playlist.id = id.clone();
        *self
            .playlists
            .last_mut()
            .context("Playlist was not created")? = playlist;
        Ok(id)
    }

    pub fn import_paths(&mut self, paths: &[PathBuf]) -> usize {
        for path in paths.iter().filter(|p| p.is_dir()) {
            if !self.local_folders.contains(path) {
                self.local_folders.push(path.clone());
            }
        }
        let before = self.local.len();
        let mut known: HashSet<_> = self.local.iter().map(|track| track.id.clone()).collect();
        let mut pending: Vec<_> = paths.iter().rev().cloned().collect();
        let mut folder_covers = HashMap::new();
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                // A chosen folder is scanned once; symlink directories are not followed.
                if let Ok(entries) = fs::read_dir(&path) {
                    let from = pending.len();
                    pending.extend(
                        entries
                            .flatten()
                            .filter(|entry| entry.file_type().is_ok_and(|t| !t.is_symlink()))
                            .map(|entry| entry.path()),
                    );
                    pending[from..].reverse();
                }
            } else if is_audio(&path)
                && path.is_file()
                && known.insert(path.to_string_lossy().into_owned())
            {
                let mut track = Track::local(&path);
                if track.artwork.is_none()
                    && let Some(folder) = path.parent()
                {
                    track.artwork = folder_covers
                        .entry(folder.to_owned())
                        .or_insert_with(|| folder_cover(folder))
                        .clone();
                }
                self.local.push(track);
            }
        }
        self.local.len() - before
    }

    pub fn local_albums(&self) -> Vec<LocalAlbum> {
        let mut albums = HashMap::new();
        for track in &self.local {
            let key = LocalAlbumKey::for_track(track);
            let artist = track
                .local_metadata
                .as_ref()
                .and_then(|tag| tag.album_artist.as_deref())
                .unwrap_or(&track.artist);
            let album = albums.entry(key.clone()).or_insert_with(|| LocalAlbum {
                key,
                title: local_album_title(track).to_owned(),
                artist: artist.to_owned(),
                artwork: None,
                track_count: 0,
            });
            album.track_count += 1;
            if album.artist != artist {
                album.artist.clear();
            }
            if album.artwork.is_none() {
                album.artwork.clone_from(&track.artwork);
            }
        }
        let mut albums: Vec<_> = albums.into_values().collect();
        albums.sort_by_cached_key(|album| {
            (
                album.title.to_lowercase(),
                album.artist.to_lowercase(),
                album.key.clone(),
            )
        });
        albums
    }

    pub fn local_album_tracks(&self, key: &LocalAlbumKey) -> Vec<Track> {
        let mut tracks: Vec<_> = self
            .local
            .iter()
            .filter(|track| LocalAlbumKey::for_track(track) == *key)
            .cloned()
            .collect();
        tracks.sort_by_cached_key(|track| {
            let metadata = track.local_metadata.as_ref();
            (
                metadata.and_then(|tag| tag.disc).unwrap_or(1),
                metadata.and_then(|tag| tag.number).unwrap_or(u32::MAX),
                Path::new(&track.id)
                    .file_name()
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
                track.id.clone(),
            )
        });
        tracks
    }
}

const MAX_COVER_BYTES: usize = 8 * 1024 * 1024;

fn folder_cover(folder: &Path) -> Option<String> {
    let names = ["cover", "folder", "front"];
    let mut candidates: Vec<_> = fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?;
            let rank = names
                .iter()
                .position(|name| stem.eq_ignore_ascii_case(name))?;
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            (matches!(extension.as_str(), "jpg" | "jpeg" | "png")
                && entry.file_type().ok()?.is_file())
            .then_some((rank, path))
        })
        .collect();
    candidates.sort_unstable();
    candidates.into_iter().find_map(|(_, path)| {
        let mut bytes = Vec::new();
        fs::File::open(path)
            .ok()?
            .take(MAX_COVER_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        store_local_cover(&bytes)
    })
}

pub(crate) fn store_local_cover(bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_COVER_BYTES {
        return None;
    }
    let directory = data_dir().join("artwork");
    let target = directory.join(format!("local-v2-{:x}.jpg", Sha256::digest(bytes)));
    if !target.is_file() {
        let mut reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode().ok()?.thumbnail(768, 768).into_rgb8();
        fs::create_dir_all(&directory).ok()?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory).ok()?;
        image
            .write_to(&mut temporary, image::ImageFormat::Jpeg)
            .ok()?;
        temporary.persist(&target).ok()?;
    }
    Some(target.to_string_lossy().into_owned())
}

pub fn local_album_title(track: &Track) -> &str {
    let title = track.album.trim();
    if title.is_empty() {
        Path::new(&track.id)
            .parent()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default()
    } else {
        title
    }
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" | "aiff" | "alac"
            )
        })
}

pub fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("RAAG_DATA_DIR") {
        return path.into();
    }
    ProjectDirs::from("app", "Raag", "Raag")
        .map(|dirs| dirs.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("raag"))
}

pub fn local_only(track: &Track) -> bool {
    track.kind == ItemKind::Local
}
