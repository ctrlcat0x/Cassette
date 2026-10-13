//! Deliberately narrow app links; never interpret a URL as a command or file path.
use crate::catalog::validate_video_id;
use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Track(String),
    Playlist(String),
    Album(String),
    Artist(String),
    Spotify { kind: String, id: String },
}
impl Link {
    pub fn parse(raw: &str) -> Result<Self> {
        if raw.len() > 2048 || raw.chars().any(char::is_whitespace) {
            bail!("Invalid music link.");
        }
        let owned;
        let raw = if let Some(rest) = raw.strip_prefix("spotify:") {
            let (kind, id) = rest
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("Invalid Spotify link"))?;
            owned = format!("https://open.spotify.com/{kind}/{id}");
            owned.as_str()
        } else {
            raw
        };
        let url = reqwest::Url::parse(raw)?;
        if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
            bail!("Unsupported music link.");
        }
        if url.scheme() == "https" {
            let host = url.host_str().unwrap_or_default();
            let parts: Vec<_> = url
                .path_segments()
                .into_iter()
                .flatten()
                .filter(|p| !p.is_empty())
                .collect();
            if host == "open.spotify.com" {
                let parts = if parts.first().is_some_and(|s| s.starts_with("intl-")) {
                    &parts[1..]
                } else {
                    &parts[..]
                };
                if let [kind @ ("track" | "album" | "artist" | "playlist"), id] = parts
                    && id.len() == 22
                    && id.bytes().all(|c| c.is_ascii_alphanumeric())
                {
                    return Ok(Self::Spotify {
                        kind: (*kind).into(),
                        id: (*id).into(),
                    });
                }
                bail!("Unsupported Spotify link.");
            }
            let query = |name: &str| {
                url.query_pairs()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.into_owned())
            };
            let native = if host == "youtu.be" && parts.len() == 1 {
                Some(("track", parts[0].to_owned()))
            } else if matches!(
                host,
                "youtube.com" | "www.youtube.com" | "music.youtube.com" | "m.youtube.com"
            ) {
                match parts.as_slice() {
                    ["watch"] => query("v").map(|id| ("track", id)),
                    ["playlist"] => query("list").map(|id| ("playlist", id)),
                    ["shorts" | "embed", id] => Some(("track", (*id).into())),
                    ["channel", id] => Some(("artist", (*id).into())),
                    ["browse", id] if id.starts_with("MPRE") => Some(("album", (*id).into())),
                    ["browse", id] if id.starts_with("UC") => Some(("artist", (*id).into())),
                    ["browse", id] if id.starts_with("VL") => {
                        Some(("playlist", id.trim_start_matches("VL").into()))
                    }
                    _ => None,
                }
            } else {
                None
            };
            if let Some((kind, id)) = native {
                return Self::parse(&format!("cassette://{kind}/{id}"));
            }
            bail!("Unsupported music link.");
        }
        if url.scheme() != "cassette"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!("Unsupported music link.");
        }
        let id = url.path().trim_start_matches('/');
        if id.is_empty()
            || id.len() > 200
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            bail!("Invalid music identifier.");
        }
        Ok(match url.host_str() {
            Some("track") => {
                validate_video_id(id)?;
                Self::Track(id.into())
            }
            Some("playlist") => {
                crate::account::playlist_id(id)?;
                Self::Playlist(id.into())
            }
            Some("album") if id.starts_with("MPRE") => Self::Album(id.into()),
            Some("artist") if id.starts_with("UC") && id.len() == 24 => Self::Artist(id.into()),
            _ => bail!("Unsupported music link."),
        })
    }
    pub fn url(&self) -> String {
        let (kind, id) = match self {
            Self::Track(id) => ("track", id),
            Self::Playlist(id) => ("playlist", id),
            Self::Album(id) => ("album", id),
            Self::Artist(id) => ("artist", id),
            Self::Spotify { kind, id } => return format!("https://open.spotify.com/{kind}/{id}"),
        };
        format!("cassette://{kind}/{id}")
    }
    pub fn public_url(&self) -> String {
        match self {
            Self::Track(id) => format!("https://music.youtube.com/watch?v={id}"),
            Self::Playlist(id) => format!("https://music.youtube.com/playlist?list={id}"),
            Self::Album(id) => format!("https://music.youtube.com/browse/{id}"),
            Self::Artist(id) => format!("https://music.youtube.com/channel/{id}"),
            Self::Spotify { .. } => self.url(),
        }
    }
}

pub struct ResolvedSpotify {
    pub name: String,
    pub tracks: Vec<crate::Track>,
    pub collection: Option<crate::Track>,
}
fn metadata_matches(track: &crate::Track, title: &str, artist: &str) -> bool {
    let normalized = |text: &str| {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    normalized(&track.title) == normalized(title)
        && (artist.is_empty()
            || artist.split([',', '&', ';']).any(|name| {
                let name = normalized(name);
                !name.is_empty()
                    && track
                        .artist
                        .split([',', '&', ';'])
                        .any(|candidate| normalized(candidate) == name)
            }))
}
pub fn resolve_spotify(kind: &str, id: &str) -> Result<ResolvedSpotify> {
    if !["track", "album", "artist", "playlist"].contains(&kind)
        || id.len() != 22
        || !id.bytes().all(|c| c.is_ascii_alphanumeric())
    {
        bail!("Invalid Spotify identifier");
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .get(format!("https://open.spotify.com/embed/{kind}/{id}"))
        .send()?
        .error_for_status()?;
    let html = String::from_utf8(crate::resource::read_limited(response, 4 * 1024 * 1024)?)?;
    let (_, json) = html.split_once("id=\"__NEXT_DATA__\"").ok_or_else(|| {
        anyhow::anyhow!(
            "Spotify did not return public metadata. Private links require public sharing."
        )
    })?;
    let (_, json) = json
        .split_once('>')
        .ok_or_else(|| anyhow::anyhow!("Invalid Spotify metadata"))?;
    let json = json.split("</script>").next().unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(json)?;
    let entity = &value["props"]["pageProps"]["state"]["data"]["entity"];
    let name = entity["name"]
        .as_str()
        .or_else(|| entity["title"].as_str())
        .ok_or_else(|| anyhow::anyhow!("Spotify metadata is unavailable for this link"))?
        .to_owned();
    let catalog = crate::catalog::Catalog::new(None)?;
    let mut tracks = Vec::new();
    let mut collection = None;
    if kind == "playlist" {
        let list = entity["trackList"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("This Spotify playlist has no public track list"))?;
        if list.len() >= 100 {
            bail!(
                "Spotify exposes only a playlist preview. Import a smaller public playlist to avoid silently losing tracks."
            );
        }
        for item in list {
            let title = item["title"].as_str().unwrap_or_default();
            let artist = item["subtitle"].as_str().unwrap_or_default();
            let page = catalog
                .search_filtered(&format!("{title} {artist}"), Some(crate::ItemKind::Song))?;
            let track = page
                .tracks
                .into_iter()
                .find(|t| t.kind == crate::ItemKind::Song && metadata_matches(t, title, artist))
                .ok_or_else(|| anyhow::anyhow!("No YouTube Music match for {title} — {artist}"))?;
            tracks.push(track);
        }
    } else {
        let artist = entity["artists"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|a| a["name"].as_str())
            .unwrap_or_default();
        let wanted = match kind {
            "album" => crate::ItemKind::Album,
            "artist" => crate::ItemKind::Artist,
            _ => crate::ItemKind::Song,
        };
        let page = catalog.search_filtered(&format!("{name} {artist}"), Some(wanted.clone()))?;
        let found = page
            .tracks
            .into_iter()
            .find(|t| t.kind == wanted && metadata_matches(t, &name, artist))
            .ok_or_else(|| anyhow::anyhow!("No matching item found on YouTube Music"))?;
        if kind == "track" {
            tracks.push(found);
        } else {
            collection = Some(found);
        }
    }
    Ok(ResolvedSpotify {
        name,
        tracks,
        collection,
    })
}
