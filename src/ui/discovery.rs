use super::*;

impl Raag {
    pub(super) fn page_heading(
        &self,
        title: impl Into<SharedString>,
        subtitle: impl Into<SharedString>,
        cx: &App,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .flex_shrink_0()
            .pt_3()
            .pb_2()
            .child(
                div()
                    .text_3xl()
                    .font_family(raag::i18n::font())
                    .font_weight(FontWeight::BOLD)
                    .child(title.into()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(subtitle.into()),
            )
    }
    pub(super) fn section_heading(&self, title: &str) -> Div {
        div()
            .text_xl()
            .font_family(raag::i18n::font())
            .font_weight(FontWeight::SEMIBOLD)
            .child(title.to_owned())
    }
    fn filter_bar(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div().flex().gap_2().flex_shrink_0();
        for (ix, label) in [
            t("All"),
            t("Songs"),
            t("Albums"),
            t("Artists"),
            t("Playlists"),
        ]
        .into_iter()
        .enumerate()
        {
            row = row.child(
                Button::new(("search-kind", ix))
                    .ghost()
                    .rounded_full()
                    .label(label)
                    .selected(self.search_filter == ix)
                    .on_click(cx.listener(move |this, _, _, cx| this.filter_search(ix, cx))),
            );
        }
        row
    }
    fn filter_search(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.search_filter = ix;
        self.pagination_attempts.clear();
        let query = self.search_query.clone();
        let kind = match ix {
            1 => Some(ItemKind::Song),
            2 => Some(ItemKind::Album),
            3 => Some(ItemKind::Artist),
            4 => Some(ItemKind::Playlist),
            _ => None,
        };
        self.catalog = Arc::new(vec![]);
        self.page = raag::catalog::Page::default();
        self.refresh_visible();
        self.scroll.reset();
        self.request(|id| Request::Search(id, query, kind), cx);
    }
    pub(super) fn gallery_geometry(&self, window: &Window) -> (usize, f32) {
        let width = (self.content_width(window) - 64.).max(280.);
        // Keep row membership fixed while pane edges and card sizes move.
        let density_width = (self.layout_width(window) - 64.).max(160.);
        let columns = ((density_width + 20.) / 184.).floor().clamp(1., 8.) as usize;
        (
            columns,
            ((width - 20. * (columns - 1) as f32) / columns as f32).min(320.),
        )
    }
    fn display_card(&self, track: &Track, extent: f32, cx: &mut Context<Self>) -> AnyElement {
        let owned = track.clone();
        let artist = track.kind == ItemKind::Artist;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(extent))
            .flex_shrink_0()
            .child(
                content_button(SharedString::from(format!("discover-{}", track.id)))
                    .size(px(extent))
                    .overflow_hidden()
                    .rounded_lg()
                    .when(artist, |button| button.rounded_full())
                    .cursor_pointer()
                    .hover(|button| button.opacity(0.86))
                    .accessibility_label(format!(
                        "{} {}",
                        if track.playable() {
                            t("Play")
                        } else {
                            t("Open")
                        },
                        track.title
                    ))
                    .child(self.cover(track, rems(extent / f32::from(cx.theme().font_size)), cx))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_track(owned.clone(), cx)),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(self.artist_name(track, cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .min_w_0()
                    .child(if artist {
                        div().child(t("Artist")).into_any_element()
                    } else {
                        self.artist_link(track, cx)
                    }),
            )
            .into_any_element()
    }
    fn card_grid(
        &self,
        tracks: &[Track],
        columns: usize,
        extent: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut grid = div().flex().flex_col().gap_6().flex_shrink_0();
        for chunk in tracks.chunks(columns) {
            let mut row = div().flex().gap_5().items_start().flex_shrink_0();
            for track in chunk {
                row = row.child(self.display_card(track, extent, cx));
            }
            grid = grid.child(row);
        }
        grid
    }
    fn shelf(
        &self,
        title: &str,
        tracks: &[Track],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filtered;
        let tracks = if self.library.hidden.is_empty() {
            tracks
        } else {
            filtered = tracks
                .iter()
                .filter(|t| !self.library.hidden.contains(&t.id))
                .cloned()
                .collect::<Vec<_>>();
            &filtered
        };
        let (columns, extent) = self.gallery_geometry(window);
        div()
            .id(SharedString::from(format!(
                "shelf-{title}-{}",
                tracks.first().map(|t| t.id.as_str()).unwrap_or("empty")
            )))
            .flex()
            .flex_col()
            .gap_4()
            .flex_shrink_0()
            .child(self.section_heading(title))
            .child(self.card_grid(tracks, columns, extent, cx))
            .into_any_element()
    }
    pub(super) fn song_tile(&self, track: &Track, cx: &mut Context<Self>) -> AnyElement {
        let owned = track.clone();
        let current = self
            .selected
            .as_ref()
            .is_some_and(|song| song.id == track.id);
        content_button(SharedString::from(format!("song-tile-{}", track.id)))
            .flex()
            .justify_start()
            .items_center()
            .gap_3()
            .w_full()
            .h(ui_px(68.))
            .px_2()
            .rounded_lg()
            .when(current, |button| button.bg(cx.theme().muted))
            .hover(|button| button.bg(cx.theme().muted))
            .accessibility_label(tr!("Play {} by {}", track.title, track.artist))
            .child(self.cover(track, rems(2.75), cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(self.artist_name(track, cx)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(self.artist_link(track, cx)),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.open_track(owned.clone(), cx)))
            .into_any_element()
    }
    pub(super) fn explore_screen(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let charts = matches!(&self.screen, Screen::Discover(id, _, _) if id == "FEmusic_charts");
        let genres =
            matches!(&self.screen, Screen::Discover(id, _, _) if id == "FEmusic_moods_and_genres");
        let title = if self.screen == Screen::Explore {
            t("Explore")
        } else {
            self.screen.title()
        };
        let subtitle = if charts {
            t("Trending on YouTube Music · India")
        } else if genres {
            t("A soundtrack for every mood. A place for every genre.")
        } else {
            t("New releases, favourite artists and your next discovery.")
        };
        let mut content = self
            .scroll
            .bind(div().id("discovery-scroll"))
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .px_8()
            .pb_8()
            .gap_7()
            .child(self.page_heading(title, subtitle, cx));
        let mut routes = div().flex().gap_3().flex_shrink_0();
        for (label, id) in [
            (t("For you"), "FEmusic_explore"),
            (t("Charts"), "FEmusic_charts"),
            (t("Moods & genres"), "FEmusic_moods_and_genres"),
            (t("New releases"), "FEmusic_new_releases"),
        ] {
            let screen = Screen::Discover(id.into(), String::new(), label.into());
            let selected = matches!(&self.screen, Screen::Discover(current, _, _) if current == id)
                || (id == "FEmusic_explore" && self.screen == Screen::Explore);
            routes = routes.child(
                Button::new(id)
                    .ghost()
                    .rounded_full()
                    .label(label)
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigate(screen.clone(), cx);
                        this.reload(cx);
                    })),
            );
        }
        content = content.child(routes);
        if !self.page.destinations.is_empty() {
            let mut tiles = div().flex().flex_wrap().gap_3().flex_shrink_0();
            let width = ((self.content_width(window) - 64. - 36.) / 4.).floor();
            for destination in self
                .page
                .destinations
                .iter()
                .filter(|d| {
                    !matches!(
                        d.title.as_str(),
                        "New releases" | "Charts" | "Moods & genres" | "Podcasts"
                    )
                })
                .take(if self.screen == Screen::Explore {
                    8
                } else {
                    96
                })
            {
                let screen = Screen::Discover(
                    destination.id.clone(),
                    destination.params.clone(),
                    destination.title.clone(),
                );
                tiles = tiles.child(
                    content_button(SharedString::from(format!("genre-{}", destination.title)))
                        .flex()
                        .items_center()
                        .w(px(width))
                        .h(ui_px(76.))
                        .px_5()
                        .rounded_xl()
                        .bg(cx.theme().primary.opacity(0.10))
                        .border_1()
                        .border_color(cx.theme().primary.opacity(0.16))
                        .hover(|button| button.bg(cx.theme().primary.opacity(0.20)))
                        .accessibility_label(tr!("Explore {}", destination.title))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(destination.title.clone()),
                        )
                        .child(icon("nav-arrow-right"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.navigate(screen.clone(), cx);
                            this.reload(cx);
                        })),
                );
            }
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .flex_shrink_0()
                    .child(self.section_heading(if genres {
                        t("Find your mood")
                    } else {
                        t("Moods & genres")
                    }))
                    .child(tiles),
            );
        }
        if self.loading && self.visible.is_empty() {
            content = content.child(self.discovery_loading(cx));
        }
        for section in &self.page.sections {
            if charts && section.tracks.iter().any(Track::playable) {
                let mut songs = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_shrink_0()
                    .child(self.section_heading(&section.title));
                for (ix, track) in section.tracks.iter().filter(|t| t.playable()).enumerate() {
                    songs = songs.child(self.track_row(track, ix, cx));
                }
                content = content.child(songs);
            } else {
                content = content.child(self.shelf(
                    if section.title.is_empty() {
                        t("Explore music")
                    } else {
                        &section.title
                    },
                    &section.tracks,
                    window,
                    cx,
                ));
            }
        }
        if self.page.sections.is_empty() && !self.visible.is_empty() {
            let (columns, extent) = self.gallery_geometry(window);
            content = content.child(self.card_grid(&self.visible, columns, extent, cx));
        } else if !self.loading && self.visible.is_empty() && self.page.destinations.is_empty() {
            content = content.child(self.empty_state(cx));
        } else if !self.page.sections.is_empty() {
            let shown: std::collections::HashSet<_> = self
                .page
                .sections
                .iter()
                .flat_map(|section| section.tracks.iter().map(|track| track.id.as_str()))
                .collect();
            let remaining: Vec<_> = self
                .visible
                .iter()
                .filter(|track| !shown.contains(track.id.as_str()))
                .cloned()
                .collect();
            if !remaining.is_empty() {
                let (columns, extent) = self.gallery_geometry(window);
                content = content.child(self.card_grid(&remaining, columns, extent, cx));
            }
        }
        content
            .when_some(self.continuation.as_ref(), |div, _| {
                div.child(self.pagination_status(cx))
            })
            .into_any_element()
    }
    fn discovery_loading(&self, cx: &App) -> Div {
        let mut placeholders = div().flex().gap_5().flex_shrink_0();
        for _ in 0..4 {
            placeholders = placeholders.child(
                div()
                    .flex_1()
                    .h(ui_px(156.))
                    .rounded_lg()
                    .bg(cx.theme().muted),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_4()
            .flex_shrink_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(gpui_kit::component::spinner::Spinner::new().small())
                    .child(t("Loading music…")),
            )
            .child(placeholders)
    }
    pub(super) fn pagination_status(&self, cx: &App) -> Div {
        div()
            .h(ui_px(28.))
            .flex_shrink_0()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .flex()
            .items_center()
            .justify_center()
            .when(self.loading, |div| div.child(t("Loading more…")))
    }
    pub(super) fn search_screen(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let heading = tr!("Results for “{}”", self.search_query);
        let header = self.page_heading(heading, t("Songs, albums and the people behind them."), cx);
        if self.search_filter > 0 {
            let mut body = div()
                .flex()
                .flex_col()
                .size_full()
                .px_8()
                .pb_6()
                .gap_5()
                .child(header)
                .child(self.filter_bar(cx));
            if self.visible.is_empty() {
                body = body.child(if self.loading {
                    self.discovery_loading(cx).into_any_element()
                } else {
                    self.empty_state(cx)
                });
            } else if self.search_filter == 1 {
                body = body.child(self.track_list(cx));
            } else {
                // Albums and artists are virtualized by rows, just like song results.
                let (columns, extent) = self.gallery_geometry(window);
                let tracks = self.visible.clone();
                let entity = cx.entity();
                body = body.child(
                    self.scroll.bind_list(
                        uniform_list(
                            "search-card-rows",
                            tracks.len().div_ceil(columns),
                            move |range, _, cx| {
                                entity.update(cx, |this, cx| {
                                    range
                                        .map(|row| {
                                            let start = row * columns;
                                            div().h(px(extent + 76.)).child(this.card_grid(
                                                &tracks[start..tracks.len().min(start + columns)],
                                                columns,
                                                extent,
                                                cx,
                                            ))
                                        })
                                        .collect::<Vec<_>>()
                                })
                            },
                        )
                        .size_full(),
                    ),
                );
            }
            return body
                .when_some(self.continuation.as_ref(), |div, _| {
                    div.child(self.pagination_status(cx))
                })
                .into_any_element();
        }
        let mut body = self
            .scroll
            .bind(div().id("search-results"))
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .px_8()
            .pb_8()
            .gap_8()
            .child(header)
            .child(self.filter_bar(cx));
        if self.loading && self.visible.is_empty() {
            body = body.child(div().px_8().child(self.discovery_loading(cx)));
        }
        let available = self.content_width(window) - 64.;
        let density_width = self.layout_width(window) - 64.;
        let stacked = density_width < 650.;
        let extent = (if stacked {
            (available - 40.) / 3.
        } else {
            (available - 32. - 80.) / 6.
        })
        .min(180.);
        let mut upper = div()
            .flex()
            .gap_8()
            .items_start()
            .flex_shrink_0()
            .when(stacked, |div| div.flex_col());
        for (title, kind, filter) in [
            (t("Artists"), ItemKind::Artist, 3),
            (t("Albums"), ItemKind::Album, 2),
        ] {
            let tracks: Vec<_> = self
                .visible
                .iter()
                .filter(|t| t.kind == kind)
                .take(3)
                .cloned()
                .collect();
            if tracks.is_empty() {
                continue;
            }
            upper = upper.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(self.section_heading(title))
                            .child(
                                Button::new(SharedString::from(format!("all-{title}")))
                                    .ghost()
                                    .small()
                                    .label(t("Show all"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.filter_search(filter, cx)
                                    })),
                            ),
                    )
                    .child(self.card_grid(&tracks, 3, extent, cx)),
            );
        }
        body = body.child(upper);
        let songs: Vec<_> = self
            .visible
            .iter()
            .filter(|t| t.playable())
            .take(12)
            .collect();
        if !songs.is_empty() {
            let columns = if density_width >= 1000. {
                4
            } else if density_width < 650. {
                2
            } else {
                3
            };
            let mut grid = div().flex().flex_col().gap_2();
            for chunk in songs.chunks(columns) {
                let mut row = div().flex().gap_4().flex_shrink_0();
                for track in chunk {
                    row = row.child(div().flex_1().min_w_0().child(self.song_tile(track, cx)));
                }
                for _ in chunk.len()..columns {
                    row = row.child(div().flex_1().min_w_0());
                }
                grid = grid.child(row);
            }
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(self.section_heading(t("Songs")))
                            .child(
                                Button::new("all-songs")
                                    .ghost()
                                    .small()
                                    .label(t("Show all songs"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.filter_search(1, cx)),
                                    ),
                            ),
                    )
                    .child(grid),
            );
        }
        let playlists: Vec<_> = self
            .visible
            .iter()
            .filter(|t| t.kind == ItemKind::Playlist)
            .cloned()
            .collect();
        if !playlists.is_empty() {
            body = body.child(self.shelf(t("Playlists"), &playlists, window, cx));
        }
        let mut previewed = std::collections::HashSet::new();
        for kind in [
            ItemKind::Artist,
            ItemKind::Album,
            ItemKind::Song,
            ItemKind::Playlist,
        ] {
            let limit = match kind {
                ItemKind::Artist | ItemKind::Album => 3,
                ItemKind::Song => 12,
                _ => usize::MAX,
            };
            previewed.extend(
                self.visible
                    .iter()
                    .filter(|track| track.kind == kind)
                    .take(limit)
                    .map(|track| track.id.as_str()),
            );
        }
        let remaining: Vec<_> = self
            .visible
            .iter()
            .filter(|track| !previewed.contains(track.id.as_str()))
            .cloned()
            .collect();
        if !remaining.is_empty() {
            let (columns, extent) = self.gallery_geometry(window);
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .flex_shrink_0()
                    .child(self.section_heading(t("More results")))
                    .child(self.card_grid(&remaining, columns, extent, cx)),
            );
        }
        if !self.loading && self.visible.is_empty() {
            body = body.child(self.empty_state(cx));
        }
        body.when_some(self.continuation.as_ref(), |div, _| {
            div.child(self.pagination_status(cx))
        })
        .into_any_element()
    }
    pub(super) fn collection_art(&self) -> Track {
        // Most navigations already have the listing record. Only consult stored
        // playlists on history/direct navigation; never clone the entire library.
        let stored = if self
            .collection_seed
            .as_ref()
            .is_none_or(|seed| seed.artwork.is_none())
        {
            match &self.screen {
                Screen::Collection(id, _, true) => self
                    .library
                    .playlists
                    .iter()
                    .find(|playlist| playlist.id == *id)
                    .map(|playlist| {
                        let mut track = Track::song(&playlist.id, &playlist.name, "");
                        track.kind = ItemKind::Playlist;
                        track.artwork =
                            playlist.tracks.iter().find_map(|song| song.artwork.clone());
                        track
                    }),
                Screen::Collection(id, _, false) => self
                    .account
                    .page(&crate::account_sync::Key::Section(
                        AccountSection::Playlists,
                    ))
                    .and_then(|page| {
                        page.tracks.iter().find(|track| {
                            track.id.strip_prefix("VL").unwrap_or(&track.id)
                                == id.strip_prefix("VL").unwrap_or(id)
                        })
                    })
                    .cloned(),
                _ => None,
            }
        } else {
            None
        };
        let seed = stored.as_ref().or(self.collection_seed.as_ref());
        let artist = matches!(&self.screen, Screen::Collection(id, _, _) if id.starts_with("UC") || id.starts_with("artist:"));
        let mut hero = collection_art(seed, &self.page, self.screen.title(), artist);
        if matches!(&self.screen, Screen::Collection(id, _, _) if id.starts_with("MPRE")) {
            hero.kind = ItemKind::Album;
            hero.artists.clone_from(&self.page.artists);
            hero.artist = self
                .page
                .artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(" & ");
        }
        if hero.artwork.is_none() {
            hero.artwork = self.catalog.iter().find_map(|track| track.artwork.clone());
        }
        if let Screen::Collection(id, _, _) = &self.screen
            && let Some(path) = self
                .library
                .playlist_metadata
                .get(id.strip_prefix("VL").unwrap_or(id))
                .and_then(|m| m.cover.as_ref())
        {
            hero.artwork = Some(path.to_string_lossy().into_owned());
        }
        hero
    }
    fn collection_hero(&self, artist: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let hero = self.collection_art();
        let title = if self.page.title.is_empty() {
            self.screen.title()
        } else {
            &self.page.title
        };
        let mut actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .child(
                Button::new("collection-play")
                    .primary()
                    .map(playback_depth)
                    .rounded_full()
                    .when(artist, |button| {
                        button.bg(rgb(0xffffff)).text_color(rgb(0x15151a))
                    })
                    .icon(icon("play"))
                    .label(if artist {
                        t("Play top songs")
                    } else {
                        t("Play")
                    })
                    .disabled(!self.visible.iter().any(Track::playable))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(track) = this.visible.iter().find(|t| t.playable()).cloned() {
                            this.open_track(track, cx);
                        }
                    })),
            )
            .child(
                Button::new("collection-shuffle")
                    .ghost()
                    .rounded_full()
                    .when(artist, |button| button.text_color(rgb(0xffffff)))
                    .icon(icon("shuffle"))
                    .label(t("Shuffle"))
                    .disabled(!self.visible.iter().any(Track::playable))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(track) = this.visible.iter().find(|t| t.playable()).cloned() {
                            this.queue.shuffle = true;
                            this.queue
                                .replace_selected(this.visible.as_ref().clone(), &track);
                            this.invalidate_collection();
                            this.complete_collection(false);
                            this.start(track, cx);
                        }
                    })),
            );
        if let Screen::Collection(id, _, false) = &self.screen
            && self.signed_in()
        {
            if artist && id.starts_with("UC") {
                let id = id.clone();
                let followed = self.page.subscribed.unwrap_or_else(|| {
                    self.account_tracks(AccountSection::Artists)
                        .iter()
                        .any(|t| t.id == id)
                });
                actions = actions.child(
                    Button::new("follow-artist")
                        .ghost()
                        .rounded_full()
                        .text_color(rgb(0xffffff))
                        .label(if followed {
                            t("Following")
                        } else {
                            t("Follow")
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.account_edit(
                                Edit::Follow {
                                    artist: id.clone(),
                                    followed: !followed,
                                },
                                cx,
                            )
                        })),
                );
            } else if !artist {
                let playlist = if id.starts_with("MPRE") {
                    self.page.playlist_id.clone()
                } else {
                    Some(id.strip_prefix("VL").unwrap_or(id).to_owned())
                };
                if let Some(playlist) = playlist {
                    let saved = self
                        .account_tracks(if id.starts_with("MPRE") {
                            AccountSection::Albums
                        } else {
                            AccountSection::Playlists
                        })
                        .iter()
                        .any(|t| {
                            t.id.strip_prefix("VL").unwrap_or(&t.id)
                                == id.strip_prefix("VL").unwrap_or(id)
                        });
                    actions = actions.child(
                        Button::new("save-collection")
                            .ghost()
                            .rounded_full()
                            .label(if saved {
                                t("Remove from library")
                            } else {
                                t("Save to library")
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.account_edit(
                                    Edit::RateCollection {
                                        playlist: playlist.clone(),
                                        saved: !saved,
                                    },
                                    cx,
                                )
                            })),
                    );
                }
            }
        }
        if let Screen::Collection(id, name, true) = &self.screen {
            let owner = cx.weak_entity();
            let id = id.clone();
            let name = name.clone();
            let no_track = self.selected.is_none();
            actions = actions.child(
                Button::new("collection-edit")
                    .ghost()
                    .icon(icon("more"))
                    .tooltip(t("Playlist actions"))
                    .accessibility_label(t("Playlist actions"))
                    .dropdown_menu(move |mut menu, window, cx| {
                        for (label, command) in [
                            (t("Add playing song"), 0),
                            (t("Rename playlist"), 1),
                            (t("Export playlist…"), 2),
                            (t("Remove playlist…"), 3),
                        ] {
                            let owner = owner.clone();
                            let id = id.clone();
                            let name = name.clone();
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .disabled(command == 0 && no_track)
                                    .on_click(move |_, window, cx| {
                                        let _ = owner.update(cx, |this, cx| match command {
                                            0 => {
                                                if let Some(track) = this.selected.clone() {
                                                    if let Err(error) =
                                                        this.library.add_to_playlist(&id, track)
                                                    {
                                                        this.notice = Some(error.to_string());
                                                    }
                                                    this.navigation_revision =
                                                        this.navigation_revision.wrapping_add(1);
                                                    this.save();
                                                    self_refresh(this, cx);
                                                }
                                            }
                                            1 => {
                                                this.playlist_editor_remote = false;
                                                this.editing_playlist = true;
                                                this.editing_description = false;
                                                this.content_revision += 1;
                                                this.playlist_name.update(cx, |input, cx| {
                                                    input.set_value(name.clone(), window, cx);
                                                    input.focus(window, cx);
                                                });
                                                cx.notify();
                                            }
                                            2 => this.export_playlist(id.clone(), cx),
                                            _ => this.remove_playlist(id.clone(), window, cx),
                                        });
                                    }),
                            );
                        }
                        {
                            let actions = owner
                                .upgrade()
                                .unwrap()
                                .read(cx)
                                .collection_actions(owner.clone());
                            actions(menu, window, cx)
                        }
                    }),
            );
        }
        if let Some(crate::account_sync::Key::Playlist(id)) = self.account_collection_key()
            && self
                .account
                .page(&crate::account_sync::Key::Playlist(id.clone()))
                .is_some_and(|page| page.editable)
        {
            actions = actions.child(self.account_playlist_menu(id, cx));
        }
        if matches!(self.screen, Screen::Collection(_, _, false)) && !self.page.editable {
            let owner = cx.weak_entity();
            actions = actions.child(
                Button::new("collection-actions")
                    .ghost()
                    .icon(icon("more"))
                    .tooltip(t("Playlist actions"))
                    .accessibility_label(t("Playlist actions"))
                    .dropdown_menu(move |menu, window, cx| {
                        let actions = owner
                            .upgrade()
                            .unwrap()
                            .read(cx)
                            .collection_actions(owner.clone());
                        actions(menu, window, cx)
                    }),
            );
        }
        let background = cx.theme().background;
        if artist {
            let height = (self.content_width(window) * 0.36).clamp(420., 640.);
            return div()
                .relative()
                .h(px(height))
                .w_full()
                .flex_shrink_0()
                .overflow_hidden()
                .bg(cx.theme().muted)
                .child(self.hero_visual(&hero, cx))
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .w_full()
                        .h(px(height * 0.68))
                        .bg(linear_gradient(
                            0.,
                            linear_color_stop(rgb(0x08090c).opacity(0.88), 0.),
                            linear_color_stop(rgb(0x08090c).opacity(0.), 1.),
                        )),
                )
                .child(
                    div()
                        .absolute()
                        .bottom(ui_px(42.))
                        .left_8()
                        .right_8()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_4()
                        .text_color(rgb(0xffffff))
                        .child(
                            div()
                                .font_family(raag::i18n::font())
                                .text_size(px(if self.layout_width(window) > 800. {
                                    60.
                                } else {
                                    42.
                                }))
                                .line_height(relative(1.08))
                                .font_weight(FontWeight::BOLD)
                                .text_left()
                                .child(title.to_owned()),
                        )
                        .child(actions),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .w_full()
                        .h(ui_px(40.))
                        .bg(linear_gradient(
                            0.,
                            linear_color_stop(background, 0.),
                            linear_color_stop(background.opacity(0.), 1.),
                        )),
                )
                .into_any_element();
        }
        let playable = self.visible.iter().filter(|track| track.playable());
        let count = playable.clone().count();
        let seconds: u64 = playable.map(|track| track.duration).sum();
        let info = if seconds > 0 {
            tr!("{count} songs · {} min", seconds / 60)
        } else {
            tr!("{count} songs")
        };
        let width = self.content_width(window).max(320.);
        let narrow = ((1100. - width) / 220.).clamp(0., 1.);
        let stacked = ((700. - width) / 100.).clamp(0., 1.);
        // Move controls below the identity before widening their row.
        let stacked_lift = (stacked / 0.65).min(1.);
        let stacked_width = ((stacked - 0.6) / 0.4).clamp(0., 1.);
        let expanded_extent = (width * 0.22).clamp(144., 224.);
        let expanded_height = (expanded_extent + 48.).max(
            if self.page.description.is_empty() {
                272.
            } else {
                336.
            } + 32. * narrow,
        );
        let compact_height = 96. + 32. * narrow + 40. * stacked_lift;
        let compact_top = (20. + 16. * narrow) * (1. - stacked_lift) + 16. * stacked_lift;
        // Short collections stay expanded. They cannot fill the compact viewport;
        // collapsing them would clamp the scroll offset and repeatedly reopen it.
        let phase = if self.visible.len() as f32 * 66. > f32::from(window.bounds().size.height) {
            (-self.scroll.offset() / 160.).clamp(0., 1.)
        } else {
            0.
        };
        let phase = phase * phase * (3. - 2. * phase);
        let blend = |from: f32, to: f32| from + (to - from) * phase;
        let extent = blend(expanded_extent, 56.);
        let text_left = blend(expanded_extent + 56., 104.);
        let expanded_actions = (expanded_extent + 56.) * (1. - stacked) + 32. * stacked;
        div()
            .id("collection-header")
            .relative()
            .w_full()
            .h(px(blend(expanded_height, compact_height)))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(background)
            .child(
                artwork_frame(cx)
                    .absolute()
                    .left_8()
                    .top(px(blend(24., compact_top)))
                    .size(px(extent))
                    .child(self.cover(
                        &hero,
                        rems((extent - 2.) / f32::from(window.rem_size())),
                        cx,
                    )),
            )
            .child(
                div()
                    .absolute()
                    .left(px(text_left))
                    .right(px(blend(
                        32.,
                        (width * 0.48 + 16.).max(416.) * (1. - stacked_width) + 32. * stacked_width,
                    )))
                    .top(px(blend(24., compact_top)))
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(blend(8., 4.)))
                    .child(
                        div()
                            .h(px(18. * (1. - phase)))
                            .overflow_hidden()
                            .opacity(1. - phase)
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child(if hero.kind == ItemKind::Album {
                                t("Album")
                            } else {
                                t("Playlist")
                            }),
                    )
                    .child(
                        div()
                            .font_family(raag::i18n::font())
                            .text_size(px(blend(34., 22.)))
                            .font_weight(FontWeight::BOLD)
                            .line_height(relative(1.12))
                            .truncate()
                            .child(title.to_owned()),
                    )
                    .when(!hero.artist.is_empty(), |info| {
                        info.child(
                            div()
                                .text_size(px(blend(20., 14.)))
                                .font_family(raag::i18n::font())
                                .child(self.artist_link(&hero, cx)),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .h(px((if self.page.description.is_empty() {
                                56.
                            } else {
                                152.
                            }) * (1. - phase)))
                            .overflow_hidden()
                            .opacity(1. - phase)
                            .when(!self.page.subtitle.is_empty(), |info| {
                                info.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.page.subtitle.clone()),
                                )
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(info),
                            )
                            .when(!self.page.description.is_empty(), |info| {
                                info.child(
                                    div().flex().flex_col().items_start().gap_1()
                                        .child(div().w_full().h(ui_px(42.)).flex_shrink_0().overflow_hidden()
                                            .text_sm().line_height(relative(1.5)).line_clamp(2).text_ellipsis()
                                            .child(self.page.description.split_whitespace().collect::<Vec<_>>().join(" ")))
                                        .child(Button::new("collection-description").ghost().small()
                                            .label(t("Show more…"))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                let title = this.page.title.clone();
                                                let description = this.page.description.clone();
                                                window.open_dialog(cx, move |dialog, window, _| {
                                                    dialog.title(title.clone()).width(ui_px(560.))
                                                        .child(div().id("collection-description-scroll")
                                                            .max_h(window.bounds().size.height * 0.55)
                                                            .overflow_y_scroll().text_sm().line_height(relative(1.6))
                                                            .child(description.clone()))
                                                });
                                            }))),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left(px(blend(expanded_actions, 32.)))
                    .right_8()
                    .top(px(blend(
                        expanded_height - 60. - 28. * narrow,
                        28. - 8. * narrow + 68. * stacked_lift,
                    )))
                    .flex()
                    .items_center()
                    .child(div().flex_basis(ui_px(0.)).flex_grow(phase).min_w_0())
                    .child(actions.gap_2().min_w_0().flex_shrink_1())
                    .child(div().flex_basis(ui_px(0.)).flex_grow(1. - phase).min_w_0()),
            )
            .into_any_element()
    }
    fn hero_visual(&self, track: &Track, cx: &App) -> AnyElement {
        self.request_missing_artwork(track);
        let sharp = self
            .artwork_path(track)
            .map(|path| self.artwork_variant(path, 2))
            .map(ImageSource::from)
            .unwrap_or_else(|| self.art_source(track));
        let soft = self
            .artwork_path(track)
            .map(|path| path.with_extension("soft.jpg"));
        let arrived = self.artwork_arrival(track);
        let phase = arrived
            .map(|at| (at.elapsed().as_secs_f32() / 0.26).min(1.))
            .unwrap_or(1.);
        let picture = div().absolute().size_full().overflow_hidden();
        if phase >= 1. || cx.reduce_motion() {
            return picture
                .child(crate::images::picture_top_keyed(
                    sharp,
                    self.images.clone(),
                    SharedString::from(format!("hero-image-{}", track.id)),
                ))
                .into_any_element();
        }
        let images = self.images.clone();
        let key = SharedString::from(format!("hero-reveal-image-{}", track.id));
        picture
            .with_animation(
                SharedString::from(format!("hero-reveal-{}", track.id)),
                Animation::new(Duration::from_millis(260)),
                move |picture, _| {
                    let phase = ease_out_quint()(
                        arrived
                            .map(|at| (at.elapsed().as_secs_f32() / 0.26).min(1.))
                            .unwrap_or(1.),
                    );
                    picture
                        .when_some(soft.clone(), |picture, path| {
                            picture.child(div_placeholder(path.into(), 1. - phase, ui_px(0.)))
                        })
                        .child(div().absolute().inset_0().opacity(phase).child(
                            crate::images::picture_top_keyed(
                                sharp.clone(),
                                images.clone(),
                                key.clone(),
                            ),
                        ))
                },
            )
            .into_any_element()
    }
    pub(super) fn collection_screen(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let artist = matches!(&self.screen, Screen::Collection(id, _, _) if (id.starts_with("UC") || id.starts_with("artist:")));
        if !artist {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .pb_6()
                .pt_4()
                .gap_5()
                .child(self.collection_hero(false, window, cx))
                .when(self.editing_playlist, |body| {
                    body.child(div().px_8().child(self.playlist_editor(cx)))
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .px_8()
                        .gap_4()
                        .child(self.collection_tools(cx))
                        .child(self.track_list(cx)),
                )
                .when_some(self.continuation.as_ref(), |div, _| {
                    div.child(self.pagination_status(cx))
                })
                .into_any_element();
        }
        let mut body = self
            .scroll
            .bind(div().id("artist-scroll"))
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .pb_8()
            .gap_6()
            .child(self.collection_hero(true, window, cx));
        if self.loading && self.visible.is_empty() {
            body = body.child(div().px_8().child(self.discovery_loading(cx)));
        }
        let columns = if self.layout_width(window) > 1150. {
            3
        } else if self.layout_width(window) > 720. {
            2
        } else {
            1
        };
        let songs: Vec<_> = self.visible.iter().filter(|t| t.playable()).collect();
        let mut top = div()
            .flex()
            .flex_col()
            .gap_3()
            .px_8()
            .flex_shrink_0()
            .child(self.section_heading(t("Top songs")));
        for chunk in songs.chunks(columns) {
            let mut row = div().flex().gap_5().w_full();
            for track in chunk {
                row = row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .border_b_1()
                        .border_color(cx.theme().border.opacity(0.45))
                        .child(self.song_tile(track, cx)),
                );
            }
            for _ in chunk.len()..columns {
                row = row.child(div().flex_1());
            }
            top = top.child(row);
        }
        body = body.child(top);
        for section in &self.page.sections {
            if section.tracks.iter().any(|t| !t.playable()) {
                body = body.child(div().px_8().child(self.shelf(
                    if section.title.is_empty() {
                        t("More to explore")
                    } else {
                        &section.title
                    },
                    &section.tracks,
                    window,
                    cx,
                )));
            }
        }
        if !self.page.description.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .flex_shrink_0()
                    .pt_3()
                    .mx_8()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(self.section_heading(&tr!("About {}", self.page.title)))
                    .child(
                        div()
                            .text_sm()
                            .line_height(relative(1.7))
                            .max_w(rems(50.))
                            .text_color(cx.theme().muted_foreground)
                            .child(self.page.description.clone()),
                    ),
            );
        }
        body.when_some(self.continuation.as_ref(), |div, _| {
            div.child(self.pagination_status(cx))
        })
        .into_any_element()
    }
}

// Artists use the wide detail banner; album/playlist covers keep their listing identity.
fn collection_art(
    seed: Option<&Track>,
    page: &raag::catalog::Page,
    title: &str,
    artist: bool,
) -> Track {
    let mut hero = seed.cloned().unwrap_or_else(|| {
        let mut hero = Track::song("page-hero", title, "");
        hero.kind = ItemKind::Playlist;
        hero
    });
    if (artist && page.artwork.is_some()) || hero.artwork.is_none() {
        hero.artwork.clone_from(&page.artwork);
    }
    if hero.artwork.is_none() {
        hero.artwork = page.tracks.iter().find_map(|track| track.artwork.clone());
    }
    if artist {
        hero.kind = ItemKind::Artist;
        if let Some(url) = &hero.artwork {
            hero.id = format!("page-hero:{url}");
        }
    }
    if hero.kind == ItemKind::Album && !page.artists.is_empty() {
        hero.artists.clone_from(&page.artists);
        hero.artist = page
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(" & ");
    }
    hero
}
