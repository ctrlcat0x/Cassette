use super::*;

impl Raag {
    pub(super) fn open_link(&mut self, link: raag::links::Link, cx: &mut Context<Self>) {
        use raag::links::Link;
        match link {
            Link::Spotify { kind, id } => {
                let playlist = kind == "playlist";
                self.notice = Some(t("Matching Spotify metadata on YouTube Music…").into());
                cx.spawn(async move |owner, cx| {
                    let result = cx
                        .background_spawn(async move {
                            raag::links::resolve_spotify(&kind, &id).map_err(|e| e.to_string())
                        })
                        .await;
                    let _ = owner.update(cx, |this, cx| {
                        match result {
                            Ok(resolved) => {
                                this.notice = None;
                                if let Some(collection) = resolved.collection {
                                    this.open_track(collection, cx);
                                } else if playlist {
                                    match this.library.create_playlist(&resolved.name) {
                                        Ok(id) => {
                                            if let Some(p) = this
                                                .library
                                                .playlists
                                                .iter_mut()
                                                .find(|p| p.id == id)
                                            {
                                                p.tracks = resolved.tracks;
                                            }
                                            this.save();
                                            this.navigate(
                                                Screen::Collection(id, resolved.name, true),
                                                cx,
                                            );
                                        }
                                        Err(error) => this.notice = Some(error.to_string()),
                                    }
                                } else if let Some(track) = resolved.tracks.first().cloned() {
                                    this.open_track(track, cx);
                                }
                            }
                            Err(error) => this.notice = Some(error),
                        }
                        this.content_revision += 1;
                        cx.notify();
                    });
                })
                .detach();
            }
            Link::Track(id) => {
                let track = self
                    .library
                    .history
                    .iter()
                    .chain(&self.library.liked)
                    .chain(self.visible.iter())
                    .find(|t| t.id == id)
                    .cloned()
                    .unwrap_or_else(|| Track::song(id, t("YouTube Music song"), "YouTube Music"));
                self.queue.replace(vec![track.clone()], &track.id);
                self.invalidate_collection();
                self.start(track, cx);
            }
            Link::Playlist(id) | Link::Album(id) | Link::Artist(id) => {
                self.navigate(
                    Screen::Collection(id.clone(), "YouTube Music".into(), false),
                    cx,
                );
                self.reload(cx);
            }
        }
    }
    pub(super) fn register_links(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |owner, cx| {
            let result = cx
                .background_spawn(async {
                    crate::system::register_links().map_err(|e| e.to_string())
                })
                .await;
            let _ = owner.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.notice = Some(error);
                }
                this.content_revision += 1;
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn search_suggestions(&self, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .top(ui_px(38.))
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .p_2()
            .rounded_lg()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_md()
            .children(
                self.suggestions
                    .iter()
                    .enumerate()
                    .map(|(index, suggestion)| {
                        let query = suggestion.clone();
                        Button::new(("search-suggestion", index))
                            .ghost()
                            .justify_start()
                            .label(suggestion.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.query.update(cx, |input, cx| {
                                    input.set_value(query.clone(), window, cx)
                                });
                                this.suggestions.clear();
                                this.suggestion_pending = None;
                                this.suggestion_revision = this.suggestion_revision.wrapping_add(1);
                                this.navigate(Screen::Search, cx);
                                this.search_query = query.clone();
                                this.search_filter = 0;
                                this.catalog = Arc::new(vec![]);
                                this.page = Default::default();
                                this.scroll.reset();
                                this.refresh_visible();
                                this.request(|id| Request::Search(id, query.clone(), None), cx);
                                window.focus(&this.focus, cx);
                            }))
                    }),
            )
    }
    pub(super) fn invalidate_collection(&mut self) {
        self.queue_epoch = self.queue_epoch.wrapping_add(1);
        self.backend
            .collection_epoch
            .store(self.queue_epoch, std::sync::atomic::Ordering::Release);
        self.radio_pending = None;
        self.prefetched_next = None;
    }
    pub(super) fn start_mix(&mut self, cx: &mut Context<Self>) {
        use rand::seq::SliceRandom;
        let mut known = std::collections::HashSet::new();
        let mut tracks: Vec<_> = self
            .library
            .stats
            .top_tracks()
            .into_iter()
            .map(|s| s.track.clone())
            .chain(self.library.liked.iter().cloned())
            .chain(self.recent_tracks())
            .chain(self.visible.iter().cloned())
            .chain(self.library.local.iter().cloned())
            .filter(|t| {
                t.playable()
                    && (!self.library.offline_only
                        || t.kind == ItemKind::Local
                        || self.offline.iter().any(|cached| cached.id == t.id))
                    && !self.library.hidden.contains(&t.id)
                    && known.insert(t.id.clone())
            })
            .take(60)
            .collect();
        tracks.shuffle(&mut rand::rng());
        if let Some(track) = tracks.first().cloned() {
            self.queue.replace(tracks, &track.id);
            self.invalidate_collection();
            self.start(track, cx);
            self.radio_mix = true;
            self.request_radio();
        }
    }
    pub(super) fn move_playlist_track(
        &mut self,
        track: &Track,
        delta: isize,
        cx: &mut Context<Self>,
    ) {
        let Screen::Collection(id, _, local) = &self.screen else {
            return;
        };
        let id = id.clone();
        if *local {
            if let Some(playlist) = self
                .library
                .playlists
                .iter_mut()
                .find(|p| p.id == id && p.smart.is_none())
                && let Some(index) = playlist.tracks.iter().position(|t| t == track)
                && let Some(target) = index
                    .checked_add_signed(delta)
                    .filter(|target| *target < playlist.tracks.len())
            {
                playlist.tracks.swap(index, target);
                self.navigation_revision = self.navigation_revision.wrapping_add(1);
                self.save();
                self.refresh_visible();
                cx.notify();
            }
        } else if let Some(page) = self
            .account
            .page(&crate::account_sync::Key::Playlist(id.clone()))
            && let Some(index) = page
                .tracks
                .iter()
                .position(|t| t.set_video_id.is_some() && t.set_video_id == track.set_video_id)
            && let Some(target) = index
                .checked_add_signed(delta)
                .filter(|target| *target < page.tracks.len())
        {
            let before = if delta < 0 {
                page.tracks[target].set_video_id.clone()
            } else {
                page.tracks
                    .get(target + 1)
                    .and_then(|t| t.set_video_id.clone())
            };
            if target + 1 == page.tracks.len() && delta > 0 && page.continuation.is_some() {
                self.notice = Some(
                    t("Scroll to load the next songs before moving this entry further.").into(),
                );
                cx.notify();
                return;
            }
            self.account_edit(
                Edit::Move {
                    playlist: id,
                    entry: track.set_video_id.clone().unwrap(),
                    before,
                },
                cx,
            );
        }
    }
    pub(super) fn complete_collection(&mut self, download: bool) -> bool {
        let Screen::Collection(id, _, false) = &self.screen else {
            return false;
        };
        let page = self
            .account_collection_key()
            .and_then(|key| self.account.page(&key))
            .cloned()
            .unwrap_or_else(|| {
                let mut page = self.page.clone();
                page.tracks = self.catalog.as_ref().clone();
                page
            });
        if page.continuation.is_none() {
            return false;
        }
        let epoch = if download {
            self.backend
                .download_epoch
                .load(std::sync::atomic::Ordering::Acquire)
        } else {
            self.backend
                .collection_epoch
                .store(self.queue_epoch, std::sync::atomic::Ordering::Release);
            self.queue_epoch
        };
        let result = self
            .backend
            .collections
            .try_send(crate::service::CollectionLoad {
                epoch,
                download,
                account: self.account_collection_key().is_some(),
                id: id.clone(),
                page,
                snapshot: self.auth.snapshot(),
                filter: if download {
                    String::new()
                } else {
                    self.browse_filter.clone()
                },
                sort: if download { 0 } else { self.browse_sort },
            });
        if result.is_err() {
            self.notice =
                Some(t("Collection loading queue is full. Wait for it to finish.").into());
        }
        true
    }
    pub(super) fn playlist_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut entries = self.playlist_entries(usize::MAX);
        entries.retain(|e| {
            e.track.title.to_lowercase().contains(&self.browse_filter)
                && (!e.local
                    || self
                        .library
                        .playlists
                        .iter()
                        .any(|p| p.id == e.track.id && p.smart.is_none()))
        });
        let count = entries.len();
        let entity = cx.entity();
        div()
            .flex()
            .flex_col()
            .size_full()
            .px_8()
            .gap_4()
            .child(self.title(t("Choose a destination for the selected music."), cx))
            .child(Input::new(&self.browse_query))
            .child(
                self.scroll
                    .bind_list(uniform_list(
                        "playlist-destinations",
                        count,
                        move |range, _, cx| {
                            entity.update(cx, |this, cx| {
                                range
                                    .map(|i| {
                                        let entry = entries[i].clone();
                                        let owned = entry.clone();
                                        content_button(SharedString::from(format!(
                                            "destination-{}",
                                            entry.track.id
                                        )))
                                        .h_16()
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .justify_start()
                                        .gap_3()
                                        .child(this.cover(&entry.track, rems(2.5), cx))
                                        .child(entry.track.title)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let tracks = std::mem::take(&mut this.picker_tracks);
                                            if owned.local {
                                                for track in tracks {
                                                    if let Err(error) = this
                                                        .library
                                                        .add_to_playlist(&owned.track.id, track)
                                                    {
                                                        this.notice = Some(error.to_string());
                                                        break;
                                                    }
                                                }
                                                this.navigation_revision =
                                                    this.navigation_revision.wrapping_add(1);
                                                this.save();
                                            } else {
                                                let tracks: Vec<_> = tracks
                                                    .into_iter()
                                                    .filter(|t| t.kind == ItemKind::Song)
                                                    .take(10_000)
                                                    .collect();
                                                for chunk in tracks.chunks(100) {
                                                    this.pending_edits.push_back(Edit::AddMany {
                                                        playlist: owned.track.id.clone(),
                                                        tracks: chunk.to_vec(),
                                                    });
                                                }
                                            }
                                            this.history_navigation(false, cx);
                                        }))
                                        .into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                            })
                        },
                    ))
                    .flex_1()
                    .min_h_0(),
            )
            .into_any_element()
    }
    pub(super) fn choose_destination(
        &mut self,
        tracks: Vec<Track>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker_tracks = tracks;
        self.browse_filter.clear();
        self.browse_query
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.navigate(Screen::PlaylistPicker, cx);
        if self.signed_in() {
            self.account_read(
                crate::account_sync::Key::Section(AccountSection::Playlists),
                false,
                cx,
            );
        }
    }
    pub(super) fn filter_and_sort(&self, tracks: &mut Vec<Track>) {
        crate::service::filter_tracks(tracks, &self.browse_filter, self.browse_sort);
    }
    pub(super) fn row_key(track: &Track) -> String {
        track
            .set_video_id
            .clone()
            .unwrap_or_else(|| track.id.clone())
    }
    pub(super) fn collection_tools(&self, cx: &mut Context<Self>) -> Div {
        let selected: Vec<_> = self
            .visible
            .iter()
            .filter(|t| {
                self.selecting
                    && self
                        .selected_rows
                        .contains(t.set_video_id.as_deref().unwrap_or(&t.id))
            })
            .cloned()
            .collect();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .child(Input::new(&self.browse_query).w(rems(18.)))
            .child(
                Button::new("sort-collection")
                    .ghost()
                    .label(
                        [t("Original order"), t("Title"), t("Artist"), t("Duration")]
                            [self.browse_sort],
                    )
                    .tooltip(t("Sort collection"))
                    .dropdown_menu({
                        let owner = cx.weak_entity();
                        let sort = self.browse_sort;
                        move |mut menu, _, _| {
                            for (index, label) in
                                [t("Original order"), t("Title"), t("Artist"), t("Duration")]
                                    .into_iter()
                                    .enumerate()
                            {
                                let owner = owner.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label).checked(sort == index).on_click(
                                        move |_, _, cx| {
                                            let _ = owner.update(cx, |this, cx| {
                                                this.browse_sort = index;
                                                this.scroll.reset();
                                                this.refresh_visible();
                                                cx.notify();
                                            });
                                        },
                                    ),
                                );
                            }
                            menu
                        }
                    }),
            )
            .child(
                Button::new("select-rows")
                    .ghost()
                    .label(if self.selecting {
                        t("Done")
                    } else {
                        t("Select")
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selecting = !this.selecting;
                        this.selected_rows.clear();
                        this.content_revision += 1;
                        cx.notify();
                    })),
            )
            .when(self.selecting, |body| {
                body.child(
                    Button::new("add-selected")
                        .ghost()
                        .label(t("Add to playlist…"))
                        .disabled(selected.is_empty())
                        .on_click(cx.listener(|this, _, window, cx| {
                            let tracks = this
                                .visible
                                .iter()
                                .filter(|t| this.selected_rows.contains(&Self::row_key(t)))
                                .cloned()
                                .collect();
                            this.choose_destination(tracks, window, cx);
                        })),
                )
                .child(
                    Button::new("select-all")
                        .ghost()
                        .label(t("Select all shown"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.selected_rows = this
                                .visible
                                .iter()
                                .filter(|t| t.playable())
                                .map(Self::row_key)
                                .collect();
                            this.content_revision += 1;
                            cx.notify();
                        })),
                )
                .when(
                    matches!(self.screen, Screen::Collection(..) | Screen::Local),
                    |body| {
                        body.child(
                            Button::new("remove-selected")
                                .ghost()
                                .label(t("Remove selected"))
                                .disabled(
                                    selected.is_empty()
                                        || matches!(&self.screen, Screen::Collection(id, _, true) if self.library.playlists.iter().any(|p| p.id == *id && p.smart.is_some()))
                                        || matches!(self.screen, Screen::Collection(_, _, false))
                                            && !self.page.editable,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.remove_selected(cx))),
                        )
                    },
                )
                .child(
                    Button::new("enqueue-selected")
                        .ghost()
                        .label(tr!("Queue {}", selected.len()))
                        .disabled(selected.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            for t in this
                                .visible
                                .iter()
                                .filter(|t| this.selected_rows.contains(&Self::row_key(t)))
                            {
                                this.queue.enqueue(t.clone(), false);
                            }
                            this.save();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("download-selected")
                        .ghost()
                        .label(t("Download selected"))
                        .disabled(selected.is_empty())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.download_tracks(selected.clone(), cx)
                        })),
                )
            })
            .when(matches!(self.screen, Screen::Local), |body| {
                body.child(
                    Button::new("rescan-local")
                        .ghost()
                        .label(t("Rescan folders"))
                        .on_click(cx.listener(|this, _, _, cx| this.rescan_local(cx))),
                )
            })
            .when(matches!(self.screen, Screen::Collection(..)), |body| {
                body.child(
                    Button::new("download-collection")
                        .ghost()
                        .icon(icon("download"))
                        .tooltip(t("Download collection"))
                        .accessibility_label(t("Download collection"))
                        .disabled(self.visible.is_empty() || self.download_status.active)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.complete_collection(true) {
                                let tracks = match &this.screen {
                                    Screen::Collection(id, _, true) => this
                                        .library
                                        .playlists
                                        .iter()
                                        .find(|p| p.id == *id)
                                        .map(|p| p.tracks.clone())
                                        .unwrap_or_default(),
                                    _ => this.catalog.as_ref().clone(),
                                };
                                this.download_tracks(tracks, cx);
                            }
                        })),
                )
            })
            .when(matches!(self.screen, Screen::Collection(_, _, false)), |body| {
                let Screen::Collection(id, _, _) = &self.screen else { return body; };
                let link = if id.starts_with("MPRE") { raag::links::Link::Album(id.clone()) }
                    else if id.starts_with("UC") { raag::links::Link::Artist(id.clone()) }
                    else { raag::links::Link::Playlist(id.strip_prefix("VL").unwrap_or(id).to_owned()) };
                body.child(Button::new("copy-collection-link").ghost().label(t("Copy music link"))
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(link.public_url()))))
            })
            .child(self.playlist_organization_tools(cx))
    }
    fn remove_selected(&mut self, cx: &mut Context<Self>) {
        let tracks: Vec<_> = self
            .visible
            .iter()
            .filter(|t| self.selected_rows.contains(&Self::row_key(t)))
            .cloned()
            .collect();
        match &self.screen {
            Screen::Collection(id, _, true) => {
                if let Some(playlist) = self
                    .library
                    .playlists
                    .iter_mut()
                    .find(|p| p.id == *id && p.smart.is_none())
                {
                    playlist
                        .tracks
                        .retain(|t| !self.selected_rows.contains(&Self::row_key(t)));
                }
                self.navigation_revision = self.navigation_revision.wrapping_add(1);
                self.save();
            }
            Screen::Collection(id, _, false) if self.page.editable => {
                for chunk in tracks.chunks(100) {
                    self.pending_edits.push_back(Edit::RemoveMany {
                        playlist: id.clone(),
                        tracks: chunk.to_vec(),
                    });
                }
            }
            Screen::Local => {
                self.library
                    .local
                    .retain(|t| !self.selected_rows.contains(&Self::row_key(t)));
                self.save();
            }
            _ => return,
        }
        self.selected_rows.clear();
        self.refresh_visible();
        cx.notify();
    }
    fn rescan_local(&mut self, cx: &mut Context<Self>) {
        let folders = self.library.local_folders.clone();
        let sender = self.backend.events_sender.clone();
        cx.background_spawn(async move {
            let mut library = Library::default();
            let count = library.import_paths(&folders);
            let _ = sender.send(Event::Imported(Box::new(library), count));
        })
        .detach();
    }
    pub(super) fn discovery_controls(&self, cx: &mut Context<Self>) -> Div {
        div().flex().flex_col().gap_4()
            .child(Switch::new("private-session").label(t("Private listening session")).checked(self.private_session)
                .on_change(cx.listener(|this, value, _, cx| {
                    this.private_session = *value;
                    this.listening = Default::default();
                    this.save();
                    cx.notify();
                })))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t("Until the app closes, new listening is excluded from local history, statistics and YouTube history sync. Queue restoration is disabled; existing history stays intact.")))
            .child(Button::new("export-listening-stats").ghost().label(t("Export listening statistics…")).on_click(cx.listener(|this, _, _, cx| this.export_stats(cx))))
            .child(Switch::new("sync-listening-history").label(t("Sync listened songs to YouTube Music")).checked(self.library.sync_history)
                .on_change(cx.listener(|this, value, _, cx| { this.library.sync_history = *value; this.save(); cx.notify(); })))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t("A song is submitted after 70% actual listening. Local listening statistics stay on this device.")))
            .child(Button::new("discovery-mode").ghost().label(tr!("Radio discovery: {}", [t("Balanced"), t("Familiar"), t("New to you")][self.library.discovery_mode.min(2)]))
                .on_click(cx.listener(|this, _, _, cx| { this.library.discovery_mode = (this.library.discovery_mode + 1) % 3; this.save(); this.content_revision += 1; cx.notify(); })))
            .child(Button::new("reset-hidden").ghost().label(tr!("Reset hidden recommendations ({})", self.library.hidden.len()))
                .on_click(cx.listener(|this, _, _, cx| { this.library.hidden.clear(); this.save(); this.refresh_visible(); cx.notify(); })))
    }
    fn export_stats(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_new_path(&data_dir(), Some("cassette-listening.json"));
        let mut stats = self.library.stats.clone();
        for entry in stats.tracks.values_mut() {
            entry.track.library_add = None;
            entry.track.library_remove = None;
            entry.track.set_video_id = None;
        }
        cx.spawn(async move |owner, cx| {
            if let Ok(Ok(Some(path))) = prompt.await {
                let result = cx
                    .background_spawn(async move {
                        raag::cache::write_json(&path, &stats).map_err(|e| e.to_string())
                    })
                    .await;
                let _ = owner.update(cx, |this, cx| {
                    if let Err(error) = result {
                        this.notice = Some(error);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }
    pub(super) fn audio_controls(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let owner = cx.weak_entity();
        let devices = self.audio_devices.clone();
        let selected = self.library.audio.output.clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(Switch::new("jiosaavn-source").label(t("Try JioSaavn audio first"))
                .checked(self.library.audio.jiosaavn)
                .on_change(cx.listener(|this, value, _, cx| { this.library.audio.jiosaavn = *value; this.apply_audio(cx); })))
            .child(div().text_sm().text_color(cx.theme().muted_foreground)
                .child(t("YouTube Music provides your catalog and library. JioSaavn can provide matching AAC audio up to 320 kb/s; YouTube is the fallback.")))
            .child(Button::new("streaming-quality").label(tr!("Streaming quality: {}", self.library.audio.streaming_quality.label()))
                .on_click(cx.listener(|this, _, _, cx| { this.library.audio.streaming_quality = this.library.audio.streaming_quality.next(); this.apply_audio(cx); })))
            .child(Button::new("download-quality").label(tr!("Download quality: {}", self.library.audio.download_quality.label()))
                .on_click(cx.listener(|this, _, _, cx| { this.library.audio.download_quality = this.library.audio.download_quality.next(); this.apply_audio(cx); })))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t("Applies to newly fetched audio. Existing downloads and the buffered next song are kept. Quality depends on the available recording and source.")))
            .child(
                Switch::new("normalize")
                    .label(t("Normalize loudness"))
                    .checked(self.library.audio.normalize)
                    .on_change(cx.listener(|this, value, _, cx| {
                        this.library.audio.normalize = *value;
                        this.apply_audio(cx);
                    })),
            )
            .child(
                Button::new("audio-output")
                    .label(tr!(
                        "Output: {}",
                        devices
                            .iter()
                            .find(|(id, _)| id == &selected)
                            .map_or(t("System default"), |(_, name)| name.as_str())
                    ))
                    .dropdown_menu(move |mut menu, _, _| {
                        for (id, name) in
                            std::iter::once(("auto".to_owned(), t("System default").to_owned()))
                                .chain(devices.clone())
                        {
                            let owner = owner.clone();
                            menu = menu.item(PopupMenuItem::new(name).on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.library.audio.output = id.clone();
                                    this.apply_audio(cx);
                                });
                            }));
                        }
                        menu
                    }),
            );
        body = body.child(self.equalizer_graph(window, cx)).child(
            Button::new("crossfade")
                .label(tr!(
                    "Crossfade: {:.0} seconds",
                    self.library.audio.crossfade
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.library.audio.crossfade = (this.library.audio.crossfade + 2.) % 12.;
                    this.apply_audio(cx);
                })),
        );
        let mut timer = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(t("Sleep timer"));
        for minutes in [15usize, 30, 60] {
            timer = timer.child(
                Button::new(("sleep", minutes))
                    .ghost()
                    .label(tr!("{minutes} min"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sleep_until =
                            Some(Instant::now() + Duration::from_secs(minutes as u64 * 60));
                        this.sleep_end = false;
                        this.content_revision += 1;
                        cx.notify();
                    })),
            );
        }
        body.child(
            timer
                .child(
                    Button::new("sleep-end")
                        .ghost()
                        .label(t("End of song"))
                        .selected(self.sleep_end)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sleep_end = true;
                            this.sleep_until = None;
                            this.content_revision += 1;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("sleep-cancel")
                        .ghost()
                        .label(t("Cancel"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sleep_until = None;
                            this.sleep_end = false;
                            this.content_revision += 1;
                            cx.notify();
                        })),
                ),
        )
        .when_some(self.sleep_until, |body, deadline| {
            body.child(div().text_sm().child(tr!(
                    "Pausing in {} min",
                    deadline
                        .saturating_duration_since(Instant::now())
                        .as_secs()
                        .div_ceil(60)
                )))
        })
    }
    pub(super) fn apply_audio(&mut self, cx: &mut Context<Self>) {
        self.library.audio.validate();
        self.player
            .command(PlayerCommand::Configure(self.library.audio.clone()));
        self.save();
        self.content_revision += 1;
        cx.notify();
    }
    pub(super) fn offline_controls(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.download_status;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .flex_shrink_0()
            .child(div().text_sm().child(if state.active {
                tr!(
                    "Downloading {} · {}/{}",
                    state.current,
                    state.completed,
                    state.total
                )
            } else {
                tr!(
                    "{} available songs · {} pinned · {:.0} MiB used",
                    self.offline.len(),
                    state.pinned,
                    state.bytes as f64 / 1048576.
                )
            }))
            .when_some(state.error.clone(), |body, error| {
                body.child(div().text_sm().child(error))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("cancel-downloads")
                            .ghost()
                            .label(t("Cancel downloads"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.downloads.cancel();
                                this.backend
                                    .download_epoch
                                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                                this.content_revision += 1;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("clear-audio-cache")
                            .ghost()
                            .label(t("Clear unpinned cache"))
                            .on_click(cx.listener(|this, _, _, _| {
                                this.downloads
                                    .submit(crate::downloads::Job::Remove(None, true));
                            })),
                    )
                    .child(
                        Button::new("audio-budget")
                            .ghost()
                            .label(tr!(
                                "Storage limit: {} GiB",
                                self.library.audio_budget / 1073741824
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.library.audio_budget =
                                    match this.library.audio_budget / 1073741824 {
                                        2 => 4,
                                        4 => 8,
                                        8 => 16,
                                        _ => 2,
                                    } * 1073741824;
                                raag::cache::configure(
                                    this.library.audio_budget,
                                    this.library.offline_only,
                                );
                                this.save();
                                this.content_revision += 1;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Switch::new("offline-only")
                    .label(t("Offline mode"))
                    .checked(self.library.offline_only)
                    .on_change(cx.listener(|this, value, _, cx| {
                        this.library.offline_only = *value;
                        raag::cache::configure(this.library.audio_budget, *value);
                        this.save();
                        this.content_revision += 1;
                        cx.notify();
                    })),
            )
    }
    pub(super) fn pump_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use souvlaki::{MediaControlEvent as E, SeekDirection};
        while let Some(event) = self.media.as_ref().and_then(|m| m.events.try_recv().ok()) {
            match event {
                E::Play if !self.playing => self.toggle_playback(cx),
                E::Pause if self.playing => self.toggle_playback(cx),
                E::Toggle => self.toggle_playback(cx),
                E::Next => self.next(false, cx),
                E::Previous => self.previous(cx),
                E::Stop => {
                    self.player.command(PlayerCommand::Stop);
                    self.playing = false;
                    self.buffering = false;
                    self.failed = self.selected.is_some();
                    self.save();
                    cx.notify();
                }
                E::SetPosition(p) => self.commit_seek(p.0.as_secs_f64(), cx),
                E::Seek(direction) => self.commit_seek(
                    self.position
                        + if direction == SeekDirection::Forward {
                            10.
                        } else {
                            -10.
                        },
                    cx,
                ),
                E::SeekBy(direction, duration) => self.commit_seek(
                    self.position
                        + duration.as_secs_f64()
                            * if direction == SeekDirection::Forward {
                                1.
                            } else {
                                -1.
                            },
                    cx,
                ),
                E::SetVolume(volume) if volume.is_finite() => {
                    self.library.volume = (volume * 100.).clamp(0., 100.);
                    self.volume.update(cx, |s, cx| {
                        s.set_value(self.library.volume as f32, window, cx)
                    });
                    self.player
                        .command(PlayerCommand::Volume(self.library.volume));
                    self.save();
                    cx.notify();
                }
                E::Raise => crate::system::show(window, cx),
                E::Quit => {
                    self.backend.flush(self.snapshot());
                    cx.quit();
                }
                E::OpenUri(uri) => {
                    if let Ok(link) = raag::links::Link::parse(&uri) {
                        self.open_link(link, cx);
                    }
                }
                _ => {}
            }
        }
        let artwork = self
            .selected
            .as_ref()
            .and_then(|track| self.artwork_path(track))
            .and_then(|path| reqwest::Url::from_file_path(path).ok())
            .map(|url| url.to_string());
        if let Some(media) = &mut self.media {
            media.update(
                self.selected.as_ref(),
                self.playing,
                self.position,
                self.duration,
                self.library.volume,
                artwork,
            );
        }
    }
    pub(super) fn download_tracks(&mut self, tracks: Vec<Track>, cx: &mut Context<Self>) {
        if !self.downloads.submit(crate::downloads::Job::Tracks(
            tracks,
            self.library.ytdlp_path.clone(),
            self.playback_cookies(),
        )) {
            self.notice = Some(t("Download queue is full. Wait for a download to finish.").into());
        }
        self.content_revision += 1;
        cx.notify();
    }
    pub(super) fn request_radio(&mut self) {
        let generation = self.player.generation();
        if self.radio_pending == Some(generation) || self.radio_attempted == Some(generation) {
            return;
        }
        if let Some(track) = self.selected.as_ref().filter(|t| t.kind == ItemKind::Song) {
            self.radio_pending = Some(generation);
            self.radio_attempted = Some(generation);
            let _ = self.backend.radio.send((generation, track.clone()));
        }
    }
}
