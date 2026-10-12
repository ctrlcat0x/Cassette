//! Free, native lyric providers. Timings are used only when supplied by the provider.
use crate::{
    Track,
    catalog::{LyricLine, parse_lrc},
};
use anyhow::{Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    pub provider: String,
    pub instrumental: bool,
}
static LRCLIB_POLICY: OnceLock<crate::request_policy::RequestPolicy> = OnceLock::new();
static HTTP: OnceLock<Client> = OnceLock::new();

fn lrclib(
    http: &Client,
    route: &str,
    query: &[(String, String)],
    deadline: Instant,
) -> Result<Value> {
    let policy = LRCLIB_POLICY.get_or_init(Default::default);
    policy.before()?;
    let response = http
        .get(format!("https://lrclib.net/api/{route}"))
        .query(query)
        .timeout(remaining(deadline)?)
        .send()?;
    policy.observe(response.status(), response.headers());
    crate::resource::json(response.error_for_status()?, crate::resource::LYRIC_BYTES)
}
fn plain(text: &str) -> Vec<LyricLine> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|text| LyricLine {
            seconds: None,
            text: text.trim().into(),
        })
        .collect()
}
fn record(value: &Value) -> Option<Lyrics> {
    let instrumental = value["instrumental"].as_bool().unwrap_or(false);
    let synced = value["syncedLyrics"]
        .as_str()
        .map(parse_lrc)
        .unwrap_or_default();
    let lines = if synced.iter().any(|line| line.seconds.is_some()) {
        synced
    } else {
        value["plainLyrics"].as_str().map(plain).unwrap_or_default()
    };
    (!lines.is_empty() || instrumental).then(|| Lyrics {
        lines,
        provider: "LRCLIB".into(),
        instrumental,
    })
}
fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn select_search<'a>(values: &'a [Value], track: &Track) -> Option<&'a Value> {
    values
        .iter()
        .filter(|value| {
            normalized(value["trackName"].as_str().unwrap_or_default()) == normalized(&track.title)
                && normalized(value["artistName"].as_str().unwrap_or_default())
                    == normalized(&track.artist)
                && (track.duration == 0
                    || value["duration"]
                        .as_f64()
                        .is_some_and(|seconds| (seconds - track.duration as f64).abs() <= 5.))
        })
        .min_by(|a, b| {
            let score = |v: &Value| {
                (v["duration"].as_f64().unwrap_or_default() - track.duration as f64).abs()
            };
            score(a).total_cmp(&score(b))
        })
}

#[derive(Clone, Copy, Debug)]
pub enum Provider {
    BiniLyrics,
    LrcRed,
    LyricsPlus,
}

fn remaining(deadline: Instant) -> Result<Duration> {
    let time = deadline.saturating_duration_since(Instant::now());
    if time.is_zero() {
        bail!("Lyrics lookup timed out");
    }
    Ok(time.min(Duration::from_secs(6)))
}
fn get(http: &Client, url: reqwest::Url, provider: Provider, deadline: Instant) -> Result<Vec<u8>> {
    use crate::request_policy::RequestPolicy;
    static POLICIES: OnceLock<[RequestPolicy; 3]> = OnceLock::new();
    // Bini redirects to lrc.red and shares its catalogue and cooldown.
    let policy = &POLICIES.get_or_init(|| std::array::from_fn(|_| RequestPolicy::default()))[if matches!(
        provider,
        Provider::LyricsPlus
    ) {
        1
    } else {
        0
    }];
    if crate::cache::offline_only() {
        bail!("Lyrics are unavailable offline");
    }
    policy.before()?;
    let response = http
        .get(url)
        .timeout(remaining(deadline)?)
        .header(
            "Accept",
            "application/json, application/ttml+xml, application/xml;q=0.9",
        )
        .send()
        .map_err(|_| anyhow::anyhow!("Lyrics connection failed"))?;
    policy.observe(response.status(), response.headers());
    crate::resource::read_limited(response.error_for_status()?, crate::resource::LYRIC_BYTES)
}
fn core_title(text: &str) -> String {
    normalized(
        text.split(['(', '['])
            .next()
            .unwrap_or(text)
            .split(" - ")
            .next()
            .unwrap_or(text),
    )
}
fn matches_recording(title: &str, artist: &str, duration: Option<f64>, track: &Track) -> bool {
    let versions = [
        "live",
        "remix",
        "acoustic",
        "instrumental",
        "karaoke",
        "cover",
        "sped",
        "slowed",
        "nightcore",
        "demo",
        "edit",
        "extended",
    ];
    let contains = |text: &str, word: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|part| part.eq_ignore_ascii_case(word))
    };
    core_title(title) == core_title(&track.title)
        && versions
            .iter()
            .all(|word| contains(title, word) == contains(&track.title, word))
        && artist.split([',', '&', ';', '/']).any(|name| {
            track.artist.split([',', '&', ';', '/']).any(|wanted| {
                !normalized(name).is_empty() && normalized(name) == normalized(wanted)
            })
        })
        && (track.duration == 0
            || duration.is_some_and(|seconds| {
                seconds.is_finite() && (seconds - track.duration as f64).abs() <= 5.
            }))
}
fn document_url(isrc: &str) -> Option<reqwest::Url> {
    let code = isrc.to_ascii_uppercase();
    (code.is_ascii()
        && code.len() == 12
        && code[..2].bytes().all(|c| c.is_ascii_uppercase())
        && code[2..5].bytes().all(|c| c.is_ascii_alphanumeric())
        && code[5..].bytes().all(|c| c.is_ascii_digit()))
    .then(|| reqwest::Url::parse(&format!("https://lrc.red/s/{code}.ttml")).ok())
    .flatten()
}
fn stamp(value: &str) -> Option<f64> {
    let seconds = if let Some(ms) = value.strip_suffix("ms") {
        ms.parse::<f64>().ok()? / 1000.
    } else if let Some(s) = value.strip_suffix('s') {
        s.parse().ok()?
    } else {
        value.split(':').try_fold(0., |time, part| {
            Some(time * 60. + part.parse::<f64>().ok()?)
        })?
    };
    (seconds.is_finite() && seconds >= 0.).then_some(seconds)
}
fn ttml(bytes: &[u8]) -> Result<Vec<LyricLine>> {
    let text = std::str::from_utf8(bytes)?;
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            nodes_limit: 40_000,
            ..Default::default()
        },
    )?;
    let mut lines: Vec<_> = document
        .descendants()
        .filter(|n| n.has_tag_name("p"))
        .filter_map(|node| {
            let seconds = node.attribute("begin").and_then(stamp).or_else(|| {
                node.descendants()
                    .find_map(|n| n.attribute("begin").and_then(stamp))
            })?;
            let text: String = node
                .descendants()
                .filter(|n| n.is_text())
                .filter(|n| {
                    !n.ancestors().any(|parent| {
                        parent.attributes().any(|a| {
                            a.name() == "role" && matches!(a.value(), "x-translation" | "x-roman")
                        })
                    })
                })
                .filter_map(|n| n.text())
                .collect();
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            (!text.is_empty()).then_some(LyricLine {
                seconds: Some(seconds),
                text,
            })
        })
        .take(2000)
        .collect();
    lines.sort_by(|a, b| {
        a.seconds
            .partial_cmp(&b.seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(lines)
}
fn client() -> Result<&'static Client> {
    if HTTP.get().is_none() {
        let value = Client::builder()
            .timeout(Duration::from_secs(6))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() < 3
                    && attempt.url().scheme() == "https"
                    && matches!(
                        attempt.url().host_str(),
                        Some("lrc.red" | "lyrics-api.binimum.org")
                    )
                {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .user_agent("Cassette/0.1.0 (app://app.raag.desktop)")
            .build()?;
        let _ = HTTP.set(value);
    }
    Ok(HTTP.get().expect("lyrics client initialized"))
}
pub fn fetch_provider(track: &Track, provider: Provider) -> Result<Lyrics> {
    fetch_hosted(
        client()?,
        track,
        provider,
        &mut std::collections::HashSet::new(),
        Instant::now() + Duration::from_secs(15),
    )
}
fn fetch_hosted(
    http: &Client,
    track: &Track,
    provider: Provider,
    tried: &mut std::collections::HashSet<String>,
    deadline: Instant,
) -> Result<Lyrics> {
    let (base, title_key) = match provider {
        Provider::BiniLyrics => ("https://lyrics-api.binimum.org/", "track"),
        Provider::LrcRed => ("https://lrc.red/search.json", "q"),
        Provider::LyricsPlus => ("https://lyricsplus.binimum.org/v2/lyrics/get", "title"),
    };
    let mut url = reqwest::Url::parse(base)?;
    if matches!(provider, Provider::LrcRed) {
        url.query_pairs_mut()
            .append_pair(title_key, &format!("{} {}", track.title, track.artist));
    } else {
        url.query_pairs_mut()
            .append_pair(title_key, &track.title)
            .append_pair("artist", &track.artist);
        if track.duration > 0 {
            url.query_pairs_mut()
                .append_pair("duration", &track.duration.to_string());
        }
    }
    let bytes = get(http, url.clone(), provider, deadline).or_else(|error| {
        let blocked = error
            .downcast_ref::<reqwest::Error>()
            .and_then(reqwest::Error::status)
            .is_some_and(|status| matches!(status.as_u16(), 401 | 403 | 429));
        if !matches!(provider, Provider::LyricsPlus) || blocked {
            return Err(error);
        }
        // One sequential backup, with the same cooldown; no mirror fan-out.
        url.set_host(Some("lyricsplus.atomix.one"))?;
        get(http, url, provider, deadline)
    })?;
    let value: Value = serde_json::from_slice(&bytes)?;
    let mut lines: Vec<LyricLine> = if matches!(provider, Provider::LyricsPlus) {
        value["lyrics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|line| {
                let seconds = line["time"].as_f64()? / 1000.;
                let text = line["text"].as_str()?.trim();
                (seconds.is_finite() && seconds >= 0. && !text.is_empty()).then(|| LyricLine {
                    seconds: Some(seconds),
                    text: text.into(),
                })
            })
            .take(2000)
            .collect()
    } else {
        let (array, title, artist) = if matches!(provider, Provider::BiniLyrics) {
            ("results", "track_name", "artist_name")
        } else {
            ("hits", "title", "artist")
        };
        let candidate = value[array]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| {
                matches_recording(
                    v[title].as_str().unwrap_or_default(),
                    v[artist].as_str().unwrap_or_default(),
                    v["duration"].as_f64(),
                    track,
                )
            })
            .ok_or_else(|| anyhow::anyhow!("No matching lyric recording"))?;
        let url = candidate["isrc"]
            .as_str()
            .and_then(document_url)
            .ok_or_else(|| anyhow::anyhow!("Invalid lyric recording ID"))?;
        if !tried.insert(url.to_string()) {
            bail!("Lyric document already tried");
        }
        // Both indexes share this host: share its cooldown too. Ignore arbitrary supplied URLs.
        ttml(&get(http, url, Provider::LrcRed, deadline)?)?
    };
    if lines.is_empty() {
        bail!("No lyrics found");
    }
    lines.sort_by(|a, b| {
        a.seconds
            .partial_cmp(&b.seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(Lyrics {
        lines,
        provider: match provider {
            Provider::BiniLyrics => "BiniLyrics",
            Provider::LrcRed => "lrc.red",
            Provider::LyricsPlus => "LyricsPlus",
        }
        .into(),
        instrumental: false,
    })
}
pub fn fetch(track: &Track) -> Result<Lyrics> {
    if track.kind == crate::ItemKind::Local {
        let path = std::path::Path::new(&track.id).with_extension("lrc");
        if let Ok(file) = std::fs::File::open(path)
            && let Ok(bytes) = crate::resource::read_limited(file, crate::resource::LYRIC_BYTES)
            && let Ok(text) = String::from_utf8(bytes)
        {
            let lines = parse_lrc(&text);
            if !lines.is_empty() {
                return Ok(Lyrics {
                    lines,
                    provider: "Local LRC".into(),
                    instrumental: false,
                });
            }
        }
    }
    let http = client()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    if crate::cache::offline_only() {
        bail!("Lyrics are not cached for offline listening.");
    }
    let mut query = vec![
        ("track_name".into(), track.title.clone()),
        ("artist_name".into(), track.artist.clone()),
    ];
    if !track.album.is_empty() {
        query.push(("album_name".into(), track.album.clone()));
    }
    if (1..=3600).contains(&track.duration) {
        query.push(("duration".into(), track.duration.to_string()));
    }
    if let Ok(value) = lrclib(http, "get", &query, deadline)
        && let Some(lyrics) = record(&value)
    {
        return Ok(lyrics);
    }
    let mut tried = std::collections::HashSet::new();
    for provider in [Provider::BiniLyrics, Provider::LrcRed, Provider::LyricsPlus] {
        if let Ok(lyrics) = fetch_hosted(http, track, provider, &mut tried, deadline) {
            return Ok(lyrics);
        }
    }
    // Search tolerates incomplete album metadata; reject covers and mismatched timings.
    if let Ok(value) = lrclib(http, "search", &query[..2], deadline)
        && let Some(values) = value.as_array()
        && let Some(candidate) = select_search(values, track)
        && let Some(lyrics) = record(candidate)
    {
        return Ok(lyrics);
    }
    let mut url = reqwest::Url::parse("https://api.lyrics.ovh/v1/")?;
    url.path_segments_mut()
        .unwrap()
        .pop_if_empty()
        .push(&track.artist)
        .push(&track.title);
    static OVH_POLICY: OnceLock<crate::request_policy::RequestPolicy> = OnceLock::new();
    let policy = OVH_POLICY.get_or_init(Default::default);
    if policy.before().is_ok()
        && let Ok(timeout) = remaining(deadline)
        && let Ok(response) = http.get(url).timeout(timeout).send()
        && {
            policy.observe(response.status(), response.headers());
            response.status().is_success()
        }
        && let Ok(value) = crate::resource::json::<Value>(response, crate::resource::LYRIC_BYTES)
        && let Some(text) = value["lyrics"].as_str()
    {
        let lines = plain(text);
        if !lines.is_empty() {
            return Ok(Lyrics {
                lines,
                provider: "lyrics.ovh".into(),
                instrumental: false,
            });
        }
    }
    bail!("Lyrics aren’t available for this song yet.")
}
