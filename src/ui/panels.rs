use super::*;

impl Raag {
    pub(super) fn toggle_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.panel = if self.panel == Some(panel) {
            None
        } else {
            Some(panel)
        };
        if let Some(panel) = self.panel {
            self.last_panel = Some(panel);
        }
        self.panel_scroll.reset();
        self.lyrics_geometry = None;
        if self.panel == Some(Panel::Lyrics) {
            self.load_lyrics();
        }
        if self.panel == Some(Panel::Recent) && self.signed_in() {
            self.account_read(
                crate::account_sync::Key::Section(AccountSection::History),
                false,
                cx,
            );
        }
        self.right_content.update(cx, |_, cx| cx.notify());
        cx.notify();
    }
    pub(super) fn right_panel(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(panel) = self.panel.or(self.last_panel) else {
            return div().into_any_element();
        };
        let title = match panel {
            Panel::Playing => "Now playing",
            Panel::Recent => "Recently played",
            Panel::Queue => "Queue",
            Panel::Lyrics => "Lyrics",
        };
        let mut tabs = div().flex().items_center().gap_2();
        for (id, name, glyph, mode) in [
            (
                "panel-playing",
                "Now playing",
                "now-playing",
                Panel::Playing,
            ),
            ("panel-recent", "Recently played", "clock", Panel::Recent),
            ("panel-queue", "Queue", "list", Panel::Queue),
            ("panel-lyrics", "Lyrics", "lyrics", Panel::Lyrics),
        ] {
            tabs = tabs.child(
                Button::new(id)
                    .ghost()
                    .icon(icon(glyph))
                    .tooltip(name)
                    .accessibility_label(name)
                    .selected(panel == mode)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.panel != Some(mode) {
                            this.toggle_panel(mode, cx);
                        }
                    })),
            );
        }
        let header = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .px_5()
            .pt_5()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_lg()
                            .font_family("Manrope")
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        Button::new("close-panel")
                            .ghost()
                            .icon(icon("xmark"))
                            .tooltip("Close panel")
                            .accessibility_label("Close panel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.panel = None;
                                this.content_revision += 1;
                                window.focus(&this.focus, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(tabs);
        let content = match panel {
            Panel::Playing => self.now_playing_panel(window, cx),
            Panel::Recent | Panel::Queue => self.panel_list(panel, cx),
            Panel::Lyrics => self.lyrics_panel(window, cx),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_5()
            .child(header)
            .child(content)
            .into_any_element()
    }
    fn now_playing_panel(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(track) = &self.selected else {
            return div()
                .p_5()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Choose a song to get started.")
                .into_any_element();
        };
        let quality = &self.stream_quality;
        let description = if self.buffering {
            "Finding the best available audio…".into()
        } else if self.failed {
            "Playback could not start. Try again.".into()
        } else if quality.bitrate > 0. {
            format!(
                "{} · {:.0} kb/s{}{}",
                quality.codec.to_uppercase(),
                quality.bitrate,
                if self.source_offline {
                    " · Offline"
                } else {
                    ""
                },
                if quality.source.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", quality.source)
                }
            )
        } else if self.source_offline {
            "Playing from this computer".into()
        } else {
            "Best available audio".into()
        };
        self.panel_scroll
            .bind(div().id("now-playing-scroll"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_5()
            .pb_5()
            .gap_5()
            .child(
                artwork_frame(cx)
                    .w_full()
                    .aspect_ratio(1.)
                    .flex_shrink_0()
                    .rounded_2xl()
                    .child(self.playing_cover(
                        track,
                        rems(
                            self.right_sidebar_rems(window)
                                - 2.5
                                - 2. / f32::from(window.rem_size()),
                        ),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(self.playing_identity(track, cx))
                    .child(self.like_button(track, "panel-like", cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(description),
            )
            .when_some(self.cache_notice.as_ref(), |container, message| {
                container.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Offline cache: {message}")),
                )
            })
            .child(self.basic_controls("panel", cx))
            .child(
                Button::new("panel-open-lyrics")
                    .ghost()
                    .label("Open lyrics")
                    .icon(icon("lyrics"))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(Panel::Lyrics, cx))),
            )
            .into_any_element()
    }
    fn panel_list(&self, panel: Panel, cx: &mut Context<Self>) -> AnyElement {
        // Queue storage already owns the songs. Rebuilding an entire snapshot
        // on every animated bounds change defeats the virtual list.
        let tracks = (panel != Panel::Queue).then(|| Arc::new(self.recent_tracks()));
        let count = tracks
            .as_ref()
            .map_or_else(|| self.queue.ordered_len(), |tracks| tracks.len());
        if count == 0 {
            return div()
                .p_5()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(if panel == Panel::Queue {
                    "Your queue is empty. Play a song to begin."
                } else {
                    "Your recently played songs will appear here."
                })
                .into_any_element();
        }
        let owner = cx.weak_entity();
        self.panel_scroll
            .bind_list(uniform_list(
                "right-panel-tracks",
                count,
                move |range, _, cx| {
                    owner
                        .update(cx, |this, cx| {
                            range
                                .filter_map(|ix| {
                                    let track = if let Some(tracks) = &tracks {
                                        tracks.get(ix)
                                    } else {
                                        this.queue.track_at(ix)
                                    }?;
                                    Some(this.panel_row(track, ix, panel, tracks.clone(), cx))
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                },
            ))
            .size_full()
            .px_3()
            .pb_4()
            .into_any_element()
    }
    fn panel_row(
        &self,
        track: &Track,
        ix: usize,
        panel: Panel,
        tracks: Option<Arc<Vec<Track>>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owned = track.clone();
        let group: SharedString = format!("panel-row-{}-{ix}", track.id).into();
        let current = if panel == Panel::Queue {
            self.queue.cursor() == Some(ix)
        } else {
            self.selected
                .as_ref()
                .is_some_and(|song| song.id == track.id)
        };
        let mut actions = div()
            .w_16()
            .justify_end()
            .flex()
            .gap_1()
            .flex_shrink_0()
            .opacity(0.65)
            .group_hover(group.clone(), |div| div.opacity(1.))
            .child(self.like_button(
                track,
                SharedString::from(format!("panel-like-{}-{ix}", track.id)),
                cx,
            ));
        if panel == Panel::Queue {
            actions = actions.child(
                Button::new(SharedString::from(format!(
                    "panel-remove-{}-{ix}",
                    track.id
                )))
                .ghost()
                .small()
                .icon(icon("xmark"))
                .disabled(current)
                .tooltip("Remove from queue")
                .accessibility_label("Remove from queue")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.queue.remove_at(ix);
                    this.right_content.update(cx, |_, cx| cx.notify());
                    cx.notify();
                })),
            );
        } else {
            let queued = track.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!(
                    "panel-enqueue-{}-{ix}",
                    track.id
                )))
                .ghost()
                .small()
                .icon(icon("plus"))
                .tooltip("Add to queue")
                .accessibility_label("Add to queue")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.queue.enqueue(queued.clone(), false);
                    this.right_content.update(cx, |_, cx| cx.notify());
                    cx.notify();
                })),
            );
        }
        let row = div()
            .group(group)
            .flex()
            .items_center()
            .h_full()
            .w_full()
            .min_w_0()
            .gap_2()
            .px_2()
            .rounded_lg()
            .when(current, |div| div.bg(cx.theme().muted))
            .hover(|div| div.bg(cx.theme().muted))
            .child(
                content_button(SharedString::from(format!("panel-play-{}-{ix}", track.id)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .justify_start()
                    .overflow_hidden()
                    .accessibility_label(format!("Play {}", track.title))
                    .child(self.cover(track, rems(2.5), cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(track.title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.artist_link(track, cx)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if panel == Panel::Queue {
                            this.queue.select_at(ix);
                        } else if let Some(tracks) = &tracks {
                            this.queue.replace(tracks.as_ref().clone(), &owned.id);
                            this.invalidate_collection();
                        }
                        this.start(owned.clone(), cx);
                    })),
            )
            .child(actions);
        div()
            .h(rems(4.5))
            .w_full()
            .py_1()
            .child(row)
            .into_any_element()
    }
    pub(super) fn follow_lyrics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self
            .lyrics
            .iter()
            .rposition(|line| line.seconds.is_some_and(|time| time <= self.position));
        let changed = active != self.lyrics_active;
        self.lyrics_active = active;
        let scroll = if self.immersive {
            &self.immersive_scroll
        } else {
            &self.lyrics_scroll
        };
        let geometry = (scroll.viewport_height(), scroll.max_offset());
        // A newly mounted list has no layout on the first playback tick. Retry
        // after layout, including when switching between sidebar and fullscreen.
        if self.lyrics_follow
            && (changed || self.lyrics_geometry != Some(geometry))
            && geometry.0 > 0.
            && geometry.1 > 0.
            && let Some(index) = active
        {
            self.lyrics_geometry = Some(geometry);
            let target = (index as f32 + 2.) * f32::from(rems(7.).to_pixels(window.rem_size()))
                + f32::from(rems(3.5).to_pixels(window.rem_size()))
                - geometry.0 / 2.;
            let view = if self.immersive {
                self.chrome[ChromePart::ImmersiveLyrics as usize].entity_id()
            } else {
                self.right_content.entity_id()
            };
            scroll.scroll_to(-target.max(0.), view, window, cx);
        }
        if changed {
            if self.immersive {
                cx.notify();
            } else if self.screen == Screen::Lyrics {
                self.content_revision += 1;
                cx.notify();
            } else {
                self.right_content.update(cx, |_, cx| cx.notify());
            }
        }
    }
    fn lyrics_panel(&self, _: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(track) = &self.selected else {
            return div()
                .p_5()
                .text_sm()
                .child("Play a song to see its lyrics.")
                .into_any_element();
        };
        let mut body = div().flex().flex_col().size_full().min_h_0().gap_4().child(
            div()
                .px_5()
                .flex()
                .items_center()
                .gap_3()
                .child(self.cover(track, rems(2.5), cx))
                .child(self.playing_identity(track, cx)),
        );
        if self.lyrics_loading {
            return body
                .child(
                    div()
                        .p_5()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Finding lyrics…"),
                )
                .into_any_element();
        }
        if let Some(error) = &self.lyrics_error {
            return body
                .child(
                    div()
                        .p_5()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(error.clone()),
                )
                .into_any_element();
        }
        if self.lyrics.is_empty() {
            return body
                .child(
                    div()
                        .p_5()
                        .text_sm()
                        .child("No lyrics are available for this song."),
                )
                .into_any_element();
        }
        let synced = self.lyrics.iter().any(|line| line.seconds.is_some());
        let owner = cx.weak_entity();
        let manual = owner.clone();
        let count = self.lyrics.len() + 4;
        let controller = self.lyrics_scroll.on_user_scroll(move |_, cx| {
            let _ = manual.update(cx, |this, cx| {
                this.lyrics_follow = false;
                this.right_content.update(cx, |_, cx| cx.notify());
            });
        });
        let list = uniform_list("animated-lyrics", count, move |range, _, cx| {
            owner
                .update(cx, |this, cx| {
                    range
                        .map(|row| {
                            if row < 2 || row >= this.lyrics.len() + 2 {
                                return div().h(rems(7.)).into_any_element();
                            }
                            let index = row - 2;
                            let line = &this.lyrics[index];
                            let seconds = line.seconds;
                            let active = this.lyrics_active == Some(index);
                            let past = this.lyrics_active.is_some_and(|current| index < current);
                            let label = div()
                                .w_full()
                                .text_center()
                                .text_size(rems(1.65))
                                .line_height(relative(1.25))
                                .font_weight(FontWeight::BOLD)
                                .text_color(cx.theme().foreground)
                                .opacity(if !synced || active {
                                    1.
                                } else if past {
                                    0.62
                                } else {
                                    0.28
                                })
                                .child(line.text.clone());
                            let label = if active && !cx.reduce_motion() {
                                label
                                    .with_animation(
                                        ("lyric-focus", index),
                                        Animation::new(Duration::from_millis(260))
                                            .with_easing(ease_out_quint()),
                                        |div, phase| div.opacity(0.55 + phase * 0.45),
                                    )
                                    .into_any_element()
                            } else {
                                label.into_any_element()
                            };
                            content_button(("lyric-line", index))
                                .w_full()
                                .h(rems(7.))
                                .px_5()
                                .flex()
                                .items_center()
                                .justify_center()
                                .overflow_hidden()
                                .disabled(seconds.is_none())
                                .accessibility_label(format!("Seek to {}", line.text))
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(time) = seconds {
                                        this.commit_seek(time, cx);
                                    }
                                }))
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        body =
            body.child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(controller.bind_list(list).size_full())
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h_12()
                            .bg(linear_gradient(
                                180.,
                                linear_color_stop(cx.theme().sidebar, 0.),
                                linear_color_stop(cx.theme().sidebar.opacity(0.), 1.),
                            )),
                    )
                    .child(div().absolute().bottom_0().left_0().right_0().h_12().bg(
                        linear_gradient(
                            0.,
                            linear_color_stop(cx.theme().sidebar, 0.),
                            linear_color_stop(cx.theme().sidebar.opacity(0.), 1.),
                        ),
                    )),
            );
        body.child(div().px_5().pb_4().flex_shrink_0().child(if synced {
            Button::new("follow-lyrics")
                .ghost()
                .small()
                .label(if self.lyrics_follow {
                    "Following song"
                } else {
                    "Resume following"
                })
                .selected(self.lyrics_follow)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.lyrics_follow = true;
                    this.lyrics_active = None;
                    this.lyrics_geometry = None;
                    this.right_content.update(cx, |_, cx| cx.notify());
                    cx.notify();
                }))
                .into_any_element()
        } else {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Untimed lyrics")
                .into_any_element()
        }))
        .into_any_element()
    }
}
