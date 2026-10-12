//! OS media surfaces and hardware keys. Updates are coalesced, never per frame.
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

pub struct Media {
    #[cfg(not(windows))]
    controls: MediaControls,
    #[cfg(windows)]
    updates: crate::jobs::JobSender<Update>,
    pub events: Receiver<MediaControlEvent>,
    previous: Option<(String, bool, u64, Option<String>)>,
    updated: Instant,
}
struct Update {
    track: Option<raag::Track>,
    playing: bool,
    position: f64,
    duration: f64,
    volume: f64,
    artwork: Option<String>,
}

#[cfg(windows)]
#[link(name = "runtimeobject")]
unsafe extern "system" {
    fn RoInitialize(kind: u32) -> i32;
    fn RoUninitialize();
}

impl Media {
    pub fn new(window: &Window) -> Option<Self> {
        let hwnd = match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut std::ffi::c_void),
            _ => None,
        };
        let mut controls = MediaControls::new(PlatformConfig {
            dbus_name: "cassette",
            display_name: "Cassette",
            hwnd,
        })
        .ok()?;
        let (sender, events) = mpsc::sync_channel(32);
        controls
            .attach(move |event| {
                let _ = sender.try_send(event);
            })
            .ok()?;
        #[cfg(windows)]
        let updates = {
            let (updates, pending) = crate::jobs::channel::<Update>(1);
            std::thread::Builder::new()
                .name("cassette-media".into())
                .spawn(move || {
                    // SMTC interfaces are Send; the receiving thread still needs
                    // its own WinRT apartment. Destroy controls before uninitializing.
                    if unsafe { RoInitialize(1) } < 0 {
                        return;
                    }
                    {
                        let mut controls = controls;
                        let mut previous = None;
                        while let Ok(update) = pending.recv() {
                            let key = (
                                update.track.as_ref().map(|t| t.id.clone()),
                                update.duration.max(0.) as u64,
                                update.artwork.clone(),
                            );
                            apply(&mut controls, &update, previous.as_ref() != Some(&key));
                            previous = Some(key);
                        }
                        let _ = controls.detach();
                    }
                    unsafe { RoUninitialize() };
                })
                .ok()?;
            updates
        };
        Some(Self {
            #[cfg(not(windows))]
            controls,
            #[cfg(windows)]
            updates,
            events,
            previous: None,
            updated: Instant::now(),
        })
    }
    pub fn update(
        &mut self,
        track: Option<&raag::Track>,
        playing: bool,
        position: f64,
        duration: f64,
        volume: f64,
        artwork: Option<String>,
    ) {
        let key = (
            track.map_or_else(String::new, |t| t.id.clone()),
            playing,
            duration.max(0.) as u64,
            artwork.clone(),
        );
        let changed = self.previous.as_ref() != Some(&key);
        if !changed && self.updated.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.updated = Instant::now();
        let update = Update {
            track: track.cloned(),
            playing,
            position,
            duration,
            volume,
            artwork,
        };
        #[cfg(windows)]
        let _ = self.updates.send(update);
        #[cfg(not(windows))]
        apply(&mut self.controls, &update, changed);
        self.previous = Some(key);
    }
}

fn apply(controls: &mut MediaControls, update: &Update, metadata_changed: bool) {
    let track = update.track.as_ref();
    if metadata_changed {
        // Souvlaki's Windows file loader expects a native path after file://,
        // rather than an escaped URI (which also leaves a leading /C:/ behind).
        #[cfg(windows)]
        let artwork = update.artwork.as_ref().map(|url| {
            reqwest::Url::parse(url)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .map_or_else(|| url.clone(), |path| format!("file://{}", path.display()))
        });
        #[cfg(not(windows))]
        let artwork = &update.artwork;
        let _ = controls.set_metadata(MediaMetadata {
            title: track.map(|t| t.title.as_str()),
            artist: track.map(|t| t.artist.as_str()),
            album: track.map(|t| t.album.as_str()),
            duration: Some(Duration::from_secs_f64(update.duration.max(0.))),
            cover_url: artwork.as_deref(),
        });
    }
    let progress = Some(MediaPosition(Duration::from_secs_f64(
        update.position.max(0.),
    )));
    let _ = controls.set_playback(if track.is_none() {
        MediaPlayback::Stopped
    } else if update.playing {
        MediaPlayback::Playing { progress }
    } else {
        MediaPlayback::Paused { progress }
    });
    #[cfg(target_os = "linux")]
    let _ = controls.set_volume(update.volume / 100.);
    #[cfg(not(target_os = "linux"))]
    let _ = update.volume;
}
