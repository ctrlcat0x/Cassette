//! Authenticated account reads and explicit edits. No automatic write retries or local-library merging.
use crate::{
    ItemKind, Track,
    catalog::{Catalog, Page, parse_page, parse_renderer},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Section {
    Playlists,
    Liked,
    History,
    Albums,
    Artists,
    Songs,
}
impl Section {
    pub const ALL: [Self; 6] = [
        Self::Playlists,
        Self::Liked,
        Self::History,
        Self::Albums,
        Self::Artists,
        Self::Songs,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::Playlists => crate::i18n::t("Playlists"),
            Self::Liked => crate::i18n::t("Liked songs"),
            Self::History => crate::i18n::t("Recently played"),
            Self::Albums => crate::i18n::t("Albums"),
            Self::Artists => crate::i18n::t("Artists"),
            Self::Songs => crate::i18n::t("Saved songs"),
        }
    }
    pub fn browse_id(self) -> &'static str {
        match self {
            Self::Playlists => "FEmusic_liked_playlists",
            Self::Liked => "VLLM",
            Self::History => "FEmusic_history",
            Self::Albums => "FEmusic_liked_albums",
            Self::Artists => "FEmusic_library_corpus_artists",
            Self::Songs => "FEmusic_liked_videos",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Edit {
    Add {
        playlist: String,
        track: Track,
    },
    AddMany {
        playlist: String,
        tracks: Vec<Track>,
    },
    RemoveMany {
        playlist: String,
        tracks: Vec<Track>,
    },
    Remove {
        playlist: String,
        track: Track,
    },
    Rename {
        playlist: String,
        name: String,
    },
    Create {
        name: String,
    },
    Like {
        track: Track,
        liked: bool,
    },
    Delete {
        playlist: String,
    },
    Move {
        playlist: String,
        entry: String,
        before: Option<String>,
    },
    Description {
        playlist: String,
        text: String,
    },
    Privacy {
        playlist: String,
        value: String,
    },
    Follow {
        artist: String,
        followed: bool,
    },
    RateCollection {
        playlist: String,
        saved: bool,
    },
    Dislike {
        track: Track,
    },
    Feedback {
        token: String,
    },
    History {
        track: Track,
    },
}
impl Edit {
    pub fn playlist(&self) -> Option<&str> {
        match self {
            Self::Add { playlist, .. }
            | Self::AddMany { playlist, .. }
            | Self::RemoveMany { playlist, .. }
            | Self::Remove { playlist, .. }
            | Self::Rename { playlist, .. } => Some(playlist),
            Self::Delete { playlist }
            | Self::Move { playlist, .. }
            | Self::Description { playlist, .. }
            | Self::Privacy { playlist, .. } => Some(playlist),
            _ => None,
        }
    }
    /// Return a stable key so the UI cannot enqueue the same edit twice while awaiting its result.
    pub fn key(&self) -> String {
        match self {
            Self::Add { playlist, track } | Self::Remove { playlist, track } => format!(
                "playlist:{playlist}:{}",
                track.set_video_id.as_deref().unwrap_or(&track.id)
            ),
            Self::Rename { playlist, .. } => format!("rename:{playlist}"),
            Self::Create { .. } => "create".into(),
            Self::Like { track, .. } => format!("like:{}", track.id),
            _ => format!("edit:{}", crate::cache::key(&format!("{self:?}"))),
        }
    }
}

impl Catalog {
    pub fn account_section(&self, section: Section, continuation: Option<&str>) -> Result<Page> {
        self.require_session()?;
        let body = continuation.map_or_else(
            || json!({"browseId":section.browse_id()}),
            |token| json!({"continuation":token}),
        );
        let response = self.post("browse", body)?;
        check_account_response(&response)?;
        let mut page = account_page(&response, section.title(), section == Section::Liked);
        page.tracks.retain(|t| match section {
            Section::Playlists => t.kind == ItemKind::Playlist && t.id != "VLLM" && t.id != "LM",
            Section::Albums => t.kind == ItemKind::Album,
            Section::Artists => t.kind == ItemKind::Artist,
            _ => t.kind == ItemKind::Song,
        });
        Ok(page)
    }
    pub fn account_playlist(&self, id: &str, continuation: Option<&str>) -> Result<Page> {
        self.require_session()?;
        let id = playlist_id(id)?;
        let body = continuation.map_or_else(
            || json!({"browseId":format!("VL{id}")}),
            |token| json!({"continuation":token}),
        );
        let response = self.post("browse", body)?;
        check_account_response(&response)?;
        let mut page = account_page(&response, "Playlist", true);
        page.tracks.retain(|t| t.kind == ItemKind::Song);
        Ok(page)
    }
    pub fn edit_account(
        &self,
        edit: &Edit,
        still_current: impl Fn() -> bool,
    ) -> Result<Option<String>> {
        self.require_session()?;
        if crate::cache::offline_only() {
            bail!("Account changes are unavailable in offline mode.");
        }
        if let Edit::History { track } = edit {
            self.record_history(&track.id, still_current)?;
            return Ok(None);
        }
        if let Some(id) = edit.playlist() {
            // Never infer ownership from the display name or presence in the user's library.
            if !self.account_playlist(id, None)?.editable {
                bail!("This playlist is read-only. YouTube Music did not grant edit access.");
            }
        }
        if !still_current() {
            bail!("The account changed before the edit could be sent.");
        }
        let (endpoint, body) = edit_request(edit)?;
        let response = self.post(endpoint, body)?;
        check_account_response(&response)?;
        if matches!(edit, Edit::Create { .. }) {
            if response.get("error").is_some() {
                bail!("YouTube Music rejected the playlist creation.");
            }
            if response.get("status").is_some() {
                check_edit_response(&response, false)?;
            }
            return Ok(Some(
                playlist_id(
                    response["playlistId"]
                        .as_str()
                        .context("YouTube did not return the created playlist ID")?,
                )?
                .to_owned(),
            ));
        }
        check_edit_response(
            &response,
            matches!(
                edit,
                Edit::Like { .. }
                    | Edit::Follow { .. }
                    | Edit::RateCollection { .. }
                    | Edit::Dislike { .. }
                    | Edit::Feedback { .. }
            ),
        )?;
        Ok(None)
    }
}

pub fn playlist_id(id: &str) -> Result<&str> {
    let id = id.strip_prefix("VL").unwrap_or(id);
    if id.is_empty()
        || id.len() > 200
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        || id.starts_with("local-")
    {
        bail!("Invalid YouTube playlist ID");
    }
    Ok(id)
}
fn edit_request(edit: &Edit) -> Result<(&'static str, Value)> {
    let name = |name: &str| -> Result<String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 || name.contains(['<', '>']) {
            bail!("Enter a playlist name between 1 and 100 characters without < or >.");
        }
        Ok(name.into())
    };
    Ok(match edit {
        Edit::History { .. } => bail!("History uses its playback tracking endpoint."),
        Edit::AddMany { playlist, tracks } | Edit::RemoveMany { playlist, tracks } => {
            if tracks.is_empty() || tracks.len() > 100 {
                bail!("A playlist edit must contain 1 to 100 songs.");
            }
            let mut actions = Vec::with_capacity(tracks.len());
            for track in tracks {
                crate::catalog::validate_video_id(&track.id)?;
                if matches!(edit, Edit::AddMany { .. }) {
                    actions.push(json!({"action":"ACTION_ADD_VIDEO","addedVideoId":track.id}));
                } else {
                    let entry = track
                        .set_video_id
                        .as_deref()
                        .filter(|id| {
                            !id.is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control)
                        })
                        .context(
                            "Refresh this playlist before removing entries with missing IDs.",
                        )?;
                    actions.push(json!({"action":"ACTION_REMOVE_VIDEO","setVideoId":entry,"removedVideoId":track.id}));
                }
            }
            (
                "browse/edit_playlist",
                json!({"playlistId":playlist_id(playlist)?,"actions":actions}),
            )
        }
        Edit::Delete { playlist } => (
            "playlist/delete",
            json!({"playlistId":playlist_id(playlist)?}),
        ),
        Edit::Move {
            playlist,
            entry,
            before,
        } => {
            let valid =
                |id: &str| !id.is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control);
            if !valid(entry) || before.as_ref().is_some_and(|id| !valid(id) || id == entry) {
                bail!("Invalid playlist occurrence.");
            }
            let mut action = json!({"action":"ACTION_MOVE_VIDEO_BEFORE", "setVideoId":entry});
            if let Some(before) = before {
                action["movedSetVideoIdSuccessor"] = json!(before);
            }
            (
                "browse/edit_playlist",
                json!({"playlistId":playlist_id(playlist)?,"actions":[action]}),
            )
        }
        Edit::Description { playlist, text } => {
            if text.chars().count() > 5000 || text.contains(['<', '>']) {
                bail!("Description must be at most 5,000 characters without < or >.");
            }
            (
                "browse/edit_playlist",
                json!({"playlistId":playlist_id(playlist)?,"actions":[{"action":"ACTION_SET_PLAYLIST_DESCRIPTION","playlistDescription":text}]}),
            )
        }
        Edit::Privacy { playlist, value } => {
            if !matches!(value.as_str(), "PRIVATE" | "PUBLIC" | "UNLISTED") {
                bail!("Invalid playlist visibility.");
            }
            (
                "browse/edit_playlist",
                json!({"playlistId":playlist_id(playlist)?,"actions":[{"action":"ACTION_SET_PLAYLIST_PRIVACY","playlistPrivacy":value}]}),
            )
        }
        Edit::Follow { artist, followed } => {
            if !artist.starts_with("UC")
                || artist.len() != 24
                || !artist
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                bail!("Invalid artist channel.");
            }
            (
                if *followed {
                    "subscription/subscribe"
                } else {
                    "subscription/unsubscribe"
                },
                json!({"channelIds":[artist]}),
            )
        }
        Edit::RateCollection { playlist, saved } => (
            if *saved {
                "like/like"
            } else {
                "like/removelike"
            },
            json!({"target":{"playlistId":playlist_id(playlist)?}}),
        ),
        Edit::Dislike { track } => {
            crate::catalog::validate_video_id(&track.id)?;
            ("like/dislike", json!({"target":{"videoId":track.id}}))
        }
        Edit::Feedback { token } => {
            if token.is_empty() || token.len() > 16384 || token.chars().any(char::is_control) {
                bail!("Missing or invalid library feedback token. Refresh this collection.");
            }
            ("feedback", json!({"feedbackTokens":[token]}))
        }
        Edit::Create { name: title } => (
            "playlist/create",
            json!({"title":name(title)?,"description":"","privacyStatus":"PRIVATE"}),
        ),
        Edit::Like { track, liked } => {
            crate::catalog::validate_video_id(&track.id)?;
            (
                if *liked {
                    "like/like"
                } else {
                    "like/removelike"
                },
                json!({"target":{"videoId":track.id}}),
            )
        }
        Edit::Add { playlist, track } => {
            crate::catalog::validate_video_id(&track.id)?;
            (
                "browse/edit_playlist",
                json!({"playlistId":playlist_id(playlist)?,"actions":[{"action":"ACTION_ADD_VIDEO","addedVideoId":track.id}]}),
            )
        }
        Edit::Remove { playlist, track } => {
            crate::catalog::validate_video_id(&track.id)?;
            let entry = track.set_video_id.as_deref().filter(|id| !id.is_empty() && id.len() <= 1024).context("This song is missing its playlist entry ID. Refresh the playlist before removing it.")?;
            (
                "browse/edit_playlist",
                json!({"playlistId":playlist_id(playlist)?,"actions":[{"action":"ACTION_REMOVE_VIDEO","setVideoId":entry,"removedVideoId":track.id}]}),
            )
        }
        Edit::Rename {
            playlist,
            name: title,
        } => (
            "browse/edit_playlist",
            json!({"playlistId":playlist_id(playlist)?,"actions":[{"action":"ACTION_SET_PLAYLIST_NAME","playlistName":name(title)?}]}),
        ),
    })
}
fn check_account_response(response: &Value) -> Result<()> {
    if find(response, "loggedOut").and_then(Value::as_bool) == Some(true) {
        bail!("Your YouTube Music session expired. Sign in again to refresh your account library.");
    }
    Ok(())
}
fn check_edit_response(response: &Value, like: bool) -> Result<()> {
    if response.get("error").is_some() {
        bail!("YouTube Music rejected the edit.");
    }
    match response.get("status").and_then(Value::as_str) {
        Some("STATUS_SUCCEEDED") => Ok(()),
        None if like && response.get("responseContext").is_some() => Ok(()),
        _ => bail!(
            "YouTube Music did not confirm the edit. Refresh before retrying; the result may be uncertain."
        ),
    }
}
fn find<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(o) => o.get(key).or_else(|| o.values().find_map(|v| find(v, key))),
        Value::Array(a) => a.iter().find_map(|v| find(v, key)),
        _ => None,
    }
}
fn account_page(response: &Value, title: &str, playlist: bool) -> Page {
    let mut page = parse_page(response, title);
    // Playlist shelf entries have occurrence IDs. Preserve duplicates and exclude suggestions.
    if playlist {
        let shelf = find(response, "musicPlaylistShelfRenderer")
            .or_else(|| find(response, "musicPlaylistShelfContinuation"));
        if let Some(shelf) = shelf {
            let mut tracks = Vec::new();
            fn rows(value: &Value, tracks: &mut Vec<Track>) {
                match value {
                    Value::Object(o) => {
                        for (key, v) in o {
                            if key == "musicResponsiveListItemRenderer" {
                                if let Some(track) = parse_renderer(v) {
                                    tracks.push(track);
                                }
                            } else {
                                rows(v, tracks);
                            }
                        }
                    }
                    Value::Array(a) => {
                        for v in a {
                            rows(v, tracks);
                        }
                    }
                    _ => {}
                }
            }
            rows(shelf, &mut tracks);
            page.tracks = tracks;
            // A playlist continuation belongs to its song shelf, not a suggested-playlist shelf.
            page.continuation = parse_page(shelf, title).continuation;
        } else if let Some(items) =
            find(response, "appendContinuationItemsAction").and_then(|v| v.get("continuationItems"))
        {
            // Current WEB_REMIX playlist continuations use actions rather than a shelf wrapper.
            let wrapper = json!({"musicPlaylistShelfRenderer":{"contents":items}});
            let continuation = account_page(&wrapper, title, true);
            page.tracks = continuation.tracks;
            page.continuation = continuation.continuation;
        }
    }
    page.sections.clear();
    if page.tracks.len() >= 10_000 {
        page.tracks.truncate(10_000);
        page.continuation = None;
    }
    page
}
