//! Optional audio substitution; the YouTube track remains the library identity.
use crate::{
    Track,
    audio_settings::Quality,
    playback::{AudioSource, StreamQuality},
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

static ENABLED: AtomicBool = AtomicBool::new(true);
pub fn configure(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
}
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

#[derive(Clone)]
struct Candidate {
    track: Track,
    encrypted: String,
    supports_320: bool,
}
type Matches = HashMap<String, (Instant, Option<Candidate>)>;
fn matches() -> &'static Mutex<Matches> {
    static MATCHES: OnceLock<Mutex<Matches>> = OnceLock::new();
    MATCHES.get_or_init(Default::default)
}
pub(crate) fn policy() -> &'static crate::request_policy::RequestPolicy {
    static POLICY: OnceLock<crate::request_policy::RequestPolicy> = OnceLock::new();
    POLICY.get_or_init(Default::default)
}

/// Called only after the shared playback permit, never on the UI thread.
pub(crate) fn resolve(
    track: &Track,
    quality: Quality,
    id: u64,
    generation: &AtomicU64,
) -> Result<Option<AudioSource>> {
    if !enabled()
        || track.artist.trim().is_empty()
        || track.duration == 0
        || generation.load(Ordering::Acquire) != id
    {
        return Ok(None);
    }
    let key = format!(
        "{}:{}:{}:{}:{:?}",
        track.id, track.title, track.artist, track.duration, track.explicit
    );
    let held = matches()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .filter(|(at, candidate)| {
            at.elapsed() < Duration::from_secs(if candidate.is_some() { 3600 } else { 600 })
        })
        .map(|(_, candidate)| candidate.clone());
    let candidate = if let Some(held) = held {
        held
    } else {
        // Searching every credited artist can exclude the correct recording/version.
        // Search the primary artist, then validate all returned recording metadata below.
        let artist = track.artists.first().map_or_else(
            || {
                track
                    .artist
                    .split(['&', ',', ';', '/'])
                    .next()
                    .unwrap_or(&track.artist)
                    .trim()
            },
            |artist| artist.name.as_str(),
        );
        let response = get(
            "search.getResults",
            &[
                ("q", &format!("{} {}", track.title, artist)),
                ("n", "12"),
                ("p", "1"),
            ],
        )?;
        if generation.load(Ordering::Acquire) != id {
            bail!("Playback request cancelled");
        }
        let found = response["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(candidate)
            .filter(|c| same_recording(track, &c.track))
            .min_by_key(|c| c.track.duration.abs_diff(track.duration));
        let mut cache = matches().lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(3600));
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(key, (Instant::now(), found.clone()));
        found
    };
    let Some(candidate) = candidate else {
        return Ok(None);
    };
    if generation.load(Ordering::Acquire) != id {
        bail!("Playback request cancelled");
    }
    let (url, bitrate) = media_url(&candidate.encrypted, candidate.supports_320, quality)?;
    Ok(Some(AudioSource {
        duration: candidate.track.duration as f64,
        url,
        headers: vec![],
        user_agent: "Mozilla/5.0".into(),
        quality: StreamQuality {
            source: "JioSaavn".into(),
            codec: "aac".into(),
            bitrate: bitrate as f64,
            protocol: "https".into(),
            ..Default::default()
        },
    }))
}

fn get(call: &str, params: &[(&str, &str)]) -> Result<Value> {
    policy().before()?;
    static HTTP: OnceLock<Result<reqwest::blocking::Client, reqwest::Error>> = OnceLock::new();
    let http = HTTP
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(3))
                .user_agent("Mozilla/5.0")
                .build()
        })
        .as_ref()
        .map_err(|_| anyhow::anyhow!("JioSaavn connection unavailable"))?;
    let mut fields = vec![("__call", call), ("_format", "json")];
    if call != "song.generateAuthToken" {
        fields.extend([("_marker", "0"), ("api_version", "4"), ("ctx", "web6dot0")]);
    }
    fields.extend_from_slice(params);
    let request = if call == "song.generateAuthToken" {
        http.post("https://www.jiosaavn.com/api.php").form(&fields)
    } else {
        http.get("https://www.jiosaavn.com/api.php").query(&fields)
    };
    let response = request
        .header("Accept", "application/json")
        .header("Cookie", "explicit_content=1")
        .send()
        .map_err(|_| {
            policy().pause(Duration::from_secs(15));
            anyhow::anyhow!("JioSaavn could not be reached")
        })?;
    policy().observe(response.status(), response.headers());
    if !response.status().is_success() {
        if response.status().as_u16() == 403 || response.status().is_server_error() {
            policy().pause(Duration::from_secs(60));
        }
        bail!("JioSaavn returned HTTP {}", response.status().as_u16());
    }
    crate::resource::json(response, 2 * 1024 * 1024).context("Invalid JioSaavn response")
}

fn candidate(value: &Value) -> Option<Candidate> {
    let info = &value["more_info"];
    let mut track = Track::song(
        value["id"].as_str()?,
        unescape(value["title"].as_str()?),
        "",
    );
    track.artist = info["artistMap"]["primary_artists"]
        .as_array()?
        .iter()
        .filter_map(|a| a["name"].as_str())
        .map(unescape)
        .collect::<Vec<_>>()
        .join(", ");
    track.duration = info["duration"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| info["duration"].as_u64())?;
    track.explicit = Some(
        value["explicit_content"].as_str() == Some("1")
            || value["explicit_content"].as_str() == Some("true")
            || value["explicit_content"].as_bool() == Some(true),
    );
    Some(Candidate {
        track,
        encrypted: info["encrypted_media_url"].as_str()?.into(),
        supports_320: info["320kbps"].as_str() == Some("true")
            || info["320kbps"].as_bool() == Some(true),
    })
}

fn unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}
fn title(text: &str) -> Vec<String> {
    words(text)
        .into_iter()
        .filter(|w| {
            !["official", "audio", "video", "lyrics", "lyric", "hd", "4k"].contains(&w.as_str())
        })
        .collect()
}
/// Deliberately conservative: version words remain significant and runtimes must agree.
fn same_recording(wanted: &Track, found: &Track) -> bool {
    let wanted_title = title(&wanted.title);
    let artists = |text: &str| {
        text.split([',', '&', ';', '/'])
            .map(|a| words(a.trim().trim_end_matches(" - Topic")))
            .filter(|a| !a.is_empty())
            .collect::<Vec<_>>()
    };
    let wanted_artists = artists(&wanted.artist);
    let found_artists = artists(&found.artist);
    !wanted_title.is_empty()
        && wanted_title == title(&found.title)
        && wanted
            .explicit
            .is_none_or(|explicit| found.explicit == Some(explicit))
        && wanted.duration > 0
        && found.duration > 0
        && wanted.duration.abs_diff(found.duration) <= 8
        && wanted_artists.iter().any(|a| found_artists.contains(a))
}

fn media_url(encrypted: &str, supports_320: bool, quality: Quality) -> Result<(String, u32)> {
    // Use the provider's format endpoint, as yt-dlp's maintained extractor does.
    // Rewriting a decrypted CDN filename can select a nonexistent rendition.
    let bitrate = match quality {
        Quality::DataSaver => 64,
        Quality::Standard => 128,
        Quality::Best if supports_320 => 320,
        Quality::Best => 128,
    };
    let response = get(
        "song.generateAuthToken",
        &[("url", encrypted), ("bitrate", &bitrate.to_string())],
    )?;
    let raw = response["auth_url"]
        .as_str()
        .context("JioSaavn did not return playable audio")?;
    let mut url = reqwest::Url::parse(raw).context("Invalid JioSaavn media address")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.host_str().is_some_and(|h| h.ends_with(".saavncdn.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        bail!("Unsupported JioSaavn media host");
    }
    let _ = url.set_scheme("https");
    Ok((url.into(), bitrate))
}

/// A CDN refusal must fall through to YouTube on the next resolve, not repeat the same URL.
pub(crate) fn refused(track_id: &str) {
    let mut cache = matches().lock().unwrap_or_else(|e| e.into_inner());
    for (_, entry) in cache
        .iter_mut()
        .filter(|(key, _)| key.starts_with(&format!("{track_id}:")))
    {
        *entry = (Instant::now(), None);
    }
}
