//! Explicit downloads have a bounded FIFO and cancellation independent of playback.
use raag::{ItemKind, Track};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender},
};

pub enum Job {
    Tracks(Vec<Track>, String, String),
    Remove(Option<String>, bool),
}
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub completed: usize,
    pub total: usize,
    pub current: String,
    pub error: Option<String>,
    pub active: bool,
    pub bytes: u64,
    pub pinned: usize,
}
pub struct Downloads {
    sender: SyncSender<(u64, Job)>,
    cancel: Arc<AtomicU64>,
    pub events: Receiver<Status>,
}
impl Downloads {
    pub fn new(artwork_events: SyncSender<crate::service::Event>) -> Self {
        let (sender, requests) = mpsc::sync_channel::<(u64, Job)>(8);
        let (updates, events) = mpsc::sync_channel(32);
        let cancel = Arc::new(AtomicU64::new(0));
        let generation = cancel.clone();
        std::thread::spawn(move || {
            let mut state = Status::default();
            (state.bytes, state.pinned) = raag::cache::audio_usage();
            let _ = updates.send(state.clone());
            while let Ok((id, job)) = requests.recv() {
                if id != generation.load(Ordering::Acquire) {
                    continue;
                }
                state.error = None;
                match job {
                    Job::Remove(track, temporary) => {
                        state.error = raag::cache::remove_audio(track.as_deref(), temporary)
                            .err()
                            .map(|e| e.to_string());
                    }
                    Job::Tracks(tracks, extractor, cookies) => {
                        let mut seen = std::collections::HashSet::new();
                        let tracks: Vec<_> = tracks
                            .into_iter()
                            .filter(|t| t.kind == ItemKind::Song && seen.insert(t.id.clone()))
                            .take(10_000)
                            .collect();
                        state.total = tracks.len();
                        state.completed = 0;
                        state.active = true;
                        for track in tracks {
                            if id != generation.load(Ordering::Acquire) {
                                break;
                            }
                            state.current = track.title.clone();
                            let _ = updates.send(state.clone());
                            match raag::cache::store_audio_controlled(
                                track.clone(),
                                true,
                                true,
                                &extractor,
                                &cookies,
                                id,
                                &generation,
                            ) {
                                Ok(()) => {
                                    crate::service::cache_artwork(&track, artwork_events.clone());
                                    if !raag::cache::offline_only()
                                        && id == generation.load(Ordering::Acquire)
                                    {
                                        let path = raag::library::data_dir()
                                            .join("lyrics")
                                            .join(format!("{}.json", raag::cache::key(&track.id)));
                                        if !path.exists()
                                            && let Ok(lyrics) = raag::lyrics::fetch(&track)
                                        {
                                            let _ = raag::cache::write_json(&path, &lyrics);
                                            let _ = raag::cache::maintain_metadata(&path);
                                        }
                                    }
                                    state.completed += 1;
                                }
                                Err(error) => state.error = Some(error.to_string()),
                            }
                        }
                    }
                }
                state.active = false;
                state.current.clear();
                (state.bytes, state.pinned) = raag::cache::audio_usage();
                let _ = updates.send(state.clone());
            }
        });
        Self {
            sender,
            cancel,
            events,
        }
    }
    pub fn submit(&self, job: Job) -> bool {
        self.sender
            .try_send((self.cancel.load(Ordering::Acquire), job))
            .is_ok()
    }
    pub fn cancel(&self) {
        self.cancel.fetch_add(1, Ordering::AcqRel);
    }
}
