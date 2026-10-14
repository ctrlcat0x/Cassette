//! Raag's independent InnerTube catalog adapter. No reference-project code is included.
use crate::auth::{Account, ActiveSession, CookieJar, SessionMaterial, SignInFailure};
use crate::{ItemKind, Track};
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::sync::{Arc, Mutex, OnceLock};
use std::{
    collections::HashSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Page {
    pub visitor_data: Option<String>,
    pub playlist_id: Option<String>,
    pub subscribed: Option<bool>,
    pub editable: bool,
    pub title: String,
    pub tracks: Vec<Track>,
    pub continuation: Option<String>,
    pub artwork: Option<String>,
    pub subtitle: String,
    pub description: String,
    pub privacy: Option<String>,
    pub sections: Vec<Section>,
    pub destinations: Vec<Destination>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Section {
    pub title: String,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Destination {
    pub title: String,
    pub id: String,
    pub params: String,
}

pub struct Catalog {
    http: Client,
    version: String,
    cookies: Option<Mutex<CookieJar>>,
    session: Option<Arc<ActiveSession>>,
    visitor_data: Mutex<String>,
    data_sync_id: String,
}

impl Catalog {
    pub fn new(cookies: Option<CookieJar>) -> Result<Self> {
        // Headers and cookies belong to individual requests; only transport is shared.
        // Clones reuse both the connection pool and blocking client's runtime thread.
        static HTTP: OnceLock<Result<Client, reqwest::Error>> = OnceLock::new();
        let http = HTTP
            .get_or_init(|| {
                Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(15))
                    .connect_timeout(Duration::from_secs(5))
                    .pool_max_idle_per_host(2)
                    .pool_idle_timeout(Duration::from_secs(30))
                    .user_agent("Mozilla/5.0")
                    .build()
            })
            .as_ref()
            .map_err(|error| anyhow::anyhow!("Catalog transport: {error}"))?
            .clone();
        Ok(Self {
            http,
            version: "1.20260930.03.00".into(),
            cookies: cookies.map(Mutex::new),
            session: None,
            visitor_data: Mutex::new(String::new()),
            data_sync_id: String::new(),
        })
    }

    pub fn set_session(&mut self, session: Option<Arc<ActiveSession>>) {
        self.session = session;
    }

    pub fn validate(
        cookies: CookieJar,
        previous: Option<&SessionMaterial>,
    ) -> Result<SessionMaterial> {
        let mut catalog = Self::new(Some(cookies))?;
        catalog.bootstrap();
        if let Some(previous) = previous {
            *catalog.visitor_data.get_mut().unwrap() = previous.visitor_data.clone();
            catalog.data_sync_id = previous.data_sync_id.clone();
        }
        let response = catalog.post("account/account_menu", json!({}))?;
        account_material(
            catalog.cookies.unwrap().into_inner().unwrap(),
            &response,
            previous,
        )
    }

    pub fn bootstrap(&mut self) {
        if crate::cache::offline_only() {
            return;
        }
        // Anonymous workers share one shell fetch. Session material never enters this cache.
        type Shell = Option<(std::time::Instant, String, String)>;
        static SHELL: OnceLock<Mutex<Shell>> = OnceLock::new();
        let mut shell = SHELL
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((at, version, visitor)) = shell.as_ref()
            && at.elapsed() < Duration::from_secs(3600)
        {
            self.version = version.clone();
            *self.visitor_data.get_mut().unwrap() = visitor.clone();
            return;
        }
        if catalog_policy().before().is_err() {
            return;
        }
        let response = self
            .http
            .get("https://music.youtube.com")
            .timeout(Duration::from_secs(3))
            .send();
        let Ok(response) = response else {
            return;
        };
        catalog_policy().observe(response.status(), response.headers());
        if response.status().is_success()
            && let Ok(bytes) = crate::resource::read_limited(response, 2 * 1024 * 1024)
            && let Ok(html) = String::from_utf8(bytes)
            && let Some(version) = config_value(&html, "INNERTUBE_CLIENT_VERSION")
        {
            self.version = version;
            *self.visitor_data.get_mut().unwrap() =
                config_value(&html, "VISITOR_DATA").unwrap_or_default();
            *shell = Some((
                std::time::Instant::now(),
                self.version.clone(),
                self.visitor_data.get_mut().unwrap().clone(),
            ));
        }
    }

    fn request(
        &self,
        endpoint: &str,
        mut body: Value,
        authenticated: bool,
    ) -> Result<reqwest::blocking::RequestBuilder> {
        body["context"] = json!({"client": {"clientName": "WEB_REMIX", "clientVersion": self.version, "hl": "en", "gl": "IN"}});
        let material = authenticated
            .then(|| self.session.as_ref().map(|session| session.material()))
            .flatten();
        let jar = material.as_ref().map(|m| m.cookies.clone()).or_else(|| {
            authenticated
                .then(|| self.cookies.as_ref().map(|j| j.lock().unwrap().clone()))
                .flatten()
        });
        let guest_visitor = self.visitor_data.lock().unwrap().clone();
        let visitor = material
            .as_ref()
            .map_or(guest_visitor.as_str(), |m| &m.visitor_data);
        let identity = material
            .as_ref()
            .map_or(self.data_sync_id.as_str(), |m| &m.data_sync_id);
        if !visitor.is_empty() {
            body["context"]["client"]["visitorData"] = json!(visitor);
        }
        body["context"]["request"] = json!({"useSsl": true, "internalExperimentFlags": []});
        body["context"]["user"] = json!({"lockedSafetyMode": false});
        if jar.is_some() && !identity.is_empty() {
            body["context"]["user"]["onBehalfOfUser"] = json!(identity);
        }
        let mut request = self
            .http
            .post(format!(
                "https://music.youtube.com/youtubei/v1/{endpoint}?prettyPrint=false"
            ))
            .header("Origin", "https://music.youtube.com")
            .header("X-Origin", "https://music.youtube.com")
            .header("Referer", "https://music.youtube.com/")
            .header("X-Youtube-Client-Name", "67")
            .header("Accept-Language", "en-IN,en;q=0.9")
            .header("X-Youtube-Client-Version", &self.version);
        if !visitor.is_empty() {
            request = request.header("X-Goog-Visitor-Id", visitor);
        }
        if let Some(jar) = &jar {
            let header = jar.header();
            request = request
                .header("Cookie", header.as_str())
                .header("X-Goog-AuthUser", "0");
            if let Some(sapisid) = jar.auth_value() {
                let epoch = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                let hash =
                    Sha1::digest(format!("{epoch} {sapisid} https://music.youtube.com").as_bytes());
                request = request.header("Authorization", format!("SAPISIDHASH {epoch}_{hash:x}"));
            }
        }
        Ok(request.json(&body))
    }

    pub(crate) fn post(&self, endpoint: &str, body: Value) -> Result<Value> {
        self.post_with_auth(endpoint, body, true)
    }

    fn post_with_auth(&self, endpoint: &str, body: Value, authenticated: bool) -> Result<Value> {
        if crate::cache::offline_only() {
            bail!("This request is unavailable in offline mode.");
        }
        let _domain = crate::diagnostics::enter(crate::diagnostics::Domain::Catalog);
        // Only read operations are safe to repeat after an ambiguous network failure.
        let retryable = matches!(
            endpoint,
            "browse" | "search" | "next" | "music/get_search_suggestions" | "account/account_menu"
        );
        let mut attempt = 0;
        let response = loop {
            catalog_policy().before()?;
            let result = self.request(endpoint, body.clone(), authenticated)?.send();
            if let Ok(response) = &result {
                catalog_policy().observe(response.status(), response.headers());
            }
            let transient = match &result {
                Ok(response) => matches!(response.status().as_u16(), 502..=504),
                Err(error) => error.is_connect() || error.is_timeout(),
            };
            if retryable && transient && attempt == 0 {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(400));
                continue;
            }
            break result.map_err(|e| {
                if e.is_timeout() {
                    SignInFailure::Timeout
                } else {
                    SignInFailure::Network
                }
            })?;
        };
        if !response.status().is_success() {
            return Err(SignInFailure::Http(response.status().as_u16()).into());
        }
        if authenticated && let Some(session) = &self.session {
            session.absorb(response.headers())?;
        } else if authenticated && let Some(jar) = &self.cookies {
            let mut jar = jar.lock().unwrap();
            for value in response.headers().get_all(reqwest::header::SET_COOKIE) {
                if let Ok(value) = value.to_str() {
                    jar.absorb(value);
                }
            }
        }
        let value: Value = crate::resource::json(response, crate::resource::PAGE_BYTES)
            .map_err(|_| SignInFailure::InvalidResponse)?;
        if let Some(error) = value.get("error") {
            if error["code"].as_u64() == Some(429) {
                catalog_policy().pause(Duration::from_secs(60));
            }
            return Err(SignInFailure::Http(
                error["code"]
                    .as_u64()
                    .and_then(|c| u16::try_from(c).ok())
                    .unwrap_or(400),
            )
            .into());
        }
        if (!authenticated || (self.session.is_none() && self.cookies.is_none()))
            && let Some(visitor) = value["responseContext"]["visitorData"]
                .as_str()
                .filter(|v| !v.is_empty())
        {
            *self.visitor_data.lock().unwrap() = visitor.into();
        }
        Ok(value)
    }

    pub fn home(&self) -> Result<Page> {
        self.browse("FEmusic_home", "Made for you")
    }
    pub(crate) fn record_history(
        &self,
        video: &str,
        still_current: impl Fn() -> bool,
    ) -> Result<()> {
        self.require_session()?;
        validate_video_id(video)?;
        let response = self.post("player", json!({"videoId":video,"isAudioOnly":true}))?;
        let mut url = history_url(&response)?;
        let cpn: String = (0..16)
            .map(|_| {
                b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
                    [rand::random::<u8>() as usize & 63] as char
            })
            .collect();
        url.query_pairs_mut()
            .append_pair("ver", "2")
            .append_pair("c", "WEB_REMIX")
            .append_pair("cpn", &cpn);
        if !still_current() || crate::cache::offline_only() {
            bail!("Listening history sync cancelled.");
        }
        let mut request = self.http.get(url).header("Origin", crate::auth::ORIGIN);
        if let Some(session) = &self.session {
            request = request.header("Cookie", session.material().cookies.header().as_str());
        }
        let response = request
            .send()
            .map_err(|_| anyhow::anyhow!("Could not sync listening history."))?;
        if response.status().as_u16() != 204 {
            bail!("YouTube Music did not confirm listening history sync.");
        }
        Ok(())
    }
    pub(crate) fn require_session(&self) -> Result<()> {
        if self.session.is_none() && self.cookies.is_none() {
            bail!("Sign in to YouTube Music to access your account library.");
        }
        Ok(())
    }
    pub fn explore(&self) -> Result<Page> {
        self.browse("FEmusic_explore", "Explore")
    }
    pub fn browse(&self, id: &str, title: &str) -> Result<Page> {
        self.discover(id, "", title)
    }
    pub fn discover(&self, id: &str, params: &str, title: &str) -> Result<Page> {
        let mut body = json!({"browseId": id});
        if !params.is_empty() {
            body["params"] = json!(params);
        }
        let value = self.post("browse", body)?;
        let mut page = parse_page(&value, title);
        if id.starts_with("MPRE") {
            for track in page
                .tracks
                .iter_mut()
                .chain(page.sections.iter_mut().flat_map(|s| &mut s.tracks))
            {
                if track.kind == ItemKind::Song && track.album.is_empty() {
                    track.album = page.title.clone();
                }
            }
        }
        Ok(page)
    }
    pub fn search(&self, query: &str) -> Result<Page> {
        self.search_filtered(query, None)
    }
    pub fn suggestions(&self, query: &str) -> Result<Vec<String>> {
        if query.trim().chars().count() < 2 || query.len() > 512 {
            return Ok(vec![]);
        }
        let value = self.post_with_auth(
            "music/get_search_suggestions",
            json!({"input":query.trim()}),
            false,
        )?;
        Ok(parse_suggestions(&value))
    }
    pub fn search_filtered(&self, query: &str, filter: Option<ItemKind>) -> Result<Page> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Page::default());
        }
        let mut body = json!({"query":query});
        let params = match filter {
            Some(ItemKind::Song) => Some("EgWKAQIIAWoMEA4QChADEAQQCRAF"),
            Some(ItemKind::Album) => Some("EgWKAQIYAWoMEA4QChADEAQQCRAF"),
            Some(ItemKind::Artist) => Some("EgWKAQIgAWoMEA4QChADEAQQCRAF"),
            Some(ItemKind::Playlist) => Some("EgWKAQIoAWoMEA4QChADEAQQCRAF"),
            _ => None,
        };
        if let Some(params) = params {
            body["params"] = json!(params);
        }
        let value = self.post("search", body)?;
        Ok(parse_page(&value, &format!("Results for “{query}”")))
    }
    pub fn more(&self, token: &str, search: bool) -> Result<Page> {
        self.more_with_visitor(token, search, None)
    }
    /// Continuations belong to the visitor that fetched the first page, including disk hits.
    pub fn more_with_visitor(
        &self,
        token: &str,
        search: bool,
        visitor: Option<&str>,
    ) -> Result<Page> {
        if let Some(visitor) = visitor.filter(|v| !v.is_empty()) {
            *self.visitor_data.lock().unwrap() = visitor.into();
        }
        let value = self.post(
            if search { "search" } else { "browse" },
            json!({"continuation": token}),
        )?;
        Ok(parse_page(&value, "More music"))
    }
    pub fn radio(&self, id: &str) -> Result<Page> {
        validate_video_id(id)?;
        let value = self.post(
            "next",
            json!({"videoId": id, "playlistId": format!("RDAMVM{id}"), "isAudioOnly": true}),
        )?;
        Ok(parse_page(&value, "Song radio"))
    }
    pub fn artist(&self, name: &str) -> Result<Page> {
        let page = self.search_filtered(name, Some(ItemKind::Artist))?;
        let artist = page
            .tracks
            .iter()
            .find(|track| track.kind == ItemKind::Artist && track.title.eq_ignore_ascii_case(name))
            .or_else(|| {
                page.tracks
                    .iter()
                    .find(|track| track.kind == ItemKind::Artist)
            })
            .context("Artist could not be found")?;
        self.browse(&artist.id, &artist.title)
    }
    pub fn lyrics(&self, track: &Track) -> Result<Vec<LyricLine>> {
        Ok(crate::lyrics::fetch(track)
            .or_else(|_| self.youtube_lyrics(track))?
            .lines)
    }

    /// Follow the exact song's lyrics tab, never a similarly named search result.
    pub fn youtube_lyrics(&self, track: &Track) -> Result<crate::lyrics::Lyrics> {
        validate_video_id(&track.id)?;
        let next = self.post("next", json!({"videoId": track.id}))?;
        let id = lyrics_browse_id(&next).context("No YouTube Music lyrics tab")?;
        let response = self.post("browse", json!({"browseId": id}))?;
        parse_youtube_lyrics(&response).context("No YouTube Music lyrics available")
    }
}

fn lyrics_browse_id(value: &Value) -> Option<&str> {
    match value {
        Value::Object(object) => {
            if let Some(tab) = object.get("tabRenderer")
                && tab["unselectable"].as_bool() != Some(true)
                && let Some(id) = tab
                    .pointer("/endpoint/browseEndpoint/browseId")
                    .and_then(Value::as_str)
                && id.starts_with("MPLY")
            {
                return Some(id);
            }
            object.values().find_map(lyrics_browse_id)
        }
        Value::Array(items) => items.iter().find_map(lyrics_browse_id),
        _ => None,
    }
}

fn parse_youtube_lyrics(value: &Value) -> Option<crate::lyrics::Lyrics> {
    match value {
        Value::Object(object) => {
            if let Some(shelf) = object.get("musicDescriptionShelfRenderer") {
                let description = text(&shelf["description"]);
                let lines: Vec<_> = description
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(|line| LyricLine {
                        seconds: None,
                        text: line.to_owned(),
                        ..Default::default()
                    })
                    .collect();
                if !lines.is_empty() {
                    let attribution = text(&shelf["footer"]);
                    return Some(crate::lyrics::Lyrics {
                        lines,
                        provider: if attribution.is_empty() {
                            "YouTube Music".into()
                        } else {
                            format!("YouTube Music · {attribution}")
                        },
                        instrumental: false,
                    });
                }
            }
            object.values().find_map(parse_youtube_lyrics)
        }
        Value::Array(items) => items.iter().find_map(parse_youtube_lyrics),
        _ => None,
    }
}

fn catalog_policy() -> &'static crate::request_policy::RequestPolicy {
    crate::request_policy::youtube()
}

pub fn validate_video_id(id: &str) -> Result<()> {
    if id.len() != 11
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        bail!("Invalid YouTube video identifier.");
    }
    Ok(())
}

pub fn config_value(html: &str, key: &str) -> Option<String> {
    let tail = html.split_once(&format!("\"{key}\""))?.1;
    let tail = tail.trim_start().strip_prefix(':')?.trim_start();
    let mut stream = serde_json::Deserializer::from_str(tail).into_iter::<String>();
    stream.next()?.ok()
}

pub fn text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.into();
    }
    if let Some(text) = value["simpleText"].as_str() {
        return text.into();
    }
    value["runs"]
        .as_array()
        .map(|runs| runs.iter().filter_map(|r| r["text"].as_str()).collect())
        .unwrap_or_default()
}

fn parse_account(value: &Value) -> Result<(Account, String, String)> {
    if value["responseContext"]["mainAppWebResponseContext"]["loggedOut"].as_bool() == Some(true) {
        return Err(SignInFailure::AccountNotConfirmed.into());
    }
    let header =
        find(value, "activeAccountHeaderRenderer").ok_or(SignInFailure::AccountNotConfirmed)?;
    let name = text(&header["accountName"]);
    if name.trim().is_empty() {
        return Err(SignInFailure::IncompleteAccount.into());
    }
    let handle = text(&header["channelHandle"]);
    fn channel(value: &Value) -> Option<String> {
        if let Some(id) = value
            .get("browseId")
            .and_then(Value::as_str)
            .filter(|id| id.starts_with("UC"))
        {
            return Some(id.into());
        }
        match value {
            Value::Object(o) => o.values().find_map(channel),
            Value::Array(a) => a.iter().find_map(channel),
            _ => None,
        }
    }
    let context = &value["responseContext"];
    let raw = context["mainAppWebResponseContext"]["datasyncId"]
        .as_str()
        .or_else(|| context["mainAppWebResponseContext"]["dataSyncId"].as_str())
        .unwrap_or("");
    // A primary user ID is not a delegated channel ID. Only brand-account responses
    // contain both parts of DELEGATED_SESSION_ID||USER_SESSION_ID.
    let data_sync_id = raw
        .split_once("||")
        .filter(|(delegated, user)| !delegated.is_empty() && !user.is_empty())
        .map_or_else(String::new, |(delegated, _)| delegated.to_owned());
    let channel_id = channel(header)
        .or_else(|| (!raw.trim_matches('|').is_empty()).then(|| crate::cache::key(raw)))
        .unwrap_or_default();
    let visitor_data = context["visitorData"].as_str().unwrap_or("").to_owned();
    Ok((
        Account {
            name,
            handle,
            channel_id,
            avatar_url: account_avatar(header),
        },
        visitor_data,
        data_sync_id,
    ))
}

fn account_avatar(header: &Value) -> Option<String> {
    ["accountPhoto", "accountThumbnail"]
        .into_iter()
        .filter_map(|field| find(&header[field], "thumbnails").and_then(Value::as_array))
        .flatten()
        .filter_map(|image| {
            let raw = image["url"].as_str()?;
            let url = if raw.starts_with("//") {
                format!("https:{raw}")
            } else {
                raw.to_owned()
            };
            let parsed = reqwest::Url::parse(&url).ok()?;
            (parsed.scheme() == "https"
                && matches!(
                    parsed.host_str(),
                    Some(
                        "yt3.ggpht.com"
                            | "yt3.googleusercontent.com"
                            | "lh3.ggpht.com"
                            | "lh3.googleusercontent.com"
                    )
                ))
            .then_some((image["width"].as_u64().unwrap_or(0), url))
        })
        .max_by_key(|(width, _)| *width)
        .map(|(_, url)| url)
}

fn account_material(
    cookies: CookieJar,
    response: &Value,
    previous: Option<&SessionMaterial>,
) -> Result<SessionMaterial> {
    let (mut account, visitor_data, data_sync_id) = parse_account(response)?;
    // Optional metadata must not turn a confirmed login into an error. Keep caches isolated
    // using a session-derived key, never a display name shared by unrelated accounts.
    if account.channel_id.is_empty() {
        account.channel_id = previous
            .filter(|p| p.account.name == account.name && p.account.handle == account.handle)
            .map(|p| p.account.channel_id.clone())
            .unwrap_or_else(|| {
                format!(
                    "session:{}",
                    crate::cache::key(cookies.auth_value().unwrap_or_default())
                )
            });
    }
    Ok(SessionMaterial {
        cookies,
        account,
        visitor_data,
        data_sync_id,
    })
}

fn find<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(object) => object
            .get(key)
            .or_else(|| object.values().find_map(|child| find(child, key))),
        Value::Array(array) => array.iter().find_map(|child| find(child, key)),
        _ => None,
    }
}

fn duration(text: &str) -> u64 {
    let pieces: Vec<_> = text.split(':').collect();
    if !(2..=3).contains(&pieces.len()) {
        return 0;
    }
    pieces
        .into_iter()
        .try_fold(0u64, |total, value| {
            value
                .parse::<u64>()
                .ok()
                .and_then(|value| total.checked_mul(60)?.checked_add(value))
        })
        .unwrap_or(0)
}

pub(crate) fn parse_renderer(value: &Value) -> Option<Track> {
    // Only the item's primary navigation identifies it. An artist's menu may
    // contain a "play radio" watchEndpoint; that does not make the artist a song.
    let primary = value.get("navigationEndpoint")
        .or_else(|| value.pointer("/title/runs/0/navigationEndpoint"))
        .or_else(|| value.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint"));
    let video = value
        .pointer("/playlistItemData/videoId")
        .and_then(Value::as_str)
        .or_else(|| {
            primary
                .and_then(|endpoint| endpoint.pointer("/watchEndpoint/videoId"))
                .and_then(Value::as_str)
        })
        .or_else(|| value.get("videoId").and_then(Value::as_str));
    let browse = primary.and_then(|endpoint| endpoint.get("browseEndpoint"));
    let id = video.or_else(|| browse.and_then(|browse| browse["browseId"].as_str()))?;
    let mut title = text(&value["title"]);
    let mut subtitle = text(&value["subtitle"]);
    if subtitle.is_empty() {
        subtitle = text(&value["longBylineText"]);
    }
    if subtitle.is_empty() {
        subtitle = text(&value["shortBylineText"]);
    }
    if let Some(columns) = value["flexColumns"].as_array() {
        if title.is_empty() {
            title = columns
                .first()
                .map(|c| text(&c["musicResponsiveListItemFlexColumnRenderer"]["text"]))
                .unwrap_or_default();
        }
        if subtitle.is_empty() {
            subtitle = columns
                .get(1)
                .map(|c| text(&c["musicResponsiveListItemFlexColumnRenderer"]["text"]))
                .unwrap_or_default();
        }
    }
    if title.is_empty() {
        return None;
    }
    let kind = if video.is_some() {
        ItemKind::Song
    } else if id.starts_with("MPRE") {
        ItemKind::Album
    } else if id.starts_with("UC") {
        ItemKind::Artist
    } else {
        ItemKind::Playlist
    };
    let artwork = find(&value["thumbnail"], "thumbnails")
        .or_else(|| find(&value["thumbnailRenderer"], "thumbnails"))
        .and_then(Value::as_array)
        .and_then(|images| images.last())
        .and_then(|image| image["url"].as_str())
        .map(crate::artwork::normalize_url);
    let length = text(&value["lengthText"]);
    let mut seconds = duration(&length);
    if seconds == 0
        && let Some(columns) = value["fixedColumns"].as_array()
    {
        seconds = columns
            .iter()
            .map(|column| {
                duration(&text(
                    &column["musicResponsiveListItemFixedColumnRenderer"]["text"],
                ))
            })
            .find(|seconds| *seconds > 0)
            .unwrap_or(0);
    }
    if seconds == 0 {
        seconds = subtitle
            .split('•')
            .map(|part| duration(part.trim()))
            .find(|seconds| *seconds > 0)
            .unwrap_or(0);
    }
    let artist = subtitle
        .split('•')
        .map(str::trim)
        .find(|part| {
            !matches!(
                *part,
                "Song" | "Video" | "Album" | "Single" | "EP" | "Playlist" | "Artist"
            ) && !part.is_empty()
        })
        .unwrap_or("")
        .to_owned();
    let mut album = String::new();
    fn album_run(value: &Value) -> Option<String> {
        value.get("runs")?.as_array()?.iter().find_map(|run| {
            let id = run
                .pointer("/navigationEndpoint/browseEndpoint/browseId")?
                .as_str()?;
            id.starts_with("MPRE")
                .then(|| run["text"].as_str().unwrap_or_default().to_owned())
        })
    }
    let mut artists = Vec::new();
    fn artist_runs(value: &Value, artists: &mut Vec<crate::ArtistRef>) {
        if let Some(runs) = value.get("runs").and_then(Value::as_array) {
            for run in runs {
                if let Some(id) = run["navigationEndpoint"]["browseEndpoint"]["browseId"]
                    .as_str()
                    .filter(|id| id.starts_with("UC"))
                    && let Some(name) = run["text"].as_str()
                    && !artists.iter().any(|a| a.id == id)
                {
                    artists.push(crate::ArtistRef {
                        id: id.into(),
                        name: name.into(),
                    });
                }
            }
        }
    }
    artist_runs(&value["subtitle"], &mut artists);
    artist_runs(&value["longBylineText"], &mut artists);
    artist_runs(&value["shortBylineText"], &mut artists);
    if let Some(name) =
        album_run(&value["subtitle"]).or_else(|| album_run(&value["longBylineText"]))
    {
        album = name;
    }
    if let Some(columns) = value["flexColumns"].as_array() {
        for column in columns.iter().skip(1) {
            let text = &column["musicResponsiveListItemFlexColumnRenderer"]["text"];
            artist_runs(text, &mut artists);
            if let Some(name) = album_run(text) {
                album = name;
            }
        }
    }
    let artist = if artists.is_empty() {
        artist
    } else {
        artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(" & ")
    };
    let (library_add, library_remove) = library_tokens(value);
    Some(Track {
        explicit: Some(
            value["badges"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|badge| {
                    find(badge, "iconType").and_then(Value::as_str) == Some("MUSIC_EXPLICIT_BADGE")
                }),
        ),
        library_add,
        library_remove,
        like_status: find(value, "likeStatus")
            .and_then(Value::as_str)
            .and_then(|s| match s {
                "LIKE" => Some(true),
                "INDIFFERENT" | "DISLIKE" => Some(false),
                _ => None,
            }),
        set_video_id: value
            .pointer("/playlistItemData/playlistSetVideoId")
            .or_else(|| value.get("setVideoId"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        artists,
        id: id.into(),
        title,
        artist,
        album,
        duration: seconds,
        artwork,
        kind,
    })
}

fn history_url(value: &Value) -> Result<reqwest::Url> {
    let url = value
        .pointer("/playbackTracking/videostatsPlaybackUrl/baseUrl")
        .and_then(Value::as_str)
        .context("No listening-history endpoint was returned.")?;
    let url = reqwest::Url::parse(url).context("Invalid listening-history endpoint.")?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("www.youtube.com" | "music.youtube.com" | "youtube.com")
        )
        || url.path() != "/api/stats/playback"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        bail!("Unexpected listening-history endpoint.");
    }
    Ok(url)
}
fn parse_suggestions(value: &Value) -> Vec<String> {
    let mut seen = HashSet::new();
    find(value, "searchSuggestionsSectionRenderer")
        .and_then(|s| s["contents"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("searchSuggestionRenderer")
                .or_else(|| item.get("historySuggestionRenderer"))
        })
        .filter_map(|item| {
            item.pointer("/navigationEndpoint/searchEndpoint/query")
                .and_then(Value::as_str)
        })
        .filter(|text| !text.is_empty() && text.len() <= 512 && seen.insert(*text))
        .take(8)
        .map(str::to_owned)
        .collect()
}
fn library_tokens(value: &Value) -> (Option<String>, Option<String>) {
    let mut result = (None, None);
    for item in value
        .pointer("/menu/menuRenderer/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let renderer = item
            .get("toggleMenuServiceItemRenderer")
            .or_else(|| item.get("menuServiceItemRenderer"))
            .unwrap_or(&Value::Null);
        let icon = renderer
            .pointer("/defaultIcon/iconType")
            .or_else(|| renderer.pointer("/icon/iconType"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let token = |endpoint: &str| {
            renderer
                .get(endpoint)
                .and_then(|e| find(e, "feedbackToken"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let normal = token("defaultServiceEndpoint").or_else(|| token("serviceEndpoint"));
        let toggled = token("toggledServiceEndpoint");
        match icon {
            "BOOKMARK_BORDER" | "LIBRARY_ADD" => result = (normal, toggled),
            "BOOKMARK" | "LIBRARY_REMOVE" => result = (toggled, normal),
            _ => {}
        }
    }
    result
}
pub fn parse_page(value: &Value, title: &str) -> Page {
    fn walk(value: &Value, tracks: &mut Vec<Track>, seen: &mut HashSet<String>) {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    if matches!(
                        key.as_str(),
                        "musicResponsiveListItemRenderer"
                            | "musicTwoRowItemRenderer"
                            | "playlistPanelVideoRenderer"
                            | "musicCardShelfRenderer"
                    ) {
                        if let Some(track) = parse_renderer(child)
                            && seen.insert(
                                track
                                    .set_video_id
                                    .clone()
                                    .unwrap_or_else(|| track.id.clone()),
                            )
                        {
                            tracks.push(track);
                        }
                        if key == "musicCardShelfRenderer" {
                            walk(&child["contents"], tracks, seen);
                        }
                    } else {
                        walk(child, tracks, seen);
                    }
                }
            }
            Value::Array(array) => {
                for child in array {
                    walk(child, tracks, seen);
                }
            }
            _ => {}
        }
    }
    let mut page = Page {
        visitor_data: value["responseContext"]["visitorData"]
            .as_str()
            .map(str::to_owned),
        title: title.into(),
        ..Page::default()
    };
    walk(value, &mut page.tracks, &mut HashSet::new());
    for name in [
        "musicImmersiveHeaderRenderer",
        "musicDetailHeaderRenderer",
        "musicResponsiveHeaderRenderer",
        "musicVisualHeaderRenderer",
    ] {
        if let Some(header) = find(value, name) {
            page.playlist_id = find(header, "playlistId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            page.subscribed = find(header, "subscribed").and_then(Value::as_bool);
            let heading = text(&header["title"]);
            if !heading.is_empty() {
                page.title = heading;
            }
            page.subtitle = text(&header["subtitle"]);
            page.description = text(&header["description"]);
            if page.description.is_empty() {
                page.description = find(header, "musicDescriptionShelfRenderer")
                    .map(|shelf| text(&shelf["description"]))
                    .unwrap_or_default();
            }
            page.artwork = find(header, "thumbnails")
                .and_then(Value::as_array)
                .and_then(|images| images.last())
                .and_then(|image| image["url"].as_str())
                .map(crate::artwork::normalize_url);
            break;
        }
    }
    fn shelves(value: &Value, sections: &mut Vec<Section>, destinations: &mut Vec<Destination>) {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    if matches!(
                        key.as_str(),
                        "musicCarouselShelfRenderer"
                            | "musicShelfRenderer"
                            | "musicPlaylistShelfRenderer"
                    ) {
                        let title = find(&child["header"], "title")
                            .map(text)
                            .filter(|title| !title.is_empty())
                            .unwrap_or_else(|| text(&child["title"]));
                        let mut tracks = Vec::new();
                        walk(child, &mut tracks, &mut HashSet::new());
                        if !tracks.is_empty() {
                            sections.push(Section { title, tracks });
                        }
                    }
                    if key == "musicNavigationButtonRenderer"
                        && let Some(endpoint) = child
                            .pointer("/clickCommand/browseEndpoint")
                            .or_else(|| child.pointer("/navigationEndpoint/browseEndpoint"))
                        && let Some(id) = endpoint["browseId"].as_str()
                    {
                        let title = text(&child["buttonText"]);
                        if !title.is_empty() && !destinations.iter().any(|item| item.title == title)
                        {
                            destinations.push(Destination {
                                title,
                                id: id.into(),
                                params: endpoint["params"].as_str().unwrap_or_default().into(),
                            });
                        }
                    }
                    shelves(child, sections, destinations);
                }
            }
            Value::Array(array) => {
                for child in array {
                    shelves(child, sections, destinations);
                }
            }
            _ => {}
        }
    }
    shelves(value, &mut page.sections, &mut page.destinations);
    page.continuation = find(value, "nextContinuationData")
        .and_then(|continuation| continuation["continuation"].as_str())
        .map(str::to_owned);
    if page.continuation.is_none() {
        page.continuation = find(value, "continuationCommand")
            .and_then(|c| c["token"].as_str())
            .map(str::to_owned);
    }
    page.editable = find(value, "musicEditablePlaylistDetailHeaderRenderer").is_some();
    page.privacy = find(value, "musicPlaylistEditHeaderRenderer")
        .and_then(|header| header["privacy"].as_str())
        .map(str::to_owned);
    page
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LyricLine {
    pub seconds: Option<f64>,
    pub text: String,
    pub words: Vec<LyricWord>,
    pub translation: Option<String>,
    pub romanization: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct LyricWord {
    pub start: f64,
    pub end: Option<f64>,
    pub text: String,
}

pub fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for line in lrc.lines() {
        let mut tail = line;
        let mut times = Vec::new();
        while let Some(rest) = tail.strip_prefix('[') {
            let Some((stamp, rest)) = rest.split_once(']') else {
                break;
            };
            let Some((minutes, seconds)) = stamp.split_once(':') else {
                break;
            };
            let (Ok(minutes), Ok(seconds)) = (minutes.parse::<f64>(), seconds.parse::<f64>())
            else {
                break;
            };
            let time = minutes * 60. + seconds;
            if time.is_finite() && time >= 0. {
                times.push(time);
            }
            tail = rest;
        }
        let (text, words) = crate::lyrics::enhanced_words(tail);
        for seconds in times {
            lines.push(LyricLine {
                seconds: Some(seconds),
                text: text.clone(),
                words: words.clone(),
                ..Default::default()
            });
        }
    }
    lines.sort_by(|a, b| {
        a.seconds
            .partial_cmp(&b.seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    lines
}
