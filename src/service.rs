use crate::auth_controller::{SessionAccess, Snapshot};
use crate::jobs::{self, JobReceiver, JobSender};
use raag::{
    ItemKind, Track,
    catalog::{Catalog, Page},
    library::{Library, data_dir},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, SyncSender},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub enum Request {
    Home(u64),
    Explore(u64),
    Search(u64, String, Option<ItemKind>),
    Browse(u64, String, String),
    Discover(u64, String, String, String),
    More(u64, String, bool, Option<String>),
    Radio(u64, String),
    Artist(u64, String),
}
pub enum Event {
    Page(u64, Page, bool),
    Failure(u64, String),
    Lyrics(String, Result<raag::lyrics::Lyrics, String>),
    Artwork(HashMap<String, PathBuf>, HashMap<PathBuf, u8>),
    ArtworkColors(HashMap<String, raag::artwork::ArtworkColors>),
    Saved(Result<(), String>),
    Imported(Box<Library>, usize),
    Offline(Vec<Track>),
    CacheStatus(Option<String>),
    Radio(u64, Result<Vec<Track>, String>),
    Suggestions(u64, Vec<String>),
    Collection(u64, bool, Result<Vec<Track>, String>),
}
enum SaveJob {
    Save(Library),
    Flush(Library, Sender<()>),
}
pub struct CollectionLoad {
    pub epoch: u64,
    pub download: bool,
    pub account: bool,
    pub id: String,
    pub page: Page,
    pub snapshot: Snapshot,
    pub filter: String,
    pub sort: usize,
}
pub fn filter_tracks(tracks: &mut Vec<Track>, filter: &str, sort: usize) {
    if !filter.is_empty() {
        tracks.retain(|track| {
            format!("{} {} {}", track.title, track.artist, track.album)
                .to_lowercase()
                .contains(filter)
        });
    }
    match sort {
        1 => tracks.sort_by_cached_key(|track| track.title.to_lowercase()),
        2 => tracks.sort_by_cached_key(|track| track.artist.to_lowercase()),
        3 => tracks.sort_by_key(|track| track.duration),
        _ => {}
    }
}
pub struct Backend {
    pub requests: JobSender<Request>,
    pub events: Receiver<Event>,
    pub events_sender: SyncSender<Event>,
    saves: JobSender<SaveJob>,
    pub caches: JobSender<(Track, bool, String, String)>,
    pub lyrics: JobSender<(String, Track, raag::lyrics::Source, bool)>,
    pub radio: JobSender<(u64, Track)>,
    pub suggestions: JobSender<(u64, String)>,
    pub collections: SyncSender<CollectionLoad>,
    pub collection_epoch: Arc<AtomicU64>,
    pub download_epoch: Arc<AtomicU64>,
}
impl Backend {
    pub fn save(&self, library: Library) {
        let _ = self.saves.send(SaveJob::Save(library));
    }
    pub fn flush(&self, library: Library) {
        let (ack, completed) = mpsc::channel();
        if self.saves.send(SaveJob::Flush(library, ack)).is_ok() {
            let _ = completed.recv_timeout(Duration::from_secs(2));
        }
    }
    pub fn new(auth: SessionAccess) -> Self {
        let (radio, radio_jobs) = jobs::channel::<(u64, Track)>(1);
        let radio_auth = auth.clone();
        let suggestion_auth = auth.clone();
        let (suggestions, suggestion_jobs) = jobs::channel::<(u64, String)>(1);
        let collection_auth = auth.clone();
        let (collections, collection_jobs) = mpsc::sync_channel::<CollectionLoad>(8);
        let (requests, requests_rx) = jobs::channel::<Request>(1);
        let (network, jobs) = jobs::channel::<(Request, PathBuf, bool, Snapshot)>(1);
        let (events, results) = mpsc::sync_channel(128);
        let radio_events = events.clone();
        let suggestion_events = events.clone();
        thread::spawn(move || {
            let Ok(mut catalog) = Catalog::new(None) else {
                return;
            };
            while let Ok((revision, query)) = suggestion_jobs.recv() {
                if raag::cache::offline_only() {
                    continue;
                }
                let snapshot = suggestion_auth.snapshot();
                catalog.set_session(snapshot.session);
                let results = catalog.suggestions(&query).unwrap_or_default();
                if suggestion_auth.current(snapshot.generation) {
                    let _ = suggestion_events.send(Event::Suggestions(revision, results));
                }
            }
        });
        let collection_events = events.clone();
        let collection_epoch = Arc::new(AtomicU64::new(0));
        let download_epoch = Arc::new(AtomicU64::new(0));
        let queue_current = collection_epoch.clone();
        let download_current = download_epoch.clone();
        thread::spawn(move || {
            let Ok(mut catalog) = Catalog::new(None) else {
                return;
            };
            while let Ok(CollectionLoad {
                epoch,
                download,
                account,
                id,
                mut page,
                snapshot,
                filter,
                sort,
            }) = collection_jobs.recv()
            {
                catalog.set_session(snapshot.session.clone());
                let mut seen = std::collections::HashSet::new();
                let current = || {
                    epoch
                        == if download {
                            download_current.load(Ordering::Acquire)
                        } else {
                            queue_current.load(Ordering::Acquire)
                        }
                };
                if !current() || !collection_auth.current(snapshot.generation) {
                    continue;
                }
                let result = (|| -> anyhow::Result<Vec<Track>> {
                    while let Some(token) = page.continuation.take() {
                        if !current() {
                            anyhow::bail!("Collection loading cancelled.");
                        }
                        if raag::cache::offline_only() {
                            anyhow::bail!("The rest of this collection is unavailable offline.");
                        }
                        if !collection_auth.current(snapshot.generation) {
                            anyhow::bail!("Account changed.");
                        }
                        if seen.len() >= 128
                            || page.tracks.len() >= 10_000
                            || !seen.insert(token.clone())
                        {
                            anyhow::bail!("Collection reached the pagination safety limit.");
                        }
                        let next = if account {
                            catalog.account_playlist(&id, Some(&token))?
                        } else {
                            catalog.more_with_visitor(
                                &token,
                                false,
                                page.visitor_data.as_deref(),
                            )?
                        };
                        page.tracks.extend(next.tracks);
                        page.continuation = next.continuation;
                        page.visitor_data = next.visitor_data.or(page.visitor_data);
                    }
                    filter_tracks(&mut page.tracks, &filter, sort);
                    Ok(page.tracks)
                })()
                .map_err(|e| e.to_string());
                if current() && collection_auth.current(snapshot.generation) {
                    let _ = collection_events.send(Event::Collection(epoch, download, result));
                }
            }
        });
        thread::spawn(move || {
            let Ok(mut catalog) = Catalog::new(None) else {
                return;
            };
            while let Ok((generation, seed)) = radio_jobs.recv() {
                if raag::cache::offline_only() {
                    let _ = radio_events.send(Event::Radio(
                        generation,
                        Err("Radio is unavailable offline.".into()),
                    ));
                    continue;
                }
                let snapshot = radio_auth.snapshot();
                catalog.set_session(snapshot.session.clone());
                let result = catalog
                    .radio(&seed.id)
                    .map(|page| page.tracks)
                    .map_err(|error| error.to_string());
                if radio_auth.current(snapshot.generation) {
                    let _ = radio_events.send(Event::Radio(generation, result));
                }
            }
        });
        let latest = Arc::new(AtomicU64::new(0));
        let cached_events = events.clone();
        let requested = latest.clone();
        // Disk hits must not wait behind bootstrap or a slow HTTP request.
        let directory = data_dir();
        let dispatch_auth = auth.clone();
        thread::spawn(move || {
            dispatch_requests(
                requests_rx,
                network,
                cached_events,
                requested,
                directory,
                dispatch_auth,
            )
        });
        let responses = events.clone();
        let lyrics_auth = auth.clone();
        thread::spawn(move || {
            let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Catalog);
            let mut catalog = match Catalog::new(None) {
                Ok(catalog) => catalog,
                Err(error) => {
                    let _ = responses.send(Event::Failure(0, error.to_string()));
                    return;
                }
            };
            let mut bootstrapped = false;
            while let Ok(mut job) = jobs.recv() {
                while let Ok(newer) = jobs.try_recv() {
                    job = newer;
                }
                let (request, saved_path, has_cache, snapshot) = job;
                if raag::cache::offline_only() {
                    if !has_cache {
                        let _ = responses.send(Event::Failure(
                            request_revision(&request),
                            "This page is unavailable offline.".into(),
                        ));
                    }
                    continue;
                }
                if request_revision(&request) != latest.load(Ordering::Acquire)
                    || !auth.current(snapshot.generation)
                {
                    continue;
                }
                catalog.set_session(snapshot.session);
                if !bootstrapped {
                    catalog.bootstrap();
                    bootstrapped = true;
                }
                if request_revision(&request) != latest.load(Ordering::Acquire)
                    || !auth.current(snapshot.generation)
                {
                    continue;
                }
                let (revision, result, append) = match request {
                    Request::Home(id) => (id, catalog.home(), false),
                    Request::Explore(id) => (id, catalog.explore(), false),
                    Request::Search(id, query, filter) => {
                        (id, catalog.search_filtered(&query, filter), false)
                    }
                    Request::Browse(id, browse_id, title) => {
                        (id, catalog.browse(&browse_id, &title), false)
                    }
                    Request::Discover(id, browse_id, params, title) => {
                        (id, catalog.discover(&browse_id, &params, &title), false)
                    }
                    Request::More(id, token, search, visitor) => (
                        id,
                        catalog.more_with_visitor(&token, search, visitor.as_deref()),
                        true,
                    ),
                    Request::Radio(id, video) => (id, catalog.radio(&video), false),
                    Request::Artist(id, name) => (id, catalog.artist(&name), false),
                };
                match result {
                    Ok(page) => {
                        if !auth.current(snapshot.generation) {
                            continue;
                        }
                        if !append {
                            let _ = raag::cache::write_json(&saved_path, &page);
                            let _ = raag::cache::maintain_metadata(&saved_path);
                        }
                        if revision == latest.load(Ordering::Acquire) {
                            fetch_page_artwork(&page, responses.clone());
                            let _ = responses.send(Event::Page(revision, page, append));
                        }
                    }
                    Err(error)
                        if !has_cache
                            && revision == latest.load(Ordering::Acquire)
                            && auth.current(snapshot.generation) =>
                    {
                        let _ = responses.send(Event::Failure(revision, format!("{error:#}")));
                    }
                    Err(_) => {}
                }
            }
        });
        let (lyrics, lyric_jobs) = jobs::channel::<(String, Track, raag::lyrics::Source, bool)>(1);
        let lyric_events = events.clone();
        thread::spawn(move || {
            let mut youtube = None;
            while let Ok((mut id, mut track, mut source, mut force)) = lyric_jobs.recv() {
                while let Ok(newer) = lyric_jobs.try_recv() {
                    (id, track, source, force) = newer;
                }
                let path = data_dir().join("lyrics").join(format!(
                    "{}.json",
                    raag::cache::key(&format!("{}:{source:?}:v2", track.id))
                ));
                let snapshot = lyrics_auth.snapshot();
                let saved: Option<raag::lyrics::Lyrics> = (track.kind != ItemKind::Local)
                    .then(|| {
                        std::fs::File::open(&path).ok().or_else(|| {
                            (source == raag::lyrics::Source::Auto)
                                .then(|| {
                                    std::fs::File::open(
                                        data_dir()
                                            .join("lyrics")
                                            .join(format!("{}.json", raag::cache::key(&track.id))),
                                    )
                                    .ok()
                                })
                                .flatten()
                        })
                    })
                    .flatten()
                    .and_then(|file| raag::resource::json(file, raag::resource::LYRIC_BYTES).ok())
                    .filter(|lyrics: &raag::lyrics::Lyrics| {
                        !lyrics.instrumental || raag::cache::offline_only()
                    });
                let has_override = data_dir()
                    .join("lyric-overrides")
                    .join(format!("{}.json", raag::cache::key(&track.id)))
                    .is_file();
                let fresh = !force
                    && !has_override
                    && saved.is_some()
                    && (raag::cache::offline_only()
                        || raag::cache::metadata_fresh(&path, Duration::from_secs(12 * 3600)));
                if !fresh && let Some(lyrics) = &saved {
                    let _ = lyric_events.send(Event::Lyrics(id.clone(), Ok(lyrics.clone())));
                }
                let result = saved.clone().filter(|_| fresh).map(Ok).unwrap_or_else(|| {
                    if raag::cache::offline_only() && !has_override && track.kind != ItemKind::Local
                    {
                        return Err("Lyrics are not cached for offline listening.".into());
                    }
                    let fetched = raag::lyrics::fetch_selected(&track, source);
                    // A provider's instrumental flag is not proof of missing vocals.
                    // Let YouTube supply lyrics before accepting a negative result.
                    let result = if !matches!(
                        source,
                        raag::lyrics::Source::Auto | raag::lyrics::Source::YouTube
                    ) || (fetched
                        .as_ref()
                        .is_ok_and(|lyrics| !lyrics.lines.is_empty())
                        || track.kind != ItemKind::Song
                        || !lyrics_auth.current(snapshot.generation))
                    {
                        fetched
                    } else {
                        if youtube.is_none() {
                            youtube = Catalog::new(None).ok();
                        }
                        if let Some(catalog) = &mut youtube {
                            catalog.set_session(snapshot.session.clone());
                            catalog.youtube_lyrics(&track).or(fetched)
                        } else {
                            fetched
                        }
                    };
                    result.map_err(|e| e.to_string())
                });
                if !lyrics_auth.current(snapshot.generation) {
                    continue;
                }
                if !fresh
                    && !has_override
                    && track.kind != ItemKind::Local
                    && let Ok(lyrics) = &result
                {
                    let _ = raag::cache::write_json(&path, lyrics);
                    let _ = raag::cache::maintain_metadata(&path);
                }
                let _ = lyric_events.send(Event::Lyrics(
                    id,
                    result.or_else(|error| saved.ok_or(error)),
                ));
            }
        });
        let (saves, save_jobs) = jobs::channel::<SaveJob>(1);
        let saved = events.clone();
        thread::spawn(move || {
            while let Ok(mut job) = save_jobs.recv() {
                // Coalesce slider/playlist bursts in a bounded window. A flush
                // is a barrier and bypasses this delay, including on shutdown.
                let deadline = Instant::now() + Duration::from_millis(180);
                while matches!(job, SaveJob::Save(_)) {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match save_jobs.recv_timeout(remaining) {
                        Ok(next) => job = next,
                        Err(_) => break,
                    }
                }
                let (library, acknowledgment) = match job {
                    SaveJob::Save(library) => (library, None),
                    SaveJob::Flush(library, acknowledgment) => (library, Some(acknowledgment)),
                };
                let result = library
                    .save(&data_dir().join("library.json"))
                    .map_err(|e| format!("{e:#}"));
                if let Some(acknowledgment) = acknowledgment {
                    let _ = acknowledgment.send(());
                }
                let _ = saved.send(Event::Saved(result));
            }
        });
        let (caches, cache_jobs) = jobs::channel::<(Track, bool, String, String)>(32);
        let cache_events = events.clone();
        thread::spawn(move || {
            let _ = raag::cache::maintain_audio();
            let _ = cache_events.send(Event::Offline(raag::cache::offline_tracks()));
            while let Ok((track, persistent, ytdlp, cookies)) = cache_jobs.recv() {
                let result = raag::cache::store_audio(track.clone(), persistent, &ytdlp, &cookies);
                if result.is_ok() {
                    fetch_artwork(vec![track], cache_events.clone());
                }
                let _ = cache_events.send(Event::CacheStatus(
                    result.err().map(|error| error.to_string()),
                ));
                let _ = cache_events.send(Event::Offline(raag::cache::offline_tracks()));
            }
        });
        Self {
            requests,
            events: results,
            events_sender: events,
            saves,
            caches,
            lyrics,
            radio,
            suggestions,
            collections,
            collection_epoch,
            download_epoch,
        }
    }
}

fn dispatch_requests(
    requests: JobReceiver<Request>,
    network: JobSender<(Request, PathBuf, bool, Snapshot)>,
    events: SyncSender<Event>,
    latest: Arc<AtomicU64>,
    directory: PathBuf,
    auth: SessionAccess,
) {
    let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Catalog);
    while let Ok(mut request) = requests.recv() {
        while let Ok(newer) = requests.try_recv() {
            request = newer;
        }
        let revision = request_revision(&request);
        latest.store(revision, Ordering::Release);
        let snapshot = auth.snapshot();
        let path = directory.join("pages").join(snapshot.scope()).join(format!(
            "{}.json",
            raag::cache::key(&request_cache_key(&request))
        ));
        let cached: Option<Page> = if matches!(request, Request::More(..)) {
            None
        } else {
            std::fs::File::open(&path)
                .ok()
                .and_then(|file| raag::resource::json(file, raag::resource::PAGE_BYTES).ok())
        };
        let has_cache = cached.is_some();
        if let Some(page) = cached
            && auth.current(snapshot.generation)
        {
            fetch_page_artwork(&page, events.clone());
            let _ = events.send(Event::Page(revision, page, false));
        }
        if has_cache
            && (raag::cache::offline_only()
                || raag::cache::metadata_fresh(&path, request_ttl(&request)))
        {
            continue;
        }
        if network.send((request, path, has_cache, snapshot)).is_err() {
            break;
        }
    }
}

fn request_ttl(request: &Request) -> Duration {
    Duration::from_secs(match request {
        Request::Home(_) => 120,
        Request::Explore(_) | Request::Search(..) | Request::Browse(..) => 600,
        Request::Discover(..) | Request::Artist(..) => 1800,
        Request::More(..) | Request::Radio(..) => 0,
    })
}

fn request_cache_key(request: &Request) -> String {
    match request {
        Request::Home(_) => "home".into(),
        Request::Explore(_) => "explore".into(),
        Request::Search(_, query, kind) => format!("search:{query}:{kind:?}"),
        Request::Browse(_, id, _) => format!("browse:{id}"),
        Request::Discover(_, id, params, _) => format!("browse:{id}:{params}"),
        Request::Radio(_, id) => format!("radio:{id}"),
        Request::Artist(_, name) => format!("artist:{name}"),
        Request::More(_, token, ..) => format!("more:{token}"),
    }
}
fn request_revision(request: &Request) -> u64 {
    match request {
        Request::Home(id)
        | Request::Explore(id)
        | Request::Search(id, ..)
        | Request::Browse(id, ..)
        | Request::Discover(id, ..)
        | Request::More(id, ..)
        | Request::Radio(id, ..)
        | Request::Artist(id, ..) => *id,
    }
}
fn fetch_page_artwork(page: &Page, events: SyncSender<Event>) {
    // Initial viewport only. Clipped/offscreen covers are scheduled by prepaint.
    let mut tracks: Vec<_> = page.tracks.iter().take(9).cloned().collect();
    if let Some(url) = &page.artwork {
        let mut hero = Track::song(format!("page-hero:{url}"), &page.title, "");
        hero.artwork = Some(url.clone());
        tracks.insert(0, hero);
    }
    if !tracks.is_empty() {
        fetch_artwork(tracks, events);
    }
}
// A bounded worker pool avoids one new network/decode thread per page and deduplicates
// disk content by URL. Restore disk hits together before waiting for any network.
type ArtworkJob = (u8, Vec<Track>, SyncSender<Event>);
type ArtworkDownload = (u8, Track, SyncSender<Event>);
type ArtworkQueue = Arc<(
    Mutex<std::collections::VecDeque<ArtworkDownload>>,
    std::sync::Condvar,
)>;
pub fn fetch_artwork(tracks: Vec<Track>, events: SyncSender<Event>) {
    submit_artwork(tracks, events, 1);
}
pub fn cache_artwork(track: &Track, events: SyncSender<Event>) {
    let tracks = restore_artwork(vec![track.clone()], &events);
    if !tracks.is_empty()
        && let Ok(client) = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(6))
            .connect_timeout(Duration::from_secs(2))
            .build()
    {
        artwork_job(tracks, events, &client);
    }
}
pub fn fetch_visible_artwork(tracks: Vec<Track>, events: SyncSender<Event>) {
    submit_artwork(tracks, events, 0);
}
fn submit_artwork(tracks: Vec<Track>, events: SyncSender<Event>, priority: u8) {
    let mut seen = std::collections::HashSet::new();
    let tracks = tracks
        .into_iter()
        .filter(|track| seen.insert(track.artwork_key().to_owned()))
        .take(192)
        .collect();
    static WORKERS: OnceLock<JobSender<ArtworkJob>> = OnceLock::new();
    let sender = WORKERS.get_or_init(|| {
        let queue: ArtworkQueue = Arc::default();
        for _ in 0..2 {
            let queue = queue.clone();
            thread::spawn(move || {
                let Ok(client) = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(6))
                    .connect_timeout(Duration::from_secs(2))
                    .build()
                else {
                    return;
                };
                loop {
                    let (lock, ready) = &*queue;
                    let mut jobs = lock.lock().unwrap();
                    while jobs.is_empty() {
                        jobs = ready.wait(jobs).unwrap();
                    }
                    let (_, track, events) = jobs.pop_front().unwrap();
                    drop(jobs);
                    // Yield between network images so
                    // newly selected songs/pages take priority over an old shelf.
                    artwork_job(vec![track], events, &client);
                }
            });
        }
        let (sender, jobs) = jobs::channel::<ArtworkJob>(8);
        // Cached covers/palettes never wait behind a slow image download.
        thread::spawn(move || {
            while let Ok((priority, tracks, events)) = jobs.recv() {
                let missing = restore_artwork(tracks, &events);
                if !missing.is_empty() {
                    let mut pending = queue.0.lock().unwrap();
                    for track in missing {
                        if let Some((rank, _, _)) = pending
                            .iter_mut()
                            .find(|(_, queued, _)| queued.artwork_key() == track.artwork_key())
                        {
                            *rank = (*rank).min(priority);
                            continue;
                        }
                        if pending.len() >= 512 {
                            if let Some(index) = pending.iter().rposition(|job| job.0 > priority) {
                                pending.remove(index);
                            } else {
                                continue;
                            }
                        }
                        pending.push_back((priority, track, events.clone()));
                    }
                    pending.make_contiguous().sort_by_key(|job| job.0);
                    queue.1.notify_all();
                }
            }
        });
        sender
    });
    let _ = sender.send((priority, tracks, events));
}
fn restore_artwork(tracks: Vec<Track>, events: &SyncSender<Event>) -> Vec<Track> {
    let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Artwork);
    let directory = data_dir().join("artwork");
    let _ = std::fs::create_dir_all(&directory);
    let mut palettes = HashMap::new();
    let mut hits = HashMap::new();
    let mut missing = Vec::new();
    for mut track in tracks {
        if track.artwork.is_none()
            && track.kind == ItemKind::Song
            && raag::catalog::validate_video_id(&track.id).is_ok()
        {
            track.artwork = Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", track.id));
        }
        let Some(url) = &track.artwork else {
            continue;
        };
        if !url.starts_with("https://") && !url.starts_with("//") {
            continue;
        }
        let path = raag::cache::artwork_path(url);
        // Reuse covers cached before thumbnail URL normalization was introduced.
        let legacy = directory.join(format!("{}.jpg", raag::cache::key(url)));
        if !path.is_file() && legacy.is_file() {
            let _ = std::fs::rename(&legacy, &path);
        }
        if path.is_file() {
            let dimensions = image::image_dimensions(&path).ok();
            let wrong_banner_shape = track.id.starts_with("page-hero:")
                && raag::artwork::aspect_ratio(url)
                    .zip(dimensions)
                    .is_some_and(|(ratio, (width, height))| {
                        (width as f32 / height.max(1) as f32 / ratio - 1.).abs() > 0.05
                    });
            if dimensions.is_none() || wrong_banner_shape {
                discard_artwork(&path);
                missing.push(track);
                continue;
            }
            let Some(colors) = artwork_colors(&path) else {
                // A truncated/invalid image must not remain a permanent placeholder.
                discard_artwork(&path);
                missing.push(track);
                continue;
            };
            if track.id.starts_with("page-hero:") {
                save_artist_backdrop(&path);
            }
            palettes.insert(track.id.clone(), colors);
            palettes.insert(track.artwork_key().to_owned(), colors);
            hits.insert(track.id.clone(), path.clone());
            hits.insert(track.artwork_key().to_owned(), path);
        } else {
            missing.push(track);
        }
    }
    if !palettes.is_empty() {
        let _ = events.send(Event::ArtworkColors(std::mem::take(&mut palettes)));
    }
    if !hits.is_empty() {
        let _ = events.send(artwork_event(hits));
    }
    missing
}
fn discard_artwork(path: &std::path::Path) {
    for extension in [
        "jpg",
        "soft.jpg",
        "128.jpg",
        "256.jpg",
        "hero.jpg",
        "hero-v2.jpg",
        "palette.json",
    ] {
        let _ = std::fs::remove_file(path.with_extension(extension));
    }
}
fn artwork_job(
    tracks: Vec<Track>,
    events: SyncSender<Event>,
    client: &reqwest::blocking::Client,
) -> Vec<Track> {
    let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Artwork);
    let mut palettes = HashMap::new();
    let mut missing = tracks;
    let mut images = HashMap::new();
    let remaining = if missing.len() > 1 {
        missing.split_off(1)
    } else {
        Vec::new()
    };
    for track in missing {
        let url = track.artwork.as_ref().unwrap();
        let path = raag::cache::artwork_path(url);
        let pixels = if track.id.starts_with("page-hero:") {
            1024
        } else {
            512
        };
        if ensure_artwork(url, &path, client, pixels).is_none() {
            continue;
        }
        if track.id.starts_with("page-hero:") {
            save_artist_backdrop(&path);
        }
        if let Some(colors) = artwork_colors(&path) {
            palettes.insert(track.id.clone(), colors);
            palettes.insert(track.artwork_key().to_owned(), colors);
        }
        images.insert(track.id.clone(), path.clone());
        images.insert(track.artwork_key().to_owned(), path);
    }
    if !palettes.is_empty() {
        let _ = events.send(Event::ArtworkColors(palettes));
    }
    if !images.is_empty() {
        let _ = events.send(artwork_event(images));
    }
    if remaining.is_empty() {
        let _ = raag::cache::maintain_artwork();
    }
    remaining
}

fn artwork_event(images: HashMap<String, PathBuf>) -> Event {
    // Resolve available derivatives on the artwork worker, never during a
    // sidebar animation or playback-clock repaint on the UI thread.
    let mut variants = HashMap::new();
    for path in images.values() {
        variants.entry(path.clone()).or_insert_with(|| {
            ["128.jpg", "256.jpg", "hero-v2.jpg"]
                .iter()
                .enumerate()
                .fold(0, |mask, (index, extension)| {
                    mask | (u8::from(path.with_extension(extension).is_file()) << index)
                })
        });
    }
    Event::Artwork(images, variants)
}

fn ensure_artwork(
    url: &str,
    path: &std::path::Path,
    client: &reqwest::blocking::Client,
    pixels: u32,
) -> Option<()> {
    let _permit = ArtworkPermit::acquire(raag::artwork::identity(url));
    if !path.exists() {
        if raag::cache::offline_only() {
            return None;
        }
        use std::io::Read;
        let sized = raag::artwork::at_size(url, pixels);
        let original = if url.starts_with("//") {
            format!("https:{url}")
        } else {
            url.to_owned()
        };
        static POLICY: OnceLock<raag::request_policy::RequestPolicy> = OnceLock::new();
        let policy = POLICY.get_or_init(Default::default);
        let mut decoded = None;
        for (attempt, address) in [&sized, &original].into_iter().enumerate() {
            policy.before().ok()?;
            let result = client.get(address).send();
            if let Ok(value) = &result {
                policy.observe(value.status(), value.headers());
            }
            if result
                .as_ref()
                .is_ok_and(|value| value.status().as_u16() == 429)
            {
                return None;
            }
            let transient = match &result {
                Ok(value) => matches!(value.status().as_u16(), 502..=504),
                Err(error) => error.is_timeout() || error.is_connect(),
            };
            if transient && attempt == 0 {
                thread::sleep(Duration::from_millis(400));
                continue;
            }
            let Some(response) = result.ok().and_then(|value| value.error_for_status().ok()) else {
                if sized == original && !transient {
                    return None;
                }
                continue;
            };
            let mut bytes = Vec::new();
            if response.take(2_000_001).read_to_end(&mut bytes).is_ok() && bytes.len() <= 2_000_000
            {
                decoded = decode_artwork(&bytes).ok();
                if decoded.is_some() {
                    break;
                }
            }
            if sized == original {
                return None;
            }
        }
        let decoded = decoded?;
        let decoded = decoded.thumbnail(1024, 1024);
        let staging = path.with_extension(format!("{:x}.tmp.jpg", rand::random::<u64>()));
        if decoded.to_rgb8().save(&staging).is_err() {
            let _ = std::fs::remove_file(&staging);
            return None;
        }
        if std::fs::rename(&staging, path).is_err() {
            let _ = std::fs::remove_file(staging);
            return None;
        }
        save_soft_artwork(&decoded, path);
    }
    Some(())
}

// Both download workers may encounter the same album from different pages.
// Share the completed file instead of issuing duplicate HTTP requests.
struct ArtworkPermit(String);
fn active_artwork() -> &'static (Mutex<std::collections::HashSet<String>>, std::sync::Condvar) {
    static ACTIVE: OnceLock<(Mutex<std::collections::HashSet<String>>, std::sync::Condvar)> =
        OnceLock::new();
    ACTIVE.get_or_init(Default::default)
}
impl ArtworkPermit {
    fn acquire(key: &str) -> Self {
        let (lock, ready) = active_artwork();
        let mut active = lock.lock().unwrap();
        while active.contains(key) {
            active = ready.wait(active).unwrap();
        }
        active.insert(key.to_owned());
        Self(key.to_owned())
    }
}
impl Drop for ArtworkPermit {
    fn drop(&mut self) {
        let (lock, ready) = active_artwork();
        lock.lock().unwrap().remove(&self.0);
        ready.notify_all();
    }
}

fn decode_artwork(bytes: &[u8]) -> image::ImageResult<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode()
}
fn save_soft_artwork(decoded: &image::DynamicImage, path: &std::path::Path) {
    let target = path.with_extension("soft.jpg");
    let staging = path.with_extension(format!("{:x}.soft.tmp.jpg", rand::random::<u64>()));
    if decoded
        .thumbnail(128, 128)
        .blur(8.)
        .to_rgb8()
        .save(&staging)
        .is_ok()
    {
        let _ = std::fs::rename(&staging, target);
    }
    let _ = std::fs::remove_file(staging);
    for extent in [128, 256] {
        let target = path.with_extension(format!("{extent}.jpg"));
        if image::image_dimensions(&target).is_err() {
            let _ = std::fs::remove_file(&target);
            let staging =
                path.with_extension(format!("{:x}.{extent}.tmp.jpg", rand::random::<u64>()));
            if decoded
                .thumbnail(extent, extent)
                .to_rgb8()
                .save(&staging)
                .is_ok()
            {
                let _ = std::fs::rename(&staging, target);
            }
            let _ = std::fs::remove_file(staging);
        }
    }
}
fn save_artist_backdrop(path: &std::path::Path) {
    let target = path.with_extension("hero-v2.jpg");
    if image::image_dimensions(&target).ok() == Some((1024, 384)) {
        return;
    }
    let _ = std::fs::remove_file(&target);
    let Ok(decoded) = image::open(path) else {
        return;
    };
    let staging = path.with_extension(format!("{:x}.hero.tmp.jpg", rand::random::<u64>()));
    if raag::artwork::artist_backdrop(&decoded)
        .to_rgb8()
        .save(&staging)
        .is_ok()
    {
        let _ = std::fs::rename(&staging, target);
    }
    let _ = std::fs::remove_file(staging);
}
fn artwork_colors(path: &std::path::Path) -> Option<raag::artwork::ArtworkColors> {
    let metadata = path.with_extension("palette.json");
    let saved: Option<raag::artwork::ArtworkColors> = std::fs::read(&metadata)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let variants_ready = ["soft.jpg", "128.jpg", "256.jpg"]
        .iter()
        .all(|extension| image::image_dimensions(path.with_extension(extension)).is_ok());
    if saved.is_some() && variants_ready {
        return saved;
    }
    let decoded = image::open(path).ok()?;
    let colors = saved.unwrap_or_else(|| raag::artwork::extract(&decoded));
    if saved.is_none() {
        let _ = raag::cache::write_json(&metadata, &colors);
    }
    if !variants_ready {
        save_soft_artwork(&decoded, path);
    }
    Some(colors)
}
