use crate::{ItemKind, Track};
use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

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
    pub mpv_path: String,
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
            mpv_path: String::new(),
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
        library.audio.validate();
        library.session.validate();
        library.discovery_mode = library.discovery_mode.min(2);
        library.audio_budget = library
            .audio_budget
            .clamp(256 * 1024 * 1024, 32 * 1024 * 1024 * 1024);
        Ok(library)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let staging = path.with_extension("json.tmp");
        use std::io::Write;
        let mut writer = std::io::BufWriter::new(fs::File::create(&staging)?);
        serde_json::to_writer(&mut writer, self)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&staging, path).context("Could not save the library")
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
        fs::write(path, serde_json::to_vec_pretty(playlist)?)?;
        Ok(())
    }
    pub fn import_playlist(&mut self, path: &Path) -> Result<String> {
        let file = std::fs::File::open(path)?;
        if file.metadata()?.len() > 10_000_000 {
            bail!("Playlist files must be smaller than 10 MB.");
        }
        let mut playlist: Playlist =
            serde_json::from_reader(file).context("This is not a Cassette playlist file")?;
        if playlist.tracks.len() > 10_000 {
            bail!("A playlist can contain at most 10,000 songs.");
        }
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
        self.import_walk(paths)
    }
    fn import_walk(&mut self, paths: &[PathBuf]) -> usize {
        let before = self.local.len();
        for path in paths {
            if path.is_dir() {
                // A chosen folder is scanned once; symlink directories are not followed.
                if let Ok(entries) = fs::read_dir(path) {
                    let children: Vec<_> = entries
                        .flatten()
                        .filter(|entry| entry.file_type().is_ok_and(|t| !t.is_symlink()))
                        .map(|entry| entry.path())
                        .collect();
                    self.import_walk(&children);
                }
            } else if is_audio(path) && path.is_file() {
                let track = Track::local(path);
                if !self.local.iter().any(|t| t.id == track.id) {
                    self.local.push(track);
                }
            }
        }
        self.local.len() - before
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
