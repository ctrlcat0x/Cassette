pub mod account;
pub mod artwork;
pub mod audio_settings;
pub mod auth;
pub mod cache;
pub mod catalog;
pub mod diagnostics;
pub mod i18n;
pub mod jiosaavn;
pub mod library;
pub mod links;
pub mod lyrics;
mod native_audio;
pub mod organization;
pub mod playback;
pub mod queue;
pub mod request_policy;
pub mod resource;
pub mod runtime;
pub mod stats;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    #[default]
    Song,
    Album,
    Artist,
    Playlist,
    Local,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ArtistRef {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalMetadata {
    pub album_artist: Option<String>,
    pub disc: Option<u32>,
    pub number: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Track {
    #[serde(default)]
    pub explicit: Option<bool>,
    #[serde(default)]
    pub library_add: Option<String>,
    #[serde(default)]
    pub library_remove: Option<String>,
    #[serde(default)]
    pub like_status: Option<bool>,
    #[serde(default)]
    pub set_video_id: Option<String>,
    #[serde(default)]
    pub artists: Vec<ArtistRef>,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: u64,
    pub artwork: Option<String>,
    pub kind: ItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_metadata: Option<LocalMetadata>,
}

impl Track {
    pub fn clear_account_state(&mut self) {
        self.library_add = None;
        self.library_remove = None;
        self.like_status = None;
        self.set_video_id = None;
    }

    pub fn song(
        id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            artist: artist.into(),
            ..Self::default()
        }
    }

    pub fn artwork_key(&self) -> &str {
        self.artwork
            .as_deref()
            .map(crate::artwork::identity)
            .unwrap_or(&self.id)
    }

    pub fn playable(&self) -> bool {
        matches!(self.kind, ItemKind::Song | ItemKind::Local)
    }

    pub fn local(path: &std::path::Path) -> Self {
        let mut track = Self {
            id: path.to_string_lossy().into_owned(),
            title: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            artist: "Local file".into(),
            kind: ItemKind::Local,
            ..Self::default()
        };
        // Runs on the import worker, never on a render or playback-clock path.
        use lofty::{
            file::{AudioFile, TaggedFileExt},
            picture::PictureType,
            tag::{Accessor, ItemKey},
        };
        if let Ok(file) = lofty::read_from_path(path) {
            track.duration = file.properties().duration().as_secs();
            if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
                if let Some(title) = tag.title() {
                    track.title = title.into_owned();
                }
                if let Some(artist) = tag.artist() {
                    track.artist = artist.into_owned();
                }
                if let Some(album) = tag.album() {
                    track.album = album.trim().to_owned();
                }
                track.local_metadata = Some(LocalMetadata {
                    album_artist: tag
                        .get_string(&ItemKey::AlbumArtist)
                        .map(str::trim)
                        .filter(|artist| !artist.is_empty())
                        .map(str::to_owned),
                    disc: tag.disk().filter(|number| *number > 0),
                    number: tag.track().filter(|number| *number > 0),
                });
                let pictures = tag.pictures();
                let front = pictures
                    .iter()
                    .find(|picture| picture.pic_type() == PictureType::CoverFront);
                track.artwork = front
                    .or_else(|| pictures.first())
                    .and_then(|picture| crate::library::store_local_cover(picture.data()));
            }
        }
        track
    }
}

pub fn clock(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.) as u64
    } else {
        0
    };
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A real, playable editorial selection for the first launch; never represents a user's library.
pub fn rotation() -> Vec<Track> {
    serde_json::from_str(include_str!("../assets/rotation.json"))
        .expect("The bundled editorial selection must contain valid track metadata")
}
