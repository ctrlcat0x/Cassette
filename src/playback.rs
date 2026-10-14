//! Playback coordination, source resolution and native audio preparation.
use crate::native_audio::{Command as AudioCommand, NativeSession};
use crate::{ItemKind, Track, catalog::validate_video_id};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
static STREAM_QUALITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);

#[derive(Clone, Debug)]
pub enum PlayerCommand {
    Load(Track, f64),
    Prefetch(Track),
    Pause(bool),
    Seek(f64),
    Volume(f64),
    Configure(crate::audio_settings::AudioSettings),
    Stop,
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum PlayerEvent {
    EngineReady,
    Devices(Vec<(String, String)>),
    Source(StreamQuality, bool),
    Loaded,
    Prepared(String),
    Position(f64),
    Duration(f64),
    Paused(bool),
    Buffering(bool),
    Ended,
    Error(String),
    SettingsError(String),
}

#[derive(Clone, Debug)]
pub struct PlaybackEvent {
    pub generation: u64,
    pub event: PlayerEvent,
}

#[derive(Clone)]
pub struct AudioSource {
    pub url: String,
    pub headers: Vec<String>,
    pub user_agent: String,
    pub quality: StreamQuality,
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct StreamQuality {
    #[serde(default)]
    pub source: String,
    pub codec: String,
    pub bitrate: f64,
    pub sample_rate: u64,
    pub protocol: String,
}

pub struct Player {
    commands: Sender<(u64, PlayerCommand)>,
    pub events: Receiver<PlaybackEvent>,
    generation: Arc<AtomicU64>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Player {
    pub fn new(ytdlp: String) -> Self {
        Self::with_cookies(ytdlp, String::new())
    }

    pub fn with_cookies(ytdlp: String, cookies: String) -> Self {
        let (commands, requests) = mpsc::channel();
        let (events, responses) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current_generation = generation.clone();
        let (resolved, sources) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _domain = crate::diagnostics::enter(crate::diagnostics::Domain::Playback);
            let mut session: Option<PreparedAudio> = None;
            let mut prepared: Option<PreparedAudio> = None;
            let mut wanted_prefetch: Option<String> = None;
            let mut volume = 70.;
            let mut settings = crate::audio_settings::AudioSettings::default();
            let mut fading: Option<(PreparedAudio, Instant)> = None;
            let mut fade_tick = Instant::now();
            let mut loading_track = None;
            let mut source_retried = false;
            let mut awaiting_audio = false;
            let extractor = ytdlp.clone();
            let warm_generation = current_generation.clone();
            let mut resolvers = vec![thread::spawn(move || {
                warm_extractor(&extractor, &warm_generation);
            })];
            // Own one silent, connected engine from startup. First playback uses
            // this exact engine instead of initializing it after extraction.
            let mut idle = IdleAudio::start().ok();
            if idle.is_some() {
                let _ = events.send(PlaybackEvent {
                    generation: 0,
                    event: PlayerEvent::EngineReady,
                });
            }
            loop {
                if let Some(ready) = &mut idle {
                    while let Ok(event) = ready.events.try_recv() {
                        if matches!(
                            event.event,
                            PlayerEvent::Devices(_) | PlayerEvent::SettingsError(_)
                        ) {
                            let _ = events.send(PlaybackEvent {
                                generation: current_generation.load(Ordering::Acquire),
                                event: event.event,
                            });
                        }
                    }
                }
                // Commands wake recv immediately. Poll the resolver/event queues
                // quickly only while audio work exists, not 100 times/sec at rest.
                let wait = if loading_track.is_some() || awaiting_audio {
                    Duration::from_millis(10)
                } else if session.is_none() && prepared.is_none() {
                    Duration::from_millis(200)
                } else {
                    // Commands still wake immediately. Steady playback/event
                    // forwarding needs no 100 Hz polling; fades tick at 30 Hz.
                    Duration::from_millis(33)
                };
                match requests.recv_timeout(wait) {
                    Ok((id, PlayerCommand::Load(track, level))) => {
                        if id != current_generation.load(Ordering::Acquire) {
                            continue;
                        }
                        loading_track = Some(track.clone());
                        awaiting_audio = true;
                        crate::cache::prioritize_playback(Duration::from_secs(35));
                        source_retried = false;
                        wanted_prefetch = None;
                        fading = None;
                        if settings.crossfade > 0.
                            && prepared.as_ref().is_some_and(|p| {
                                p.track_id == track.id && p.engine.loaded.load(Ordering::Acquire)
                            })
                        {
                            fading = session.take().map(|old| (old, Instant::now()));
                        } else {
                            session = None;
                        }
                        volume = level;
                        if prepared.as_mut().is_some_and(|ready| {
                            ready.track_id == track.id && ready.drain_preparation()
                        }) {
                            let mut ready = prepared.take().unwrap();
                            ready.drain_preparation();
                            ready.engine.generation = id;
                            configure(&mut ready.engine, &settings);
                            let _ = events.send(PlaybackEvent {
                                generation: id,
                                event: PlayerEvent::Source(
                                    ready.source.quality.clone(),
                                    !ready.source.url.starts_with("https://"),
                                ),
                            });
                            if ready.engine.loaded.load(Ordering::Acquire) {
                                loading_track = None;
                                ready.loaded_forwarded = true;
                                let _ = events.send(PlaybackEvent {
                                    generation: id,
                                    event: PlayerEvent::Loaded,
                                });
                                if ready.duration > 0. {
                                    let _ = events.send(PlaybackEvent {
                                        generation: id,
                                        event: PlayerEvent::Duration(ready.duration),
                                    });
                                }
                            }
                            let _ = ready.engine.send(AudioCommand::Volume(if fading.is_some() {
                                0.
                            } else {
                                volume
                            }));
                            let _ = ready.engine.send(AudioCommand::Pause(false));
                            session = Some(ready);
                        } else {
                            prepared = None;
                            let sender = resolved.clone();
                            let ytdlp = ytdlp.clone();
                            let cookies = cookies.clone();
                            let generation = current_generation.clone();
                            resolvers.retain(|task: &thread::JoinHandle<()>| !task.is_finished());
                            resolvers.push(thread::spawn(move || {
                                let source = resolve(&track, &ytdlp, &cookies, id, &generation);
                                let _ = sender.send((id, track.id, false, source));
                            }));
                        }
                    }
                    Ok((id, PlayerCommand::Prefetch(track))) => {
                        if id != current_generation.load(Ordering::Acquire) {
                            continue;
                        }
                        wanted_prefetch = Some(track.id.clone());
                        if prepared
                            .as_ref()
                            .is_some_and(|ready| ready.track_id == track.id)
                        {
                            continue;
                        }
                        prepared = None;
                        let sender = resolved.clone();
                        let ytdlp = ytdlp.clone();
                        let cookies = cookies.clone();
                        let generation = current_generation.clone();
                        resolvers.retain(|task: &thread::JoinHandle<()>| !task.is_finished());
                        resolvers.push(thread::spawn(move || {
                            let source = resolve(&track, &ytdlp, &cookies, id, &generation);
                            let _ = sender.send((id, track.id, true, source));
                        }));
                    }
                    Ok((_, PlayerCommand::Shutdown))
                    | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok((_, PlayerCommand::Configure(mut value))) => {
                        value.validate();
                        crate::jiosaavn::configure(value.jiosaavn);
                        STREAM_QUALITY.store(value.streaming_quality.level(), Ordering::Relaxed);
                        crate::cache::configure_download_quality(value.download_quality);
                        settings = value;
                        if let Some(ready) = &mut session {
                            configure(&mut ready.engine, &settings);
                        }
                        if let Some(ready) = &mut prepared {
                            configure(&mut ready.engine, &settings);
                        }
                        if let Some(ready) = &mut idle {
                            configure(&mut ready.engine, &settings);
                        }
                    }
                    Ok((id, command)) => {
                        if id != current_generation.load(Ordering::Acquire) {
                            continue;
                        }
                        if let PlayerCommand::Volume(level) = command {
                            volume = level;
                        }
                        if matches!(command, PlayerCommand::Stop) {
                            fading = None;
                            loading_track = None;
                            session = None;
                            prepared = None;
                            continue;
                        }
                        if matches!(command, PlayerCommand::Pause(_) | PlayerCommand::Seek(_)) {
                            fading = None;
                            if let Some(ready) = &mut session {
                                let _ = ready.engine.send(AudioCommand::Volume(volume));
                            }
                        }
                        if let Some(ready) = &mut session {
                            let message = match command {
                                PlayerCommand::Pause(paused) => AudioCommand::Pause(paused),
                                PlayerCommand::Seek(seconds) => AudioCommand::Seek(seconds),
                                PlayerCommand::Volume(volume) => AudioCommand::Volume(volume),
                                _ => unreachable!(),
                            };
                            if let Err(error) = ready.engine.send(message) {
                                let _ = events.send(PlaybackEvent {
                                    generation: id,
                                    event: PlayerEvent::Error(format!("{error:#}")),
                                });
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                while let Ok((id, track_id, prefetch, source)) = sources.try_recv() {
                    if prefetch && wanted_prefetch.as_ref() != Some(&track_id) {
                        continue;
                    }
                    if id != current_generation.load(Ordering::Acquire) {
                        continue;
                    }
                    let result = source.and_then(|source| {
                        PreparedAudio::load(
                            track_id,
                            source,
                            id,
                            if prefetch { 0. } else { volume },
                            prefetch,
                            if prefetch { None } else { idle.take() },
                        )
                    });
                    match result {
                        Ok(mut ready) if prefetch => {
                            configure(&mut ready.engine, &settings);
                            prepared = Some(ready);
                        }
                        Ok(mut ready) => {
                            configure(&mut ready.engine, &settings);
                            let _ = events.send(PlaybackEvent {
                                generation: id,
                                event: PlayerEvent::Source(
                                    ready.source.quality.clone(),
                                    !ready.source.url.starts_with("https://"),
                                ),
                            });
                            session = Some(ready);
                        }
                        Err(error) if !prefetch => {
                            loading_track = None;
                            awaiting_audio = false;
                            crate::cache::prioritize_playback(Duration::ZERO);
                            let _ = events.send(PlaybackEvent {
                                generation: id,
                                event: PlayerEvent::Error(format!("{error:#}")),
                            });
                        }
                        Err(_) => {} // Background preparation must not interrupt the current song.
                    }
                }
                if let Some((old, since)) = &mut fading
                    && fade_tick.elapsed() >= Duration::from_millis(33)
                {
                    fade_tick = Instant::now();
                    let ratio =
                        (since.elapsed().as_secs_f64() / settings.crossfade.max(0.01)).min(1.);
                    let _ = old.engine.send(AudioCommand::Volume(volume * (1. - ratio)));
                    if let Some(ready) = &mut session {
                        let _ = ready.engine.send(AudioCommand::Volume(volume * ratio));
                    }
                    while old.events.try_recv().is_ok() {}
                    if ratio >= 1. {
                        fading = None;
                    }
                }
                if let Some(ready) = &mut prepared {
                    if !ready.drain_preparation() || ready.loading_expired() {
                        forget_source(&ready.track_id);
                        prepared = None;
                    } else if ready.engine.loaded.load(Ordering::Acquire) && !ready.announced {
                        ready.announced = true;
                        let _ = events.send(PlaybackEvent {
                            generation: current_generation.load(Ordering::Acquire),
                            event: PlayerEvent::Prepared(ready.track_id.clone()),
                        });
                    }
                }
                if let Some(ready) = &mut session {
                    let mut failed = false;
                    let mut ended = false;
                    let mut retry = false;
                    let session_generation = ready.engine.generation;
                    while let Ok(event) = ready.events.try_recv() {
                        if awaiting_audio
                            && matches!(&event.event, PlayerEvent::Position(position) if *position > 0.05)
                        {
                            awaiting_audio = false;
                            // Give the first audible packets time to build a useful lead.
                            crate::cache::prioritize_playback(Duration::from_secs(3));
                        }
                        failed = matches!(event.event, PlayerEvent::Error(_));
                        ended = matches!(event.event, PlayerEvent::Ended);
                        if failed {
                            let limited = matches!(&event.event, PlayerEvent::Error(message) if request_limited(&message.to_lowercase()));
                            if limited {
                                source_policy(&ready.source).pause(Duration::from_secs(60));
                            }
                            forget_source(&ready.track_id);
                            if !ready.loaded_forwarded
                                && !source_retried
                                && ready.source.url.starts_with("https://")
                                && !limited
                                && (ready.source.quality.source == "JioSaavn"
                                    || matches!(&event.event, PlayerEvent::Error(message) if message.contains("403") || message.contains("410")))
                            {
                                retry = true;
                                break;
                            }
                        }
                        if matches!(event.event, PlayerEvent::Loaded) {
                            if ready.loaded_forwarded {
                                continue;
                            }
                            ready.loaded_forwarded = true;
                            loading_track = None;
                        }
                        let _ = events.send(PlaybackEvent {
                            generation: ready.engine.generation,
                            event: event.event,
                        });
                        if failed || ended {
                            break;
                        }
                    }
                    if retry {
                        session = None;
                        source_retried = true;
                        crate::cache::prioritize_playback(Duration::from_secs(35));
                        if let Some(track) = loading_track.clone() {
                            let sender = resolved.clone();
                            let ytdlp = ytdlp.clone();
                            let cookies = cookies.clone();
                            let generation = current_generation.clone();
                            let id = session_generation;
                            resolvers.retain(|task| !task.is_finished());
                            resolvers.push(thread::spawn(move || {
                                let source = resolve(&track, &ytdlp, &cookies, id, &generation);
                                let _ = sender.send((id, track.id, false, source));
                            }));
                        }
                    } else if failed || ended {
                        loading_track = None;
                        awaiting_audio = false;
                        crate::cache::prioritize_playback(Duration::ZERO);
                        session = None;
                    } else if ready.loading_expired() {
                        loading_track = None;
                        awaiting_audio = false;
                        crate::cache::prioritize_playback(Duration::ZERO);
                        forget_source(&ready.track_id);
                        let _ = events.send(PlaybackEvent {
                            generation: ready.engine.generation,
                            event: PlayerEvent::Error(
                                "Audio did not start within 25 seconds. Retry playback.".into(),
                            ),
                        });
                        session = None;
                    }
                }
            }
            drop(session);
            drop(prepared);
            drop(idle);
            for task in resolvers {
                let _ = task.join();
            }
        });
        Self {
            commands,
            events: responses,
            generation,
            worker: Some(worker),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    pub fn command(&self, command: PlayerCommand) {
        match &command {
            PlayerCommand::Load(..) => crate::cache::prioritize_playback(Duration::from_secs(35)),
            PlayerCommand::Seek(_) => crate::cache::prioritize_playback(Duration::from_secs(5)),
            PlayerCommand::Stop | PlayerCommand::Shutdown => {
                crate::cache::prioritize_playback(Duration::ZERO);
            }
            _ => {}
        }
        let id = if matches!(
            command,
            PlayerCommand::Load(..) | PlayerCommand::Stop | PlayerCommand::Shutdown
        ) {
            self.generation.fetch_add(1, Ordering::AcqRel) + 1
        } else {
            self.generation()
        };
        let _ = self.commands.send((id, command));
    }
}

struct IdleAudio {
    engine: NativeSession,
    events: Receiver<PlaybackEvent>,
}
fn configure(engine: &mut NativeSession, settings: &crate::audio_settings::AudioSettings) {
    let _ = engine.send(AudioCommand::Configure(settings.clone()));
}
impl IdleAudio {
    fn start() -> Result<Self> {
        let (sender, events) = mpsc::channel();
        let engine = NativeSession::start(sender, 0)?;
        Ok(Self { engine, events })
    }
}

// yt-dlp's CLI cannot stay resident between extractions. Warm the executable
// and Python imports once per path; cancel immediately if playback takes priority.
fn warm_extractor(ytdlp: &str, generation: &AtomicU64) {
    static WARMED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    let path = executable("yt-dlp", ytdlp);
    {
        let mut warmed = WARMED.get_or_init(Default::default).lock().unwrap();
        if warmed.contains(&path) {
            return;
        }
        if warmed.len() >= 16 {
            warmed.clear();
        }
        warmed.insert(path.clone());
    }
    let mut command = Command::new(path);
    command
        .arg("--ignore-config")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hidden(&mut command);
    let Ok(mut child) = command.spawn() else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline && generation.load(Ordering::Acquire) == 0 => {
                thread::sleep(Duration::from_millis(40));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

struct PreparedAudio {
    track_id: String,
    source: AudioSource,
    engine: NativeSession,
    events: Receiver<PlaybackEvent>,
    duration: f64,
    loaded_forwarded: bool,
    announced: bool,
}
impl PreparedAudio {
    fn load(
        track_id: String,
        source: AudioSource,
        generation: u64,
        volume: f64,
        paused: bool,
        idle: Option<IdleAudio>,
    ) -> Result<Self> {
        let IdleAudio { mut engine, events } = idle
            .and_then(|mut idle| {
                (!idle.engine.closing.load(Ordering::Acquire) && idle.engine.is_running())
                    .then_some(idle)
            })
            .map_or_else(IdleAudio::start, Ok)?;
        while events.try_recv().is_ok() {}
        engine.generation = generation;
        engine.started = Instant::now();
        engine.loaded.store(false, Ordering::Release);
        engine.send(AudioCommand::Pause(paused))?;
        engine.send(AudioCommand::Volume(volume))?;
        engine.send(AudioCommand::Load(source.clone()))?;
        Ok(Self {
            track_id,
            source,
            engine,
            events,
            duration: 0.,
            loaded_forwarded: false,
            announced: false,
        })
    }
    fn drain_preparation(&mut self) -> bool {
        let mut healthy = true;
        while let Ok(event) = self.events.try_recv() {
            match event.event {
                PlayerEvent::Duration(duration) => self.duration = duration,
                PlayerEvent::Error(_) | PlayerEvent::Ended => healthy = false,
                _ => {}
            }
        }
        healthy
    }
    fn loading_expired(&self) -> bool {
        self.engine.started.elapsed() > Duration::from_secs(25)
            && !self.engine.loaded.load(Ordering::Acquire)
    }
}

// Short-lived memory reuse prevents extraction on replay and on background cache jobs.
// Signed URLs and headers never go to disk; cookie/tool changes have separate keys.
fn source_cache()
-> &'static std::sync::Mutex<std::collections::HashMap<String, (Instant, AudioSource)>> {
    static SOURCES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, (Instant, AudioSource)>>,
    > = std::sync::OnceLock::new();
    SOURCES.get_or_init(Default::default)
}

/// Account boundaries must discard signed sources, even when a cookie path is reused.
pub fn clear_source_cache() {
    public_routes().lock().unwrap().clear();
    if let Ok(mut sources) = source_cache().lock() {
        sources.clear();
    }
}

// At most two extractors may run, and simultaneous requests for one song share
// the completed source. Waits remain cancellable when the queue/song changes.
fn resolution_slots() -> &'static (
    std::sync::Mutex<std::collections::HashSet<String>>,
    std::sync::Condvar,
) {
    static SLOTS: std::sync::OnceLock<(
        std::sync::Mutex<std::collections::HashSet<String>>,
        std::sync::Condvar,
    )> = std::sync::OnceLock::new();
    SLOTS.get_or_init(Default::default)
}
struct ResolutionPermit(String);
impl ResolutionPermit {
    fn acquire(key: &str, id: u64, generation: &AtomicU64, deadline: Instant) -> Result<Self> {
        let (lock, ready) = resolution_slots();
        let mut active = lock.lock().unwrap();
        loop {
            if generation.load(Ordering::Acquire) != id {
                bail!("Playback request cancelled");
            }
            if Instant::now() >= deadline {
                bail!("Audio resolution is busy. Retry playback.");
            }
            if active.len() < 2 && !active.contains(key) {
                active.insert(key.to_owned());
                return Ok(Self(key.to_owned()));
            }
            active = ready
                .wait_timeout(active, Duration::from_millis(20))
                .unwrap()
                .0;
        }
    }
}
impl Drop for ResolutionPermit {
    fn drop(&mut self) {
        let (lock, ready) = resolution_slots();
        lock.lock().unwrap().remove(&self.0);
        ready.notify_all();
    }
}
pub(crate) fn forget_source(track_id: &str) {
    if let Ok(mut sources) = source_cache().lock() {
        let prefix = format!("{track_id}:");
        if sources.iter().any(|(key, (_, source))| {
            key.starts_with(&prefix) && source.quality.source == "JioSaavn"
        }) {
            crate::jiosaavn::refused(track_id);
        }
        sources.retain(|key, _| !key.starts_with(&prefix));
    }
}
fn remembered_source(key: &str) -> Option<AudioSource> {
    source_cache()
        .lock()
        .ok()?
        .get(key)
        .filter(|(at, _)| at.elapsed() < Duration::from_secs(300))
        .map(|(_, source)| source.clone())
}

/// Resolve fresh URLs outside the UI thread. No stream URLs or cookies are persisted.
pub fn resolve(
    track: &Track,
    ytdlp: &str,
    cookies: &str,
    id: u64,
    generation: &AtomicU64,
) -> Result<AudioSource> {
    resolve_with_quality(
        track,
        ytdlp,
        cookies,
        id,
        generation,
        crate::audio_settings::Quality::from_level(STREAM_QUALITY.load(Ordering::Relaxed)),
    )
}
pub fn resolve_with_quality(
    track: &Track,
    ytdlp: &str,
    cookies: &str,
    id: u64,
    generation: &AtomicU64,
    quality: crate::audio_settings::Quality,
) -> Result<AudioSource> {
    if track.kind == ItemKind::Local {
        let path = PathBuf::from(&track.id);
        if !path.is_file() {
            bail!("This local file has moved or is no longer available.");
        }
        return Ok(AudioSource {
            url: path.canonicalize()?.to_string_lossy().into_owned(),
            headers: vec![],
            user_agent: String::new(),
            quality: StreamQuality::default(),
        });
    }
    if track.kind != ItemKind::Song {
        bail!("Open this collection and choose a song to play.");
    }
    validate_video_id(&track.id)?;
    if let Some((path, quality)) = crate::cache::cached_audio(&track.id) {
        return Ok(AudioSource {
            url: path.canonicalize()?.to_string_lossy().into_owned(),
            headers: vec![],
            user_agent: String::new(),
            quality,
        });
    }
    let cache_key = format!(
        "{}:{ytdlp}:{cookies}:{}:{}",
        track.id,
        quality.level(),
        crate::jiosaavn::enabled()
    );
    if crate::cache::offline_only() {
        bail!("This song is not downloaded. Turn off offline mode to stream it.");
    }
    if let Some(source) = remembered_source(&cache_key) {
        return Ok(source);
    }
    let deadline = Instant::now() + Duration::from_secs(35);
    let permit = Arc::new(ResolutionPermit::acquire(
        &cache_key, id, generation, deadline,
    )?);
    // The other requester may have completed while this one was waiting.
    if let Some(source) = remembered_source(&cache_key) {
        return Ok(source);
    }
    // Keep Premium/account-only audio when it works. Public extraction is a
    // bounded fallback when account formats are rejected or only muxed video works.
    let public_first = public_routes()
        .lock()
        .unwrap()
        .get(cookies)
        .is_some_and(|at| at.elapsed() < Duration::from_secs(600));
    let mut substitute_pending =
        crate::jiosaavn::enabled() && !track.artist.trim().is_empty() && track.duration > 0;
    let (sender, candidates) = mpsc::channel();
    let cancel = Arc::new(AtomicU64::new(id));
    if substitute_pending {
        let track = track.clone();
        let sender = sender.clone();
        let cancel = cancel.clone();
        let permit = permit.clone();
        thread::spawn(move || {
            // Keep the shared permit until both providers stop, including on rapid skips.
            let _permit = permit;
            let result =
                crate::jiosaavn::resolve(&track, quality, id, &cancel).and_then(|source| {
                    if let Some(source) = &source {
                        if cancel.load(Ordering::Acquire) != id {
                            bail!("Playback request cancelled");
                        }
                        if let Err(error) = validate_stream(
                            source,
                            deadline.min(Instant::now() + Duration::from_secs(3)),
                        ) {
                            crate::jiosaavn::refused(&track.id);
                            return Err(error);
                        }
                    }
                    Ok(source)
                });
            let _ = sender.send((false, result));
        });
    }
    let mut youtube = Some({
        let track = track.clone();
        let ytdlp = ytdlp.to_owned();
        let cookies = cookies.to_owned();
        let cancel = cancel.clone();
        let permit = permit.clone();
        move || {
            let _permit = permit;
            let result = select_source_with_route(&cookies, public_first, |credentials| {
                youtube_policy().before()?;
                let result =
                    extract_source(&track, &ytdlp, credentials, id, &cancel, deadline, quality);
                if result.as_ref().is_err_and(|error| {
                    error.to_string().contains("rate limit")
                        || error.to_string().contains("limiting requests")
                }) {
                    youtube_policy().pause(Duration::from_secs(60));
                }
                if credentials.is_empty() && result.is_ok() && !cookies.is_empty() {
                    remember_public_route(&cookies);
                }
                result
            });
            let _ = sender.send((true, result.map(Some)));
        }
    });
    let mut youtube_error = None;
    let result = loop {
        if generation.load(Ordering::Acquire) != id {
            break Err(anyhow::anyhow!("Playback request cancelled"));
        }
        if Instant::now() >= deadline {
            break Err(anyhow::anyhow!(
                "Audio resolution took too long. Retry playback."
            ));
        }
        // Only extract YouTube audio after JioSaavn reports a miss or failure.
        if !substitute_pending && let Some(job) = youtube.take() {
            thread::spawn(job);
        }
        match candidates.recv_timeout(Duration::from_millis(20)) {
            Ok((_, Ok(Some(source)))) => break Ok(source),
            Ok((true, Err(error))) => youtube_error = Some(error),
            Ok((false, _)) => substitute_pending = false,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(anyhow::anyhow!("Audio providers stopped responding."));
            }
            _ => {}
        }
        if !substitute_pending && let Some(error) = youtube_error.take() {
            break Err(error);
        }
    };
    // Stop the losing extractor, too. Its process loop checks every 40 ms;
    // blocking HTTP remains bounded by its timeout.
    cancel.store(id.wrapping_add(1), Ordering::Release);
    let source = result?;
    if generation.load(Ordering::Acquire) != id {
        bail!("Playback request cancelled");
    }
    if let Ok(mut sources) = source_cache().lock() {
        sources.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(300));
        if sources.len() >= 32
            && let Some(oldest) = sources
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone())
        {
            sources.remove(&oldest);
        }
        sources.insert(cache_key, (Instant::now(), source.clone()));
    }
    Ok(source)
}

#[derive(Debug)]
struct RejectedFormats;
impl std::fmt::Display for RejectedFormats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("YouTube did not return a downloadable audio format. Retry playback or update the playback tools.")
    }
}
impl std::error::Error for RejectedFormats {}

fn formats_rejected(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    !request_limited(&lower)
        && !lower.contains("not a bot")
        && (lower.contains("requested format is not available") || lower.contains("http error 403"))
}

fn request_limited(message: &str) -> bool {
    message.contains("429")
        || message.contains("rate limit")
        || message.contains("too many requests")
        || message.contains("limiting requests")
        || message.contains("this content isn't available, try again later")
        || message.contains("this content isn’t available, try again later")
}

fn public_routes() -> &'static std::sync::Mutex<std::collections::HashMap<String, Instant>> {
    static ROUTES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Instant>>,
    > = std::sync::OnceLock::new();
    ROUTES.get_or_init(Default::default)
}
fn select_source_with_route(
    cookies: &str,
    public_first: bool,
    mut extract: impl FnMut(&str) -> Result<(AudioSource, bool)>,
) -> Result<AudioSource> {
    let mut public_result = if public_first {
        Some(extract(""))
    } else {
        None
    };
    if matches!(&public_result, Some(Err(error)) if !error.is::<RejectedFormats>() && !error.is::<AccountRequired>())
    {
        return public_result.take().unwrap().map(|(source, _)| source);
    }
    if matches!(&public_result, Some(Ok((_, true)))) {
        return public_result.take().unwrap().map(|(source, _)| source);
    }
    // Reuse even a failed guest result: at most one request per route.
    select_source(cookies, |credentials| {
        if credentials.is_empty()
            && let Some(result) = public_result.take()
        {
            result
        } else {
            extract(credentials)
        }
    })
}
fn select_source(
    cookies: &str,
    mut extract: impl FnMut(&str) -> Result<(AudioSource, bool)>,
) -> Result<AudioSource> {
    match extract(cookies) {
        Ok((source, _)) => Ok(source),
        Err(error) if !cookies.trim().is_empty() && error.is::<RejectedFormats>() => {
            extract("").map(|(source, _)| source)
        }
        Err(error) => Err(error),
    }
}
fn remember_public_route(cookies: &str) {
    let mut routes = public_routes().lock().unwrap();
    routes.retain(|_, at| at.elapsed() < Duration::from_secs(600));
    if routes.len() >= 16 {
        routes.clear();
    }
    routes.insert(cookies.to_owned(), Instant::now());
}

/// Probe only the chosen stream, rather than asking yt-dlp to download-test
/// every fallback format. Never persist signed URLs or include them in errors.
fn youtube_policy() -> &'static crate::request_policy::RequestPolicy {
    crate::request_policy::youtube()
}

pub(crate) fn source_policy(source: &AudioSource) -> &'static crate::request_policy::RequestPolicy {
    if source.quality.source == "JioSaavn" {
        crate::jiosaavn::policy()
    } else {
        youtube_policy()
    }
}

fn validate_stream(source: &AudioSource, deadline: Instant) -> Result<()> {
    source_policy(source).before()?;
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(6))
            .build()
            .expect("stream probe client")
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        bail!("YouTube took too long to respond. Retry playback.");
    }
    let mut request = client
        .get(&source.url)
        .timeout(remaining.min(Duration::from_secs(6)))
        .header("User-Agent", &source.user_agent);
    for header in &source.headers {
        if let Some((name, value)) = header.split_once(": ")
            && !name.eq_ignore_ascii_case("range")
        {
            request = request.header(name, value);
        }
    }
    let response = request
        .header("Range", "bytes=0-1023")
        .send()
        .map_err(|error| {
            anyhow::anyhow!(if error.is_timeout() {
                "The audio server timed out. Retry playback."
            } else {
                "Could not reach the audio server. Check your connection."
            })
        })?;
    source_policy(source).observe(response.status(), response.headers());
    match response.status().as_u16() {
        200..=299 => {
            if response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .is_some_and(|content| {
                    content.starts_with("text/html") || content.starts_with("application/json")
                })
            {
                return Err(RejectedFormats.into());
            }
            let bytes = crate::resource::read_limited(response.take(1), 1)
                .context("The audio server returned no media bytes")?;
            if bytes.is_empty() {
                bail!("The audio server returned no media bytes");
            }
            Ok(())
        }
        403 | 410 => Err(RejectedFormats.into()),
        429 => bail!("YouTube is limiting requests. Wait a moment before retrying."),
        _ => bail!("The audio server rejected the stream. Retry playback."),
    }
}

#[derive(Debug)]
struct AccountRequired;
impl std::fmt::Display for AccountRequired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("This track requires a signed-in YouTube Music account.")
    }
}
impl std::error::Error for AccountRequired {}

fn extract_source(
    track: &Track,
    ytdlp: &str,
    cookies: &str,
    id: u64,
    generation: &AtomicU64,
    deadline: Instant,
    quality: crate::audio_settings::Quality,
) -> Result<(AudioSource, bool)> {
    if generation.load(Ordering::Acquire) != id || Instant::now() >= deadline {
        bail!("Playback request cancelled or timed out");
    }
    let mut command = Command::new(executable("yt-dlp", ytdlp));
    command.args([
        "--ignore-config",
        "--no-playlist",
        "--dump-single-json",
        "--skip-download",
        "--no-progress",
        "--socket-timeout",
        "10",
        "--retries",
        "0",
        "--extractor-retries",
        "0",
        "--sleep-requests",
        "0.5",
        "--extractor-args",
        "youtube:player_skip=initial_data",
        "-f",
        quality.format(),
        "--format-sort",
        "quality,abr,asr",
    ]);
    let deno = executable("deno", "");
    if deno.is_file() {
        command
            .arg("--js-runtimes")
            .arg(format!("deno:{}", deno.display()));
    }
    let cookie_snapshot = if !cookies.trim().is_empty() {
        if !PathBuf::from(cookies).is_file() {
            bail!("The YouTube cookie file is missing. Choose it again in Settings.");
        }
        Some(crate::auth::playback_snapshot(std::path::Path::new(
            cookies,
        ))?)
    } else {
        None
    };
    if let Some(snapshot) = &cookie_snapshot {
        command.arg("--cookies").arg(snapshot.path());
    }
    command
        .arg(format!("https://www.youtube.com/watch?v={}", track.id))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hidden(&mut command);
    let mut child = command
        .spawn()
        .context("yt-dlp is missing. Install the playback tools or set its path in Settings.")?;
    let stdout = child.stdout.take().context("Extractor output is missing")?;
    let stderr = child
        .stderr
        .take()
        .context("Extractor diagnostics are missing")?;
    let output = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(4_000_001)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let diagnostic = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(128_001).read_to_end(&mut bytes).map(|_| bytes)
    });
    let status = loop {
        if generation.load(Ordering::Acquire) != id || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = output.join();
            let _ = diagnostic.join();
            if generation.load(Ordering::Acquire) != id {
                bail!("Playback request cancelled");
            }
            bail!("YouTube took too long to respond. Check your connection and retry.");
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = output.join();
                let _ = diagnostic.join();
                bail!("Extractor failed: {error}");
            }
        }
    };
    let bytes = output
        .join()
        .map_err(|_| anyhow::anyhow!("Could not read extractor output"))??;
    let errors = diagnostic
        .join()
        .map_err(|_| anyhow::anyhow!("Could not read extractor diagnostics"))??;
    if !status.success() {
        let errors = String::from_utf8_lossy(&errors);
        let lower = errors.to_lowercase();
        if request_limited(&lower) || lower.contains("not a bot") {
            bail!("{}", extraction_error(&errors));
        }
        if formats_rejected(&errors) {
            return Err(RejectedFormats.into());
        }
        if !lower.contains("not a bot")
            && !lower.contains("429")
            && (lower.contains("private")
                || lower.contains("sign in")
                || lower.contains("members-only"))
        {
            return Err(AccountRequired.into());
        }
        bail!("{}", extraction_error(&errors));
    }
    if bytes.len() > 4_000_000 {
        bail!("The extractor returned an unexpectedly large response.");
    }
    let value: Value = serde_json::from_slice(&bytes)
        .context("yt-dlp returned an invalid response. Update the playback tools.")?;
    let url = value["url"]
        .as_str()
        .context("No playable audio stream was returned")?
        .to_owned();
    if !url.starts_with("https://") {
        bail!("The extractor returned an unsupported stream address.");
    }
    let user_agent = value["http_headers"]["User-Agent"]
        .as_str()
        .unwrap_or("Mozilla/5.0")
        .to_owned();
    let headers = value["http_headers"]
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter(|(key, _)| !key.eq_ignore_ascii_case("user-agent"))
                .filter_map(|(key, value)| {
                    value
                        .as_str()
                        .filter(|value| !value.contains(['\r', '\n']))
                        .map(|value| format!("{key}: {value}"))
                })
                .collect()
        })
        .unwrap_or_default();
    let source = AudioSource {
        url,
        headers,
        user_agent,
        quality: StreamQuality {
            source: "YouTube Music".into(),
            codec: value["acodec"].as_str().unwrap_or_default().into(),
            bitrate: value["abr"].as_f64().unwrap_or_default(),
            sample_rate: value["asr"].as_u64().unwrap_or_default(),
            protocol: value["protocol"].as_str().unwrap_or_default().into(),
        },
    };
    validate_stream(&source, deadline)?;
    Ok((source, value["vcodec"].as_str() == Some("none")))
}

pub fn extraction_error(message: &str) -> &'static str {
    let message = message.to_lowercase();
    if request_limited(&message) {
        "YouTube is limiting requests. Wait before retrying playback."
    } else if message.contains("not a bot") {
        "YouTube blocked this request (rate limit / sign-in check). Retry later or refresh your YouTube sign-in in Settings."
    } else if message.contains("permission denied") || message.contains("permissionerror") {
        "yt-dlp could not access its files. Check playback-tool and cookie-file permissions."
    } else if message.contains("cookies are no longer valid")
        || message.contains("cookies have expired")
    {
        "Your YouTube playback session expired. Sign in again from Settings."
    } else if message.contains("no such option") || message.contains("unrecognized arguments") {
        "This yt-dlp version does not support the playback options. Update the playback tools."
    } else if message.contains("certificate_verify_failed")
        || message.contains("certificate verify failed")
    {
        "YouTube's secure connection could not be verified. Check your computer's clock and network certificates."
    } else if message.contains("timed out") || message.contains("timeout") {
        "YouTube's connection timed out. Check your connection and retry playback."
    } else if message.contains("unable to download")
        || message.contains("getaddrinfo")
        || message.contains("name resolution")
        || message.contains("connection reset")
        || message.contains("network is unreachable")
    {
        "Could not connect to YouTube. Check your connection and retry playback."
    } else if message.contains("po token")
        || message.contains("po_token")
        || message.contains("proof of origin")
    {
        "YouTube requires a playback token for this format. Update yt-dlp; some formats may still require a PO Token provider."
    } else if message.contains("runtime")
        || message.contains("challenge")
        || message.contains("yt-dlp-ejs")
        || message.contains("signature extraction")
    {
        "YouTube's playback challenge could not be resolved. Update yt-dlp and Deno using the playback-tools script."
    } else if message.contains("drm protected") {
        "YouTube returned DRM-protected audio. Try another track or check your account access."
    } else if message.contains("private")
        || message.contains("sign in")
        || message.contains("members-only")
    {
        "This track requires a signed-in YouTube Music session. Sign in from Settings, or play another track."
    } else if message.contains("not available")
        || message.contains("unavailable")
        || message.contains("removed")
    {
        "This track is unavailable in your region or has been removed. Try another track."
    } else {
        "YouTube audio could not be resolved. Check your connection and update yt-dlp and Deno, then retry."
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.command(PlayerCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// End-to-end native audio smoke check using a caller-provided recording.
/// Output is muted; the real device callback must advance media time.
pub fn verify_native(path: &str) -> Result<()> {
    crate::runtime::prepare()?;
    let player = Player::new(String::new());
    let mut track = if path.starts_with("https://") {
        let crate::links::Link::Track(id) = crate::links::Link::parse(path)? else {
            bail!("Audio smoke requires a song link");
        };
        Track::song(id, "Native audio smoke", "")
    } else {
        let path = std::fs::canonicalize(path)?;
        let mut track = Track::song(
            path.to_string_lossy().into_owned(),
            "Native audio smoke",
            "Smoke Artist",
        );
        track.kind = ItemKind::Local;
        track
    };
    track.duration = 0;
    if track.kind == ItemKind::Song {
        let page = crate::catalog::Catalog::new(None)?.radio(&track.id)?;
        let songs: Vec<_> = page
            .tracks
            .iter()
            .filter(|song| song.kind == ItemKind::Song)
            .collect();
        anyhow::ensure!(!songs.is_empty(), "Song radio returned no recommendations");
        anyhow::ensure!(
            songs.iter().all(|song| !song.artist.trim().is_empty()),
            "Song radio returned missing artist metadata"
        );
        println!("Radio metadata: {} songs have artists", songs.len());
    }
    let duration = std::cell::Cell::new(0f64);
    let wait = |predicate: &dyn Fn(&PlayerEvent) -> bool| -> Result<PlayerEvent> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let response = player
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("Native playback smoke timed out")?;
            if response.generation != player.generation() {
                continue;
            }
            if let PlayerEvent::Duration(value) = response.event {
                duration.set(value);
            }
            match &response.event {
                PlayerEvent::Error(error) | PlayerEvent::SettingsError(error) => bail!("{error}"),
                event if predicate(event) => return Ok(response.event),
                _ => {}
            }
        }
    };
    player.command(PlayerCommand::Configure(
        crate::audio_settings::AudioSettings {
            jiosaavn: false,
            normalize: true,
            equalizer: [2., -1., 3., -2., 1.],
            crossfade: 2.,
            ..Default::default()
        },
    ));
    player.command(PlayerCommand::Load(track.clone(), 0.));
    wait(&|event| matches!(event, PlayerEvent::Loaded))?;
    wait(&|event| matches!(event, PlayerEvent::Position(value) if *value > 0.1))?;
    player.command(PlayerCommand::Pause(true));
    wait(&|event| matches!(event, PlayerEvent::Paused(true)))?;
    player.command(PlayerCommand::Seek(0.5));
    wait(&|event| matches!(event, PlayerEvent::Position(value) if (*value - 0.5).abs() < 0.01))?;
    player.command(PlayerCommand::Pause(false));
    wait(&|event| matches!(event, PlayerEvent::Paused(false)))?;
    wait(&|event| matches!(event, PlayerEvent::Position(value) if *value > 0.65))?;
    player.command(PlayerCommand::Prefetch(track.clone()));
    wait(&|event| matches!(event, PlayerEvent::Prepared(id) if id == &track.id))?;
    player.command(PlayerCommand::Load(track, 0.));
    wait(&|event| matches!(event, PlayerEvent::Loaded))?;
    wait(&|event| matches!(event, PlayerEvent::Position(value) if *value > 0.1))?;
    // Decode and drain the real tail; seeking past EOF bypasses resampler flushing.
    player.command(PlayerCommand::Seek((duration.get() - 0.5).max(0.)));
    wait(&|event| matches!(event, PlayerEvent::Ended))?;
    player.command(PlayerCommand::Stop);
    println!(
        "Native audio: decode, resample, output, EQ, normalization, pause, seek, prefetch, crossfade, end and stop passed ({:.3} s)",
        duration.get()
    );
    Ok(())
}

pub fn executable(name: &str, override_path: &str) -> PathBuf {
    if !override_path.trim().is_empty() {
        return PathBuf::from(override_path.trim());
    }
    if let Some(path) = crate::runtime::executable(name) {
        return path;
    }
    let filename = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    if let Ok(current) = std::env::current_exe()
        && let Some(parent) = current.parent()
    {
        for directory in [parent.to_path_buf(), parent.join("runtime")] {
            if name == "yt-dlp" {
                let unpacked = directory.join("yt-dlp").join(&filename);
                if unpacked.is_file() {
                    return unpacked;
                }
            }
            let path = directory.join(&filename);
            if path.is_file() {
                return path;
            }
        }
    }
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".runtime")
        .join(&filename);
    if name == "yt-dlp" {
        let unpacked = development.parent().unwrap().join("yt-dlp").join(&filename);
        if unpacked.is_file() {
            return unpacked;
        }
    }
    if development.is_file() {
        return development;
    }
    if let Some(path) = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join(&filename))
            .find(|path| path.is_file())
    }) {
        return path;
    }
    #[cfg(unix)]
    for directory in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        let path = PathBuf::from(directory).join(&filename);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(filename)
}

fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    let _ = command;
}
