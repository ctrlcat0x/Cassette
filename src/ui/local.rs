use super::*;

fn track_count(count: usize) -> String {
    if count == 1 {
        t("1 track").to_owned()
    } else {
        tr!("{count} tracks")
    }
}

fn album_title(album: &LocalAlbum) -> &str {
    if album.title.is_empty() {
        t("Untitled album")
    } else {
        &album.title
    }
}

fn album_cover(album: &LocalAlbum, extent: Pixels, cx: &App) -> Div {
    div()
        .size(extent)
        .flex_shrink_0()
        .rounded_lg()
        .overflow_hidden()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().muted)
        .child(
            icon("music-double-note")
                .size(extent / 4.)
                .text_color(cx.theme().muted_foreground),
        )
        .when_some(album.artwork.as_ref(), |cover, path| {
            cover.child(
                img(PathBuf::from(path))
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
        })
}

impl Raag {
    pub(super) fn local_screen(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .px_8()
            .pb_6()
            .gap_4();
        if let Screen::LocalAlbum(key, _) = &self.screen {
            let album = self.local_albums.iter().find(|album| &album.key == key);
            let detail = album.map_or_else(
                || track_count(0),
                |album| {
                    let count = track_count(album.track_count);
                    if album.artist.is_empty() || album.artist == "Local file" {
                        count
                    } else {
                        format!("{} · {count}", album.artist)
                    }
                },
            );
            let key = key.clone();
            return body
                .pt_7()
                .child(
                    div().flex().child(
                        Button::new("back-to-local-albums")
                            .ghost()
                            .icon(icon("nav-arrow-left"))
                            .label(t("Albums"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.back.last() == Some(&Screen::Local) {
                                    this.history_navigation(false, window, cx);
                                } else {
                                    this.navigate(Screen::Local, cx);
                                }
                            })),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_5()
                        .flex_shrink_0()
                        .when_some(album, |row, album| {
                            row.child(album_cover(album, ui_px(112.), cx))
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_start()
                                .min_w_0()
                                .gap_3()
                                .child(self.page_heading(
                                    self.screen.title().to_owned(),
                                    detail,
                                    cx,
                                ))
                                .child(
                                    Button::new("play-local-album")
                                        .primary()
                                        .icon(icon("play"))
                                        .label(t("Play album"))
                                        .disabled(album.is_none())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let tracks = this.library.local_album_tracks(&key);
                                            if let Some(first) = tracks.first().cloned() {
                                                this.play_tracks(first, tracks, cx);
                                            }
                                        })),
                                ),
                        ),
                )
                .when(album.is_some(), |body| {
                    body.child(self.collection_tools(cx))
                })
                .child(self.track_list(cx))
                .into_any_element();
        }

        body = body
            .child(self.title(t("Music from folders and files on this computer."), cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("local-albums")
                                    .ghost()
                                    .label(t("Albums"))
                                    .selected(self.screen == Screen::Local)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.navigate(Screen::Local, cx)
                                    })),
                            )
                            .child(
                                Button::new("local-songs")
                                    .ghost()
                                    .label(t("Songs"))
                                    .selected(self.screen == Screen::LocalSongs)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.navigate(Screen::LocalSongs, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("local-add-folder")
                                    .primary()
                                    .icon(icon("folder"))
                                    .label(t("Add folder…"))
                                    .disabled(self.local_scan.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| this.import(true, cx))),
                            )
                            .child(
                                Button::new("local-add-files")
                                    .ghost()
                                    .label(t("Add files…"))
                                    .disabled(self.local_scan.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| this.import(false, cx))),
                            )
                            .when(!self.library.local_folders.is_empty(), |actions| {
                                actions.child(
                                    Button::new("rescan-local")
                                        .ghost()
                                        .icon(icon("refresh"))
                                        .tooltip(t("Rescan folders"))
                                        .accessibility_label(t("Rescan folders"))
                                        .disabled(self.local_scan.is_some())
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.rescan_local(cx)),
                                        ),
                                )
                            }),
                    ),
            )
            .when(self.local_scan.is_some(), |body| {
                body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(loading_indicator(cx))
                        .child(t("Scanning local files…")),
                )
            });
        if self.screen == Screen::LocalSongs {
            return body
                .when(!self.library.local.is_empty(), |body| {
                    body.child(
                        self.collection_tools(cx).child(
                            Button::new("play-local-songs")
                                .primary()
                                .icon(icon("play"))
                                .label(t("Play"))
                                .disabled(self.visible.is_empty())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(track) = this.visible.first().cloned() {
                                        this.open_track(track, cx);
                                    }
                                })),
                        ),
                    )
                })
                .child(self.track_list(cx))
                .into_any_element();
        }
        if self.local_albums.is_empty() {
            return body
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .text_center()
                        .child(
                            icon("library")
                                .size_10()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(
                            if self.local_scan.is_some() {
                                t("Finding your music…")
                            } else {
                                t("No local albums yet")
                            },
                        ))
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(t("Add a music folder to browse your albums here.")),
                        ),
                )
                .into_any_element();
        }

        let scale = raag::i18n::scale();
        let gap = 16. * scale;
        let width = (self.content_width(window) - 64. * scale).max(1.);
        let columns = ((width + gap) / (156. * scale + gap)).floor().clamp(1., 8.) as usize;
        let extent = px(((width - gap * (columns - 1) as f32) / columns as f32).min(244. * scale));
        let albums = self.local_albums.clone();
        let owner = cx.entity();
        body.child(
            self.scroll
                .bind_list(uniform_list(
                    "local-album-grid",
                    albums.len().div_ceil(columns),
                    move |range, _, cx| {
                        owner.update(cx, |_, cx| {
                            range
                                .map(|row| {
                                    div().h(extent + ui_px(80.)).flex().gap(px(gap)).children(
                                        albums
                                            .iter()
                                            .skip(row * columns)
                                            .take(columns)
                                            .map(|album| Self::local_album_card(album, extent, cx)),
                                    )
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

    fn local_album_card(album: &LocalAlbum, extent: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let title = album_title(album).to_owned();
        let key = album.key.clone();
        let detail = if album.artist.is_empty() || album.artist == "Local file" {
            track_count(album.track_count)
        } else {
            album.artist.clone()
        };
        let id = SharedString::from(format!("local-album-{key:?}"));
        content_button(id)
            .w(extent)
            .h(extent + ui_px(64.))
            .p_2()
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
            .accessibility_label(tr!("Open {}", title))
            .child(album_cover(album, (extent - ui_px(16.)).max(px(1.)), cx))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.browse_filter.clear();
                this.browse_query
                    .update(cx, |input, cx| input.set_value("", window, cx));
                this.navigate(Screen::LocalAlbum(key.clone(), title.clone()), cx);
            }))
            .into_any_element()
    }
}
