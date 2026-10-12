//! One bounded FIFO worker for account reads and explicit writes. Never replay writes on startup.
use crate::auth_controller::{SessionAccess, Snapshot};
use raag::{
    Track,
    account::{Edit, Section},
    catalog::{Catalog, Page},
    library::data_dir,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Section(Section),
    Playlist(String),
}
impl Key {
    fn pending(&self) -> String {
        format!("read:{self:?}")
    }
    fn path(&self, snapshot: &Snapshot) -> PathBuf {
        data_dir()
            .join("pages")
            .join(snapshot.scope())
            .join(format!(
                "{}.json",
                raag::cache::key(&format!("account:{self:?}"))
            ))
    }
}
enum Action {
    Read(Key, Option<String>),
    Edit(Box<Edit>),
}
struct Job {
    snapshot: Snapshot,
    action: Action,
}
struct Event {
    generation: u64,
    pending: String,
    update: Update,
}
pub enum Update {
    Page {
        key: Key,
        page: Arc<Page>,
        append: bool,
        settled: bool,
    },
    Edited {
        edit: Box<Edit>,
        created: Option<String>,
    },
    Failed(String),
}
pub struct AccountSync {
    jobs: SyncSender<Job>,
    events: Receiver<Event>,
    pages: HashMap<Key, Arc<Page>>,
    pending: HashSet<String>,
    confirmed_likes: HashMap<String, bool>,
}
impl AccountSync {
    pub fn new(access: SessionAccess) -> Self {
        let (jobs, requests) = mpsc::sync_channel::<Job>(16);
        let (sender, events) = mpsc::sync_channel(16);
        thread::spawn(move || {
            let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Catalog);
            let mut catalog = match Catalog::new(None) {
                Ok(c) => c,
                Err(_) => return,
            };
            let mut bootstrapped = false;
            while let Ok(job) = requests.recv() {
                let generation = job.snapshot.generation;
                if job.snapshot.session.is_none() || !access.current(generation) {
                    continue;
                }
                if !bootstrapped && !raag::cache::offline_only() {
                    catalog.bootstrap();
                    bootstrapped = true;
                }
                catalog.set_session(job.snapshot.session.clone());
                let pending = match &job.action {
                    Action::Read(key, _) => key.pending(),
                    Action::Edit(edit) => edit.key(),
                };
                let publish = |update| {
                    if access.current(generation) {
                        sender
                            .send(Event {
                                generation,
                                pending: pending.clone(),
                                update,
                            })
                            .is_ok()
                    } else {
                        true
                    }
                };
                let result = match job.action {
                    Action::Read(key, token) => {
                        let path = key.path(&job.snapshot);
                        if token.is_none()
                            && let Ok(file) = std::fs::File::open(&path)
                            && let Ok(page) =
                                raag::resource::json::<Page>(file, raag::resource::PAGE_BYTES)
                            && !publish(Update::Page {
                                key: key.clone(),
                                page: Arc::new(page),
                                append: false,
                                settled: false,
                            })
                        {
                            break;
                        }
                        let result = if raag::cache::offline_only() {
                            Err(anyhow::anyhow!(
                                "Account sync is unavailable in offline mode."
                            ))
                        } else {
                            match &key {
                                Key::Section(section) => {
                                    catalog.account_section(*section, token.as_deref())
                                }
                                Key::Playlist(id) => catalog.account_playlist(id, token.as_deref()),
                            }
                        };
                        result.map(|page| {
                            if token.is_none() && access.current(generation) {
                                let _ = raag::cache::write_json(&path, &page);
                                if access.current(generation) {
                                    let _ = raag::cache::maintain_metadata(&path);
                                } else {
                                    let _ = std::fs::remove_file(&path);
                                }
                            }
                            Update::Page {
                                key,
                                page: Arc::new(page),
                                append: token.is_some(),
                                settled: true,
                            }
                        })
                    }
                    Action::Edit(edit) => catalog
                        .edit_account(&edit, || access.current(generation))
                        .map(|created| {
                            // Invalidate replaceable snapshots after acknowledgement, never roll old account data into a new account.
                            let mut keys: Vec<_> =
                                Section::ALL.into_iter().map(Key::Section).collect();
                            if let Some(id) = edit.playlist() {
                                keys.push(Key::Playlist(id.into()));
                            }
                            for key in keys {
                                let _ = std::fs::remove_file(key.path(&job.snapshot));
                            }
                            Update::Edited { edit, created }
                        }),
                };
                catalog.set_session(None);
                if !publish(result.unwrap_or_else(|error| Update::Failed(error.to_string()))) {
                    break;
                }
            }
        });
        Self {
            jobs,
            events,
            pages: HashMap::new(),
            pending: HashSet::new(),
            confirmed_likes: HashMap::new(),
        }
    }
    pub fn reset(&mut self) {
        self.pages.clear();
        self.pending.clear();
        self.confirmed_likes.clear();
    }
    pub fn page(&self, key: &Key) -> Option<&Page> {
        self.pages.get(key).map(Arc::as_ref)
    }
    pub fn loading(&self, key: &Key) -> bool {
        self.pending.contains(&key.pending())
    }
    pub fn editing(&self, edit: &Edit) -> bool {
        self.pending.contains(&edit.key())
    }
    pub fn syncing(&self) -> bool {
        self.pending.iter().any(|key| key.starts_with("read:"))
    }
    pub fn busy_edits(&self) -> bool {
        self.pending.iter().any(|key| !key.starts_with("read:"))
    }
    pub fn liked(&self, track: &Track) -> bool {
        self.confirmed_likes
            .get(&track.id)
            .copied()
            .unwrap_or_else(|| {
                self.page(&Key::Section(Section::Liked))
                    .is_some_and(|p| p.tracks.iter().any(|t| t.id == track.id))
                    || track.like_status.unwrap_or(false)
            })
    }
    pub fn read(&mut self, snapshot: Snapshot, key: Key, more: bool) -> bool {
        let token = if more {
            self.page(&key).and_then(|p| p.continuation.clone())
        } else {
            None
        };
        if more && token.is_none() {
            return false;
        }
        let pending = key.pending();
        self.submit(snapshot, pending, Action::Read(key, token))
    }
    pub fn edit(&mut self, snapshot: Snapshot, edit: Edit) -> bool {
        self.submit(snapshot, edit.key(), Action::Edit(Box::new(edit)))
    }
    fn submit(&mut self, snapshot: Snapshot, pending: String, action: Action) -> bool {
        if snapshot.session.is_none() || self.pending.contains(&pending) {
            return false;
        }
        self.enqueue(pending, Job { snapshot, action })
    }
    fn enqueue(&mut self, pending: String, job: Job) -> bool {
        if self.pending.contains(&pending) || self.jobs.try_send(job).is_err() {
            return false;
        }
        self.pending.insert(pending);
        true
    }
    pub fn poll(&mut self, access: &SessionAccess) -> Option<Update> {
        loop {
            let event = self.events.try_recv().ok()?;
            if !access.current(event.generation) {
                continue;
            }
            // Disk snapshots only seed an empty view. Replaying one over a live
            // collection rolls back pagination/artwork while refresh is pending.
            if matches!(&event.update, Update::Page { key, settled: false, .. } if self.pages.contains_key(key))
            {
                continue;
            }
            if !matches!(&event.update, Update::Page { settled: false, .. }) {
                self.pending.remove(&event.pending);
            }
            if let Update::Page {
                key,
                page,
                append,
                settled,
            } = &event.update
            {
                if *key == Key::Section(Section::Liked) && *settled && !append {
                    let liked_ids: HashSet<_> =
                        page.tracks.iter().map(|track| track.id.as_str()).collect();
                    for (id, liked) in &mut self.confirmed_likes {
                        if liked_ids.contains(id.as_str()) {
                            *liked = true;
                        } else if page.continuation.is_none() {
                            *liked = false;
                        }
                    }
                }
                if *append && let Some(existing) = self.pages.get_mut(key) {
                    merge(Arc::make_mut(existing), page);
                } else {
                    self.pages.insert(key.clone(), page.clone());
                }
                self.trim_pages(key);
            }
            if let Update::Edited { edit, created } = &event.update {
                self.apply_edit(edit, created.as_deref());
            }
            return Some(event.update);
        }
    }
    pub fn apply_like(&mut self, track: &Track, liked: bool) {
        if self.confirmed_likes.len() >= 10_000 {
            self.confirmed_likes.clear();
        }
        self.confirmed_likes.insert(track.id.clone(), liked);
        let page = Arc::make_mut(self.pages.entry(Key::Section(Section::Liked)).or_default());
        page.tracks.retain(|t| t.id != track.id);
        if liked {
            page.tracks.insert(0, track.clone());
        }
    }
    fn trim_pages(&mut self, current: &Key) {
        while self.pages.len() > 24 {
            let Some(old) = self
                .pages
                .keys()
                .find(|key| *key != current && matches!(key, Key::Playlist(_)))
                .cloned()
            else {
                break;
            };
            self.pages.remove(&old);
        }
    }
    fn apply_edit(&mut self, edit: &Edit, created: Option<&str>) {
        match edit {
            Edit::AddMany { playlist, tracks } => {
                for track in tracks {
                    self.apply_edit(
                        &Edit::Add {
                            playlist: playlist.clone(),
                            track: track.clone(),
                        },
                        None,
                    );
                }
            }
            Edit::RemoveMany { playlist, tracks } => {
                for track in tracks {
                    self.apply_edit(
                        &Edit::Remove {
                            playlist: playlist.clone(),
                            track: track.clone(),
                        },
                        None,
                    );
                }
            }
            Edit::Like { track, liked } => self.apply_like(track, *liked),
            Edit::Add { playlist, track } => {
                if let Some(page) = self.pages.get_mut(&Key::Playlist(playlist.clone())) {
                    let mut track = track.clone();
                    track.set_video_id = None;
                    Arc::make_mut(page).tracks.push(track);
                }
            }
            Edit::Remove { playlist, track } => {
                if let Some(page) = self.pages.get_mut(&Key::Playlist(playlist.clone())) {
                    Arc::make_mut(page)
                        .tracks
                        .retain(|t| t.set_video_id != track.set_video_id);
                }
            }
            Edit::Rename { playlist, name } => {
                if let Some(page) = self.pages.get_mut(&Key::Playlist(playlist.clone())) {
                    Arc::make_mut(page).title = name.trim().into();
                }
                if let Some(page) = self.pages.get_mut(&Key::Section(Section::Playlists)) {
                    for track in &mut Arc::make_mut(page).tracks {
                        if track.id == *playlist {
                            track.title = name.trim().into();
                        }
                    }
                }
            }
            Edit::Description { playlist, text } => {
                if let Some(page) = self.pages.get_mut(&Key::Playlist(playlist.clone())) {
                    Arc::make_mut(page).description = text.clone();
                }
            }
            Edit::Privacy { playlist, value } => {
                if let Some(page) = self.pages.get_mut(&Key::Playlist(playlist.clone())) {
                    Arc::make_mut(page).privacy = Some(value.clone());
                }
            }
            Edit::Create { name } => {
                if let Some(id) = created {
                    let mut track = Track::song(id, name.trim(), "YouTube Music");
                    track.kind = raag::ItemKind::Playlist;
                    Arc::make_mut(
                        self.pages
                            .entry(Key::Section(Section::Playlists))
                            .or_default(),
                    )
                    .tracks
                    .insert(0, track);
                    self.pages.insert(
                        Key::Playlist(id.into()),
                        Arc::new(Page {
                            title: name.trim().into(),
                            editable: true,
                            ..Default::default()
                        }),
                    );
                    self.trim_pages(&Key::Playlist(id.into()));
                }
            }
            Edit::Delete { playlist } => {
                self.pages.remove(&Key::Playlist(playlist.clone()));
                if let Some(page) = self.pages.get_mut(&Key::Section(Section::Playlists)) {
                    Arc::make_mut(page).tracks.retain(|t| {
                        t.id.strip_prefix("VL").unwrap_or(&t.id)
                            != playlist.strip_prefix("VL").unwrap_or(playlist)
                    });
                }
            }
            Edit::Move {
                playlist,
                entry,
                before,
            } => {
                if let Some(page) = self.pages.get_mut(&Key::Playlist(playlist.clone()))
                    && let Some(index) = page
                        .tracks
                        .iter()
                        .position(|t| t.set_video_id.as_ref() == Some(entry))
                {
                    let page = Arc::make_mut(page);
                    let track = page.tracks.remove(index);
                    let target = before
                        .as_ref()
                        .and_then(|id| {
                            page.tracks
                                .iter()
                                .position(|t| t.set_video_id.as_ref() == Some(id))
                        })
                        .unwrap_or(page.tracks.len());
                    page.tracks.insert(target, track);
                }
            }
            _ => {}
        }
    }
}
fn merge(existing: &mut Page, page: &Page) {
    let previous_count = existing.tracks.len();
    let mut seen: HashSet<_> = existing.tracks.iter().map(identity).collect();
    existing.tracks.extend(
        page.tracks
            .iter()
            .filter(|t| seen.insert(identity(t)))
            .cloned(),
    );
    existing.tracks.truncate(10_000);
    existing.continuation = if existing.tracks.len() >= 10_000
        || (previous_count == existing.tracks.len() && existing.continuation == page.continuation)
    {
        None
    } else {
        page.continuation.clone()
    };
}
fn identity(track: &Track) -> (String, String) {
    (
        track.id.clone(),
        track.set_video_id.clone().unwrap_or_default(),
    )
}
