use super::*;
use crate::account_sync::Key;
use gpui_kit::component::tooltip::Tooltip;
use raag::library::PlaylistView;

#[derive(Clone)]
pub(super) struct CollectionEntry {
    pub(super) track: Track,
    pub(super) local: bool,
    pub(super) detail: String,
}

impl Raag {
    pub(super) fn playlist_entries(&self, limit: usize) -> Vec<CollectionEntry> {
        let remote = self.account.page(&Key::Section(AccountSection::Playlists));
        remote
            .into_iter()
            .flat_map(|page| &page.tracks)
            .take(limit)
            .map(|track| CollectionEntry {
                track: track.clone(),
                local: false,
                detail: if track.artist.is_empty() {
                    "YouTube Music".into()
                } else {
                    track.artist.clone()
                },
            })
            .chain(self.library.playlists.iter().take(limit).map(|playlist| {
                let mut track = Track::song(&playlist.id, &playlist.name, "");
                track.kind = ItemKind::Playlist;
                track.artwork = playlist.tracks.iter().find_map(|song| song.artwork.clone());
                CollectionEntry {
                    track,
                    local: true,
                    detail: format!(
                        "{} {} · Local playlist",
                        playlist.tracks.len(),
                        if playlist.tracks.len() == 1 {
                            "song"
                        } else {
                            "songs"
                        }
                    ),
                }
            }))
            .collect()
    }

    fn open_collection_entry(&mut self, entry: &CollectionEntry, cx: &mut Context<Self>) {
        if entry.local {
            self.navigate(
                Screen::Collection(entry.track.id.clone(), entry.track.title.clone(), true),
                cx,
            );
        } else if entry.track.kind == ItemKind::Playlist {
            self.open_account_playlist(entry.track.clone(), cx);
        } else {
            self.open_track(entry.track.clone(), cx);
        }
    }

    fn begin_playlist(&mut self, remote: bool, window: &mut Window, cx: &mut Context<Self>) {
        let changing_section = self.library_section != AccountSection::Playlists;
        self.library_section = AccountSection::Playlists;
        self.navigate(Screen::Library, cx);
        if changing_section && self.signed_in() {
            self.account_read(Key::Section(AccountSection::Playlists), false, cx);
        }
        self.playlist_editor_remote = remote;
        self.editing_playlist = true;
        self.editing_description = false;
        self.content_revision += 1;
        self.playlist_name.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn sidebar_playlists(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.library.playlist_view;
        let (image, label) = match mode {
            PlaylistView::List => ("view-list", "List → compact grid"),
            PlaylistView::CompactGrid => ("grid-small", "Compact grid → artwork grid"),
            PlaylistView::ArtworkGrid => ("grid-large", "Artwork grid → list"),
        };
        let mut body = div()
            .id("sidebar-playlists")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_2()
            .mt_6()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl_2()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Playlists"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("playlist-view")
                                    .ghost()
                                    .small()
                                    .icon(icon(image))
                                    .tooltip(label)
                                    .accessibility_label(format!("Playlist view: {label}"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.library.playlist_view =
                                            this.library.playlist_view.next();
                                        this.navigation_revision =
                                            this.navigation_revision.wrapping_add(1);
                                        this.sidebar_scroll.reset();
                                        this.save();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("new-playlist-side")
                                    .ghost()
                                    .small()
                                    .icon(icon("plus"))
                                    .tooltip("Create playlist")
                                    .accessibility_label("Create playlist")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.begin_playlist(this.signed_in(), window, cx);
                                    })),
                            ),
                    ),
            );
        let entries = self.playlist_entries(80);
        if entries.is_empty() {
            body = body.child(
                div()
                    .px_2()
                    .py_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Make room for your favourites."),
            );
        }
        let columns = match mode {
            PlaylistView::List => 1,
            PlaylistView::CompactGrid => 3,
            PlaylistView::ArtworkGrid => 2,
        };
        for chunk in entries.chunks(columns) {
            let mut row = div().flex().items_start().gap_2().flex_shrink_0();
            for entry in chunk {
                let selected = matches!(&self.screen, Screen::Collection(id, _, local)
                    if id == &entry.track.id && local == &entry.local);
                let extent = if mode == PlaylistView::List {
                    36.
                } else {
                    // Leave room for both the padding and focus border inside each cell.
                    (224. - 8. * (columns - 1) as f32) / columns as f32 - 10.
                };
                let owned = entry.clone();
                let title = entry.track.title.clone();
                let mut button = content_button(SharedString::from(format!(
                    "side-playlist-{}-{}",
                    entry.local, entry.track.id
                )))
                .flex()
                .gap_2()
                .p_1()
                .rounded_lg()
                .flex_shrink_0()
                .border_1()
                .border_color(cx.theme().border.opacity(0.))
                .cursor_pointer()
                .accessibility_label(format!("Open playlist {title}"))
                .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
                .hover(|button| button.bg(cx.theme().muted))
                .focus_visible(|button| button.border_color(cx.theme().primary))
                .when(selected, |button| button.bg(cx.theme().muted))
                .child(self.cover(
                    &entry.track,
                    rems(extent / f32::from(cx.theme().font_size)),
                    cx,
                ))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.open_collection_entry(&owned, cx)),
                );
                if mode == PlaylistView::List {
                    button = button
                        .w_full()
                        .h(px(48.))
                        .items_center()
                        .justify_start()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .truncate()
                                .child(entry.track.title.clone()),
                        );
                } else {
                    button = button.w(px(extent + 10.)).flex_col().items_start().when(
                        mode == PlaylistView::CompactGrid,
                        |button| {
                            button.child(
                                div()
                                    .w_full()
                                    .text_xs()
                                    .truncate()
                                    .child(entry.track.title.clone()),
                            )
                        },
                    );
                }
                row = row.child(button);
            }
            body = body.child(row);
        }
        body.into_any_element()
    }

    fn library_card(
        &self,
        entry: &CollectionEntry,
        extent: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owned = entry.clone();
        let artist = entry.track.kind == ItemKind::Artist;
        content_button(SharedString::from(format!(
            "library-card-{}-{}",
            entry.local, entry.track.id
        )))
        .w(px(extent))
        .h(px(extent + 64.))
        .p_2()
        .overflow_hidden()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_start()
        .justify_start()
        .gap_2()
        .rounded_lg()
        .cursor_pointer()
        .hover(|button| button.bg(cx.theme().muted))
        .focus_visible(|button| button.bg(cx.theme().muted).text_color(cx.theme().primary))
        .accessibility_label(format!("Open {}", entry.track.title))
        .child(
            div()
                .size(px(extent - 16.))
                .flex_shrink_0()
                .overflow_hidden()
                .rounded_lg()
                .when(artist, |image| image.rounded_full())
                .child(self.cover(
                    &entry.track,
                    rems((extent - 16.) / f32::from(cx.theme().font_size)),
                    cx,
                )),
        )
        .child(
            div()
                .w_full()
                .flex_shrink_0()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(entry.track.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(entry.detail.clone()),
                ),
        )
        .on_click(cx.listener(move |this, _, _, cx| this.open_collection_entry(&owned, cx)))
        .into_any_element()
    }

    pub(super) fn library_screen(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let signed_in = self.signed_in();
        let section = if signed_in {
            self.library_section
        } else {
            AccountSection::Playlists
        };
        let key = Key::Section(section);
        let loading = signed_in && self.account.loading(&key);
        let owner = cx.weak_entity();
        let mut body = div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .px_8()
            .pb_6()
            .gap_5()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .flex_shrink_0()
                    .child(self.page_heading(
                        "Your library",
                        "The music you keep coming back to.",
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("library-create")
                                    .primary()
                                    .icon(icon("plus"))
                                    .label("New playlist")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.begin_playlist(this.signed_in(), window, cx)
                                    })),
                            )
                            .child(
                                Button::new("library-actions")
                                    .ghost()
                                    .icon(icon("more"))
                                    .tooltip("Library actions")
                                    .accessibility_label("Library actions")
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for (label, command) in [
                                            ("Import playlist…", 0),
                                            ("New local playlist…", 1),
                                            ("Local files", 2),
                                            ("Liked songs", 3),
                                        ] {
                                            let owner = owner.clone();
                                            menu =
                                                menu.item(PopupMenuItem::new(label).on_click(
                                                    move |_, window, cx| {
                                                        let _ = owner.update(cx, |this, cx| {
                                                            match command {
                                                                0 => this.import_playlist(cx),
                                                                1 => this.begin_playlist(
                                                                    false, window, cx,
                                                                ),
                                                                2 => {
                                                                    this.navigate(Screen::Local, cx)
                                                                }
                                                                _ => {
                                                                    this.navigate(Screen::Liked, cx)
                                                                }
                                                            }
                                                        });
                                                    },
                                                ));
                                        }
                                        menu
                                    }),
                            ),
                    ),
            );
        if self.editing_playlist {
            body = body.child(self.playlist_editor(cx));
        }
        if signed_in {
            let mut tabs = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .flex_1()
                .min_w_0();
            for tab in AccountSection::ALL {
                tabs = tabs.child(
                    Button::new(SharedString::from(format!("library-tab-{tab:?}")))
                        .ghost()
                        .label(tab.title())
                        .selected(section == tab)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.library_section != tab {
                                this.library_section = tab;
                                this.pagination_attempts.clear();
                                this.scroll.reset();
                                this.account_read(Key::Section(tab), false, cx);
                            }
                        })),
                );
            }
            body = body.child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .pb_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .flex_shrink_0()
                    .child(tabs),
            );
        }
        let gallery = matches!(
            section,
            AccountSection::Playlists | AccountSection::Albums | AccountSection::Artists
        );
        body = body.child(self.collection_tools(cx));
        let mut entries = if section == AccountSection::Playlists {
            self.playlist_entries(usize::MAX)
        } else if gallery {
            self.account_tracks(section)
                .into_iter()
                .map(|track| {
                    let detail = if track.kind == ItemKind::Artist {
                        "Artist".into()
                    } else {
                        track.artist.clone()
                    };
                    CollectionEntry {
                        track,
                        local: false,
                        detail,
                    }
                })
                .collect()
        } else {
            vec![]
        };
        if gallery {
            entries.retain(|entry| {
                format!("{} {}", entry.track.title, entry.detail)
                    .to_lowercase()
                    .contains(&self.browse_filter)
            });
            if self.browse_sort > 0 {
                entries.sort_by_cached_key(|entry| {
                    if self.browse_sort == 2 {
                        entry.detail.to_lowercase()
                    } else {
                        entry.track.title.to_lowercase()
                    }
                });
            }
        }
        let count = if gallery {
            entries.len()
        } else {
            self.visible.len()
        };
        body = body.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .flex_shrink_0()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_3()
                        .child(
                            div()
                                .text_lg()
                                .font_family("Manrope")
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(section.title()),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{count} {}",
                                    if count == 1 { "item" } else { "items" }
                                )),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if loading {
                            "Updating…"
                        } else if signed_in {
                            "YouTube Music"
                        } else {
                            "On this computer"
                        }),
                ),
        );
        if count == 0 {
            body = body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(
                        icon("library")
                            .size_8()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_family("Manrope")
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if loading {
                                "Finding your music…"
                            } else {
                                "A little room for your favourites"
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .text_center()
                            .child(if loading {
                                "Your collection will appear here."
                            } else if section == AccountSection::Playlists {
                                "Create a playlist or import one from the menu above."
                            } else {
                                "Save music in YouTube Music, then sync from your profile menu."
                            }),
                    ),
            );
        } else if gallery {
            let (columns, extent) = self.gallery_geometry(window);
            let entries = Arc::new(entries);
            let owner = cx.weak_entity();
            body = body.child(
                self.scroll
                    .bind_list(uniform_list(
                        SharedString::from(format!("library-gallery-{section:?}")),
                        entries.len().div_ceil(columns),
                        move |range, _, cx| {
                            owner
                                .update(cx, |this, cx| {
                                    range
                                        .map(|index| {
                                            let mut row = div()
                                                .h(px(extent + 80.))
                                                .flex()
                                                .gap_5()
                                                .items_start();
                                            for entry in
                                                entries.iter().skip(index * columns).take(columns)
                                            {
                                                row =
                                                    row.child(this.library_card(entry, extent, cx));
                                            }
                                            row
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                        },
                    ))
                    .flex_1()
                    .min_h_0(),
            );
        } else {
            body = body.child(self.track_list(cx));
        }
        if signed_in
            && self
                .account
                .page(&key)
                .is_some_and(|page| page.continuation.is_some())
        {
            body = body.child(self.pagination_status(cx));
        }
        body.into_any_element()
    }
}
