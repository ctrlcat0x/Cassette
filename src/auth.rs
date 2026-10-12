//! Website-session authentication. Secrets never implement Debug or enter library JSON.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::{
    collections::BTreeMap,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub mod storage;
pub const SESSION_LIMIT: u64 = 128 * 1024;
pub const ORIGIN: &str = "https://music.youtube.com";

/// yt-dlp saves its cookie jar on exit. Give each invocation a private copy so
/// it cannot overwrite a rotating session or race another extractor. Keep the
/// parent hidden, but not the copy: Windows denies Python's truncate-open on
/// files with FILE_ATTRIBUTE_HIDDEN, even when the owner has full access.
pub(crate) fn playback_snapshot(path: &Path) -> Result<tempfile::NamedTempFile> {
    let bytes = Zeroizing::new(crate::resource::read_limited(
        std::fs::File::open(path).context("Could not read playback credentials")?,
        SESSION_LIMIT,
    )?);
    let mut file = tempfile::Builder::new()
        .prefix("raag-extractor-")
        .suffix(".txt")
        .tempfile_in(storage::export_dir()?)?;
    storage::restrict(file.path())?;
    file.write_all(&bytes)?;
    file.flush()?;
    Ok(file)
}

/// A length-delimited anonymous-pipe message; does not wait for descendant processes to close stdout.
pub fn read_login_message(mut reader: impl std::io::Read) -> Result<Zeroizing<Vec<u8>>> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as u64;
    if length == 0 || length > SESSION_LIMIT {
        bail!("Invalid login response size");
    }
    let bytes = Zeroizing::new(crate::resource::read_limited(reader.take(length), length)?);
    if bytes.len() as u64 != length {
        bail!("Incomplete login response");
    }
    Ok(bytes)
}

#[derive(Debug)]
pub struct LoginCanceled;
impl std::fmt::Display for LoginCanceled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Sign-in canceled")
    }
}
impl std::error::Error for LoginCanceled {}

/// Safe failure categories: never contain cookies, response bodies, URLs or account details.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SignInFailure {
    Network,
    Timeout,
    Http(u16),
    InvalidResponse,
    AccountNotConfirmed,
    IncompleteAccount,
    CookieImport,
    CookieCapture,
    Handoff,
    SessionExport,
}
impl SignInFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Network => "validation_network",
            Self::Timeout => "validation_timeout",
            Self::Http(_) => "validation_http",
            Self::InvalidResponse => "validation_response",
            Self::AccountNotConfirmed => "account_not_confirmed",
            Self::IncompleteAccount => "account_identity",
            Self::CookieImport => "cookie_import",
            Self::CookieCapture => "cookie_capture",
            Self::Handoff => "helper_handoff",
            Self::SessionExport => "playback_session_export",
        }
    }
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::Network
                | Self::Timeout
                | Self::Http(401 | 403 | 500..=599)
                | Self::AccountNotConfirmed
                | Self::IncompleteAccount
        )
    }
}
impl std::fmt::Display for SignInFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network => f.write_str("YouTube Music could not be reached. Check your connection and retry."),
            Self::Timeout => f.write_str("YouTube Music timed out. Retry the request."),
            Self::Http(status) => write!(f, "YouTube Music rejected the request (HTTP {status}). Retry the request."),
            Self::InvalidResponse => f.write_str("YouTube Music returned an unreadable response. Retry the request."),
            Self::AccountNotConfirmed => f.write_str("YouTube Music has not confirmed a signed-in account yet. Finish signing in on the YouTube Music page and retry."),
            Self::IncompleteAccount => f.write_str("YouTube Music confirmed a session but did not return an account name. Retry sign-in."),
            Self::CookieImport => f.write_str("The selected file does not contain a usable YouTube session. Import a fresh Netscape cookie file."),
            Self::CookieCapture => f.write_str("The login browser did not provide usable YouTube session cookies. Retry sign-in."),
            Self::Handoff => f.write_str("The login window could not pass its session back to Cassette. Retry sign-in."),
            Self::SessionExport => f.write_str("The validated session could not be prepared for playback. Check access to your temporary folder and retry."),
        }
    }
}
impl std::error::Error for SignInFailure {}

#[derive(Serialize, Deserialize)]
pub enum LoginReply {
    Validated(SessionMaterial),
    Failed(SignInFailure),
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct BrowserCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<i64>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CookieJar(Vec<BrowserCookie>);
impl CookieJar {
    pub fn into_cookies(self) -> Vec<BrowserCookie> {
        self.0
    }
    pub fn new(cookies: Vec<BrowserCookie>) -> Result<Self> {
        if cookies.len() > 512
            || cookies
                .iter()
                .map(|c| c.name.len() + c.value.len() + c.domain.len() + c.path.len())
                .sum::<usize>() as u64
                > SESSION_LIMIT
        {
            bail!("Too many session cookies");
        }
        let now = now();
        let mut jar = Self::default();
        for cookie in cookies {
            if valid_cookie(&cookie, now) {
                jar.upsert(cookie);
            }
        }
        if jar.auth_value().is_none() {
            bail!("YouTube has not provided a valid signed-in session yet");
        }
        Ok(jar)
    }
    fn upsert(&mut self, cookie: BrowserCookie) {
        self.0.retain(|c| {
            !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path)
        });
        self.0.push(cookie);
    }
    pub fn header(&self) -> Zeroizing<String> {
        let now = now();
        let mut values = BTreeMap::<&str, &BrowserCookie>::new();
        for cookie in &self.0 {
            if !valid_cookie(cookie, now) || !"/youtubei/v1/".starts_with(&cookie.path) {
                continue;
            }
            let old = values.get(cookie.name.as_str());
            if old.is_none_or(|old| {
                (cookie.domain.len(), cookie.path.len()) > (old.domain.len(), old.path.len())
            }) {
                values.insert(&cookie.name, cookie);
            }
        }
        let mut header = Zeroizing::new(String::new());
        for c in values.values() {
            if !header.is_empty() {
                header.push_str("; ");
            }
            let _ = write!(&mut *header, "{}={}", c.name, c.value);
        }
        header
    }
    pub fn auth_value(&self) -> Option<&str> {
        // Deterministic precedence independent of native cookie-store iteration order.
        ["__Secure-3PAPISID", "SAPISID"]
            .into_iter()
            .find_map(|name| {
                self.0
                    .iter()
                    .filter(|c| c.name == name && valid_cookie(c, now()))
                    .max_by_key(|c| (c.domain.len(), c.path.len()))
                    .map(|c| c.value.as_str())
            })
    }
    pub fn absorb(&mut self, header: &str) {
        let Ok(parsed) = cookie::Cookie::parse(header.to_owned()) else {
            return;
        };
        let domain = parsed
            .domain()
            .unwrap_or("music.youtube.com")
            .trim_start_matches('.')
            .to_ascii_lowercase();
        if !youtube_domain(&domain) {
            return;
        }
        let path = parsed.path().unwrap_or("/youtubei/v1/").to_owned();
        let expires = parsed
            .max_age()
            .map(|age| now().saturating_add(age.whole_seconds()))
            .or_else(|| parsed.expires_datetime().map(|date| date.unix_timestamp()));
        let replacement = BrowserCookie {
            name: parsed.name().into(),
            value: parsed.value().into(),
            domain,
            path,
            secure: parsed.secure().unwrap_or(false),
            http_only: parsed.http_only().unwrap_or(false),
            expires,
        };
        self.0.retain(|c| {
            !(c.name == replacement.name
                && c.domain == replacement.domain
                && c.path == replacement.path)
        });
        if valid_cookie(&replacement, now()) && self.0.len() < 512 {
            self.0.push(replacement);
        }
    }
    pub fn netscape(&self) -> Zeroizing<String> {
        let mut output = Zeroizing::new(String::from("# Netscape HTTP Cookie File\n"));
        for c in self.0.iter().filter(|c| valid_cookie(c, now())) {
            let domain = if c.domain == "youtube.com" {
                ".youtube.com"
            } else {
                &c.domain
            };
            output.push_str(&format!(
                "{}{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                if c.http_only { "#HttpOnly_" } else { "" },
                domain,
                if domain.starts_with('.') {
                    "TRUE"
                } else {
                    "FALSE"
                },
                c.path,
                if c.secure { "TRUE" } else { "FALSE" },
                c.expires.unwrap_or(0),
                c.name,
                c.value
            ));
        }
        output
    }
    pub fn import(path: &Path) -> Result<Self> {
        let bytes = Zeroizing::new(crate::resource::read_limited(
            std::fs::File::open(path)?,
            SESSION_LIMIT,
        )?);
        let text =
            std::str::from_utf8(&bytes).context("Cookie file must be UTF-8 Netscape format")?;
        let mut cookies = Vec::new();
        for line in text.lines() {
            let http_only = line.starts_with("#HttpOnly_");
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 7 {
                bail!("Cookie file must use Netscape format");
            }
            let expiry: i64 = fields[4].parse().context("Invalid cookie expiration")?;
            cookies.push(BrowserCookie {
                domain: fields[0].trim_start_matches('.').to_ascii_lowercase(),
                path: fields[2].into(),
                secure: fields[3] == "TRUE",
                expires: (expiry != 0).then_some(expiry),
                name: fields[5].into(),
                value: fields[6].into(),
                http_only,
            });
        }
        Self::new(cookies)
    }
    pub fn export(&self) -> Result<tempfile::NamedTempFile> {
        let mut file = tempfile::Builder::new()
            .prefix("raag-session-")
            .suffix(".txt")
            .tempfile_in(storage::export_dir()?)?;
        storage::restrict(file.path())?;
        storage::hide_private(file.path())?;
        file.write_all(self.netscape().as_bytes())?;
        file.flush()?;
        Ok(file)
    }
}

fn valid_cookie(c: &BrowserCookie, now: i64) -> bool {
    youtube_domain(&c.domain)
        && !c.name.is_empty()
        && !c.value.is_empty()
        && c.name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        && !c.value.bytes().any(|b| b.is_ascii_control() || b == b';')
        && c.path.starts_with('/')
        && !c.path.bytes().any(|b| b.is_ascii_control())
        && c.expires.is_none_or(|expiry| expiry > now)
        && c.name.len() + c.value.len() < 16384
        && (!c.name.starts_with("__Secure-") || c.secure)
}
pub fn youtube_domain(domain: &str) -> bool {
    matches!(
        domain.trim_start_matches('.'),
        "youtube.com" | "music.youtube.com"
    )
}
pub fn completion_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some("music.youtube.com")
            && u.port_or_known_default() == Some(443)
            && u.username().is_empty()
            && u.password().is_none()
    })
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub handle: String,
    pub channel_id: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}
impl Account {
    pub fn cache_scope(&self) -> String {
        crate::cache::key(&format!("{}:{}", self.channel_id, self.handle))
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionMaterial {
    pub cookies: CookieJar,
    pub account: Account,
    pub visitor_data: String,
    pub data_sync_id: String,
}

/// Shared only by backend workers; the UI receives Account and a playback file path.
pub struct ActiveSession {
    account: Account,
    material: std::sync::Mutex<SessionMaterial>,
    export: tempfile::TempPath,
    playback_path: String,
    dirty: std::sync::atomic::AtomicBool,
}
impl ActiveSession {
    pub fn new(material: SessionMaterial) -> Result<Self> {
        let export = material.cookies.export()?;
        let playback_path = export.path().to_string_lossy().into_owned();
        Ok(Self {
            account: material.account.clone(),
            material: std::sync::Mutex::new(material),
            export: export.into_temp_path(),
            playback_path,
            dirty: true.into(),
        })
    }
    pub fn material(&self) -> SessionMaterial {
        self.material.lock().unwrap().clone()
    }
    pub fn account(&self) -> Account {
        self.account.clone()
    }
    pub fn playback_path(&self) -> &str {
        &self.playback_path
    }
    pub fn absorb(&self, headers: &reqwest::header::HeaderMap) -> Result<()> {
        let mut material = self.material.lock().unwrap();
        let mut changed = false;
        for value in headers.get_all(reqwest::header::SET_COOKIE) {
            if let Ok(value) = value.to_str() {
                material.cookies.absorb(value);
                changed = true;
            }
        }
        if changed {
            // Preserve rotated credentials even if refreshing the playback file fails.
            self.dirty.store(true, std::sync::atomic::Ordering::Release);
            // Atomic replacement avoids yt-dlp reading a truncated rotating cookie file.
            let mut replacement = tempfile::NamedTempFile::new_in(self.export.parent().unwrap())?;
            storage::restrict(replacement.path())?;
            storage::hide_private(replacement.path())?;
            replacement.write_all(material.cookies.netscape().as_bytes())?;
            replacement
                .persist(&self.export)
                .context("Could not refresh private playback credentials")?;
        }
        Ok(())
    }
    pub fn persist(&self) -> Result<()> {
        if !self.dirty.swap(false, std::sync::atomic::Ordering::AcqRel) {
            return Ok(());
        }
        let bytes = Zeroizing::new(serde_json::to_vec(&self.material())?);
        if let Err(error) = storage::save(&bytes) {
            self.dirty.store(true, std::sync::atomic::Ordering::Release);
            return Err(error);
        }
        Ok(())
    }
}
