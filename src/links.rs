//! Deliberately narrow app links; never interpret a URL as a command or file path.
use crate::catalog::validate_video_id;
use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Track(String),
    Playlist(String),
    Album(String),
    Artist(String),
}
impl Link {
    pub fn parse(raw: &str) -> Result<Self> {
        if raw.len() > 2048 || raw.chars().any(char::is_whitespace) {
            bail!("Invalid music link.");
        }
        let url = reqwest::Url::parse(raw)?;
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
        };
        format!("cassette://{kind}/{id}")
    }
}
