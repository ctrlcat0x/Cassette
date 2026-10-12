use super::*;
use crate::account_sync::{Key, Update};

impl Raag {
    pub(super) fn signed_in(&self) -> bool {
        self.auth.snapshot().session.is_some()
    }
    pub(super) fn account_tracks(&self, section: AccountSection) -> Vec<Track> {
        self.account
            .page(&Key::Section(section))
            .map_or_else(Vec::new, |p| p.tracks.clone())
    }
    pub(super) fn account_collection_key(&self) -> Option<Key> {
        let Screen::Collection(id, _, false) = &self.screen else {
            return None;
        };
        (self.signed_in()
            && (id.starts_with("VL")
                || id.starts_with("PL")
                || self.account.page(&Key::Playlist(id.clone())).is_some()
                || self.account.loading(&Key::Playlist(id.clone()))))
        .then(|| Key::Playlist(id.clone()))
    }
    pub(super) fn is_liked(&self, track: &Track) -> bool {
        if self.signed_in() && track.kind == ItemKind::Song {
            self.account.liked(track)
        } else {
            self.library.liked(&track.id)
        }
    }
    pub(super) fn recent_tracks(&self) -> Vec<Track> {
        let remote = self.account.page(&Key::Section(AccountSection::History));
        let mut seen = std::collections::HashSet::new();
        self.library
            .history
            .iter()
            .chain(remote.into_iter().flat_map(|p| &p.tracks))
            .filter(|t| seen.insert(t.id.clone()))
            .take(200)
            .cloned()
            .collect()
    }
    pub(super) fn account_view_key(&self) -> Option<Key> {
        if !self.signed_in() {
            return None;
        }
        match self.screen {
            Screen::PlaylistPicker => Some(Key::Section(AccountSection::Playlists)),
            Screen::Liked => Some(Key::Section(AccountSection::Liked)),
            Screen::Library => Some(Key::Section(self.library_section)),
            _ => self.account_collection_key(),
        }
    }
    pub(super) fn sync_account_view(&mut self) {
        if let Some(key) = self.account_view_key() {
            self.loading = self.account.loading(&key);
            if let Some(page) = self.account.page(&key) {
                self.page = page.clone();
                self.continuation = page.continuation.clone();
            }
        }
        self.refresh_visible();
    }
    pub(super) fn account_read(&mut self, key: Key, more: bool, cx: &mut Context<Self>) {
        if !self.account.loading(&key) && !self.account.read(self.auth.snapshot(), key, more) {
            self.notice = Some(
                "Account requests are busy. Wait a moment, then sync from the profile menu.".into(),
            );
        }
        self.sync_account_view();
        cx.notify();
    }
    pub(super) fn account_edit(&mut self, edit: Edit, cx: &mut Context<Self>) {
        if self.account.editing(&edit) {
            return;
        }
        self.notice = if self.account.edit(self.auth.snapshot(), edit) {
            None
        } else {
            Some(
                "Could not queue the edit. Sign in or wait for the current changes to finish."
                    .into(),
            )
        };
        self.content_revision += 1;
        cx.notify();
    }
    pub(super) fn sync_youtube_music(&mut self, cx: &mut Context<Self>) {
        if self.auth.busy() || self.loading || self.account.syncing() {
            return;
        }
        self.pagination_attempts.clear();
        if self.signed_in() {
            let snapshot = self.auth.snapshot();
            for section in AccountSection::ALL {
                self.account
                    .read(snapshot.clone(), Key::Section(section), false);
            }
            if let Some(key) = self.account_collection_key() {
                self.account.read(snapshot, key, false);
            }
        }
        if self.account_view_key().is_some() {
            self.sync_account_view();
        } else if matches!(
            self.screen,
            Screen::Home
                | Screen::Explore
                | Screen::Discover(..)
                | Screen::Search
                | Screen::Radio(_)
                | Screen::Collection(_, _, false)
        ) {
            self.reload(cx);
        }
        cx.notify();
    }
    pub(super) fn open_account_playlist(&mut self, track: Track, cx: &mut Context<Self>) {
        let id = track.id.clone();
        self.navigate(
            Screen::Collection(id.clone(), track.title.clone(), false),
            cx,
        );
        self.collection_seed = Some(track);
        self.account_read(Key::Playlist(id), false, cx);
    }
    pub(super) fn account_update(&mut self, update: Update, cx: &mut Context<Self>) {
        // Playlists may finish syncing while another screen owns the content pane.
        self.navigation_revision = self.navigation_revision.wrapping_add(1);
        let relevant = match &update {
            Update::Page { key, .. } => self.account_view_key().as_ref() == Some(key),
            _ => true,
        };
        match update {
            Update::Page { key, page, .. } => {
                // Only warm the initial viewport; visible covers use the existing priority loader.
                if self.account_view_key().as_ref() == Some(&key)
                    || key == Key::Section(AccountSection::History)
                {
                    fetch_artwork(
                        page.tracks.iter().take(9).cloned().collect(),
                        self.backend.events_sender.clone(),
                    );
                }
            }
            Update::Edited { edit, created } => {
                let edit = *edit;
                // The confirmed playlist/heart state is the success feedback.
                let batch_pending = !self.pending_edits.is_empty()
                    && matches!(edit, Edit::AddMany { .. } | Edit::RemoveMany { .. });
                if let Some(id) = edit
                    .playlist()
                    .filter(|_| !matches!(edit, Edit::Delete { .. }) && !batch_pending)
                {
                    self.account
                        .read(self.auth.snapshot(), Key::Playlist(id.into()), false);
                }
                match &edit {
                    Edit::Create { name } => {
                        if let Some(id) = created
                            && self.screen == Screen::Library
                            && self.editing_playlist
                            && self.playlist_editor_remote
                        {
                            self.editing_playlist = false;
                            let mut track = Track::song(id, name.trim(), "");
                            track.kind = ItemKind::Playlist;
                            self.open_account_playlist(track, cx);
                        }
                    }
                    Edit::Rename { playlist, name } => {
                        if matches!(&self.screen, Screen::Collection(id, _, false) if id == playlist)
                        {
                            self.editing_playlist = false;
                            self.screen =
                                Screen::Collection(playlist.clone(), name.trim().into(), false);
                        }
                    }
                    Edit::Description { playlist, .. } => {
                        if matches!(&self.screen, Screen::Collection(id, _, false) if id == playlist)
                        {
                            self.editing_playlist = false;
                            self.editing_description = false;
                        }
                    }
                    Edit::Delete { playlist } => {
                        if matches!(&self.screen, Screen::Collection(id, _, false) if id == playlist)
                        {
                            self.navigate(Screen::Library, cx);
                        }
                    }
                    Edit::Follow { artist, followed } => {
                        if matches!(&self.screen, Screen::Collection(id, _, false) if id == artist)
                        {
                            self.page.subscribed = Some(*followed);
                        }
                    }
                    _ => {}
                }
                let sections: &[AccountSection] = match &edit {
                    Edit::Like { .. } | Edit::Dislike { .. } => &[AccountSection::Liked],
                    Edit::Follow { .. } => &[AccountSection::Artists],
                    Edit::History { .. } => &[AccountSection::History],
                    Edit::Feedback { .. } => &[AccountSection::Songs, AccountSection::Albums],
                    Edit::RateCollection { .. } => {
                        &[AccountSection::Playlists, AccountSection::Albums]
                    }
                    _ => &[AccountSection::Playlists],
                };
                if !batch_pending {
                    for section in sections {
                        self.account
                            .read(self.auth.snapshot(), Key::Section(*section), false);
                    }
                }
            }
            Update::Failed(message) => {
                self.pending_edits.clear();
                self.notice = Some(format!("YouTube Music: {message}"));
            }
        }
        if relevant {
            self.sync_account_view();
        } else if self.screen == Screen::Home {
            self.content_revision += 1;
        }
    }
    pub(super) fn account_playlist_menu(
        &self,
        playlist: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        let privacy = self.page.privacy.clone();
        let no_song = self
            .selected
            .as_ref()
            .is_none_or(|track| track.kind != ItemKind::Song);
        Button::new("account-playlist-menu")
            .ghost()
            .icon(icon("more"))
            .tooltip("Playlist actions")
            .accessibility_label("Playlist actions")
            .dropdown_menu(move |mut menu, window, cx| {
                let add_owner = owner.clone();
                let add_id = playlist.clone();
                menu = menu
                    .item(
                        PopupMenuItem::new("Add playing song")
                            .icon(icon("plus"))
                            .disabled(no_song)
                            .on_click(move |_, _, cx| {
                                let _ = add_owner.update(cx, |this, cx| {
                                    if let Some(track) = this.selected.clone() {
                                        this.account_edit(
                                            Edit::Add {
                                                playlist: add_id.clone(),
                                                track,
                                            },
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                    .separator();
                for (label, description) in [("Rename playlist…", false), ("Edit description…", true)] {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .icon(icon(if description { "lyrics" } else { "text" }))
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.editing_playlist = true;
                                    this.editing_description = description;
                                    this.playlist_editor_remote = true;
                                    let name = if description {
                                        this.page.description.clone()
                                    } else if this.page.title.is_empty() {
                                        this.screen.title().to_owned()
                                    } else {
                                        this.page.title.clone()
                                    };
                                    this.playlist_name.update(cx, |input, cx| {
                                        input.set_value(name, window, cx);
                                        input.focus(window, cx);
                                    });
                                    this.content_revision += 1;
                                    cx.notify();
                                });
                            }),
                    );
                }
                let privacy_owner = owner.clone();
                let privacy_id = playlist.clone();
                let privacy = privacy.clone();
                menu = menu.submenu("Visibility", window, cx, move |mut menu, _, _| {
                    for (label, value) in [
                        ("Private", "PRIVATE"),
                        ("Unlisted", "UNLISTED"),
                        ("Public", "PUBLIC"),
                    ] {
                        let owner = privacy_owner.clone();
                        let id = privacy_id.clone();
                        menu = menu.item(
                            PopupMenuItem::new(label)
                                .checked(privacy.as_deref() == Some(value))
                                .on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        this.account_edit(
                                            Edit::Privacy {
                                                playlist: id.clone(),
                                                value: value.into(),
                                            },
                                            cx,
                                        );
                                    });
                                }),
                        );
                    }
                    menu
                });
                let delete_owner = owner.clone();
                let delete_id = playlist.clone();
                menu.separator().item(
                    PopupMenuItem::new("Delete playlist…").on_click(move |_, window, cx| {
                        let _ = delete_owner.update(cx, |_this, cx| {
                            let prompt = window.prompt(
                                PromptLevel::Warning,
                                "Delete this playlist?",
                                Some("This permanently deletes the playlist from your YouTube account."),
                                &["Cancel", "Delete playlist"],
                                cx,
                            );
                            let id = delete_id.clone();
                            cx.spawn(async move |owner, cx| {
                                if prompt.await == Ok(1) {
                                    let _ = owner.update(cx, |this, cx| {
                                        this.account_edit(Edit::Delete { playlist: id }, cx);
                                    });
                                }
                            })
                            .detach();
                        });
                    }),
                )
            })
            .into_any_element()
    }
}
