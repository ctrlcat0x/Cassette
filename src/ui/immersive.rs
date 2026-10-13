use super::*;
use gpui_kit::component::menu::DropdownMenu;

impl Raag {
    pub(super) fn immersive_palette(&self) -> (u32, u32) {
        let color = |track: &Track| {
            self.artwork_colors
                .get(track.artwork_key())
                .or_else(|| self.artwork_colors.get(&track.id))
                .map(|colors| colors.ambient)
        };
        let ambient = self.selected.as_ref().and_then(color).unwrap_or(0x23232b);
        let previous = self
            .previous_track
            .as_ref()
            .and_then(color)
            .unwrap_or(ambient);
        (previous, ambient)
    }
    pub(super) fn immersive_background(&self, cx: &mut Context<Self>) -> AnyElement {
        let (previous, ambient) = self.immersive_palette();
        let soft = self
            .selected
            .as_ref()
            .and_then(|track| self.artwork_path(track))
            .map(|path| path.with_extension("soft.jpg"));
        div()
            .relative()
            .size_full()
            .bg(rgb(ambient))
            .when_some(
                soft.filter(|_| self.library.immersive_artwork),
                |background, path| {
                    background.child(
                        div()
                            .absolute()
                            .inset_0()
                            .opacity(0.65)
                            .child(img(path).size_full().object_fit(ObjectFit::Cover)),
                    )
                },
            )
            .child(div().absolute().inset_0().bg(rgb(0x08090d).opacity(0.40)))
            .with_animation(
                ("immersive-colour", self.player.generation() as usize),
                Animation::new(Duration::from_millis(if cx.reduce_motion() {
                    1
                } else {
                    450
                })),
                move |div, phase| div.bg(rgb(raag::artwork::blend(previous, ambient, phase))),
            )
            .into_any_element()
    }
    fn immersive_extent(&self, window: &Window) -> f32 {
        let width = f32::from(window.bounds().size.width);
        let height = f32::from(window.bounds().size.height);
        (height * 0.43)
            .min((height - 260.).max(240.))
            .min(width * if self.immersive_lyrics { 0.30 } else { 0.35 })
            .clamp(240., 440.)
    }
    pub(super) fn immersive_cover(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(track) = &self.selected else {
            return div().into_any_element();
        };
        let extent = self.immersive_extent(window);
        artwork_frame(cx)
            .size_full()
            .rounded_2xl()
            .child(self.playing_cover(
                track,
                rems((extent - 2.) / f32::from(window.rem_size())),
                cx,
            ))
            .into_any_element()
    }
    pub(super) fn toggle_immersive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.immersive {
            self.exit_immersive(window, cx);
            return;
        }
        if self.selected.is_none() {
            return;
        }
        self.immersive = true;
        self.immersive_lyrics = true;
        self.lyrics_follow = true;
        self.lyrics_active = None;
        self.lyrics_geometry = None;
        self.immersive_scroll.reset();
        self.load_lyrics();
        if !window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }
    pub(super) fn exit_immersive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.immersive = false;
        self.immersive_scroll.reset();
        if window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.content_revision += 1;
        window.focus(&self.focus, cx);
        cx.notify();
    }
    pub(super) fn immersive_player(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.selected.is_none() {
            return div().into_any_element();
        }
        let white: Hsla = rgb(0xffffff).into();
        let (_, ambient) = self.immersive_palette();
        let width = f32::from(window.bounds().size.width);
        let extent = self.immersive_extent(window);
        let left = div()
            .id("immersive-art-and-controls")
            .flex()
            .flex_col()
            .w(px(extent))
            .flex_shrink_0()
            .gap_4()
            .child(self.chrome_view(
                ChromePart::ImmersiveArt,
                StyleRefinement::default().size(px(extent)),
            ))
            .when(self.immersive_lyrics, |left| {
                left.child(self.chrome_view(
                    ChromePart::ImmersiveTitle,
                    StyleRefinement::default().w_full().h(rems(5.5)),
                ))
                .child(self.progress(cx))
                .child(self.chrome_view(
                    ChromePart::ImmersiveButtons,
                    StyleRefinement::default().w_full().h(ui_px(40.)),
                ))
            });
        let content = if self.immersive_lyrics {
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap(px((width * 0.065).clamp(40., 110.)))
                .h_full()
                .px_10()
                .child(left)
                .child(
                    div()
                        .w(px((width * 0.40).min(620.)))
                        .h_full()
                        .min_h_0()
                        .child(self.chrome_view(
                            ChromePart::ImmersiveLyrics,
                            StyleRefinement::default().size_full(),
                        )),
                )
        } else {
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap_12()
                .h_full()
                .px_10()
                .child(left)
                .child(
                    div()
                        .id("immersive-details")
                        .flex()
                        .flex_col()
                        .gap_6()
                        .w(px((width * 0.35).min(520.)))
                        .child(self.chrome_view(
                            ChromePart::ImmersiveTitle,
                            StyleRefinement::default().w_full().h(rems(5.5)),
                        ))
                        .child(self.progress(cx))
                        .child(self.chrome_view(
                            ChromePart::ImmersiveButtons,
                            StyleRefinement::default().w_full().h(ui_px(40.)),
                        )),
                )
        };
        let show_lyrics = self.immersive_lyrics;
        let show_artwork = self.library.immersive_artwork;
        let menu = Button::new("immersive-menu")
            .ghost()
            .icon(icon("nav-arrow-down"))
            .text_color(white.opacity(0.75))
            .tooltip(t("Player options"))
            .accessibility_label(t("Fullscreen player options"))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                menu.menu_with_check(t("Lyrics"), show_lyrics, Box::new(ToggleImmersiveLyrics))
                    .menu_with_check(
                        t("Blurred artwork background"),
                        show_artwork,
                        Box::new(ToggleImmersiveArtwork),
                    )
                    .separator()
                    .menu(t("Exit fullscreen"), Box::new(ExitImmersive))
            });
        div()
            .id("immersive-player")
            .key_context("Raag")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(rgb(ambient))
            .text_color(white)
            .line_height(relative(1.4))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| {
                this.set_ui_scale(this.library.ui_scale + 0.1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, window, cx| {
                this.set_ui_scale(this.library.ui_scale - 0.1, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ResetZoom, window, cx| this.set_ui_scale(1., window, cx)),
            )
            .on_action(cx.listener(|this, _: &TogglePlayback, _, cx| this.toggle_playback(cx)))
            .on_action(cx.listener(|this, _: &NextTrack, _, cx| this.next(false, cx)))
            .on_action(cx.listener(|this, _: &PreviousTrack, _, cx| this.previous(cx)))
            .on_action(
                cx.listener(|this, _: &Fullscreen, window, cx| this.exit_immersive(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ExitImmersive, window, cx| this.exit_immersive(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CloseOverlay, window, cx| this.exit_immersive(window, cx)),
            )
            .on_action(cx.listener(|_, _: &ShowLyrics, window, cx| {
                window.dispatch_action(Box::new(ToggleImmersiveLyrics), cx);
            }))
            .on_action(cx.listener(|this, _: &ShowQueue, window, cx| {
                this.exit_immersive(window, cx);
                this.toggle_panel(Panel::Queue, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.exit_immersive(window, cx);
                this.query.update(cx, |input, cx| input.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleImmersiveLyrics, _, cx| {
                this.immersive_lyrics = !this.immersive_lyrics;
                this.lyrics_active = None;
                this.lyrics_geometry = None;
                this.lyrics_follow = true;
                this.immersive_scroll.reset();
                if this.immersive_lyrics {
                    this.load_lyrics();
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleImmersiveArtwork, _, cx| {
                this.library.immersive_artwork = !this.library.immersive_artwork;
                this.save();
                cx.notify();
            }))
            .child(div().absolute().inset_0().child(self.chrome_view(
                ChromePart::ImmersiveBackground,
                StyleRefinement::default().size_full(),
            )))
            .child(content)
            .child(div().absolute().top_5().right_6().child(menu))
            .into_any_element()
    }
    pub(super) fn immersive_identity(&self, track: &Track, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .w_full()
            .h_full()
            .gap_3()
            .text_color(rgb(0xffffff))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_2xl()
                            .font_family(raag::i18n::font())
                            .font_weight(FontWeight::BOLD)
                            .line_height(relative(1.15))
                            .line_clamp(2)
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xffffff).opacity(0.68))
                            .child(self.artist_link(track, cx)),
                    ),
            )
            .child(
                self.like_button(track, "immersive-like", cx)
                    .text_color(rgb(0xffffff)),
            )
            .into_any_element()
    }
    pub(super) fn immersive_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h_full()
            .gap_5()
            .child(
                Button::new("immersive-repeat")
                    .ghost()
                    .icon(icon("repeat"))
                    .text_color(rgb(0xffffff))
                    .selected(self.queue.repeat != Repeat::Off)
                    .tooltip(t("Repeat"))
                    .accessibility_label(t("Repeat"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.queue.repeat = match this.queue.repeat {
                            Repeat::Off => Repeat::All,
                            Repeat::All => Repeat::One,
                            Repeat::One => Repeat::Off,
                        };
                        cx.notify();
                    })),
            )
            .child(self.basic_controls("immersive", cx))
            .child(
                Button::new("immersive-shuffle")
                    .ghost()
                    .icon(icon("shuffle"))
                    .text_color(rgb(0xffffff))
                    .selected(self.queue.shuffle)
                    .tooltip(t("Shuffle"))
                    .accessibility_label(t("Shuffle"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.queue.toggle_shuffle();
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    pub(super) fn immersive_lyric_list(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.lyrics_loading {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_center()
                .text_color(rgb(0xffffff).opacity(0.65))
                .child(t("Finding lyrics…"))
                .into_any_element();
        }
        if self.lyrics.is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_center()
                .text_color(rgb(0xffffff).opacity(0.65))
                .child(
                    self.lyrics_error
                        .clone()
                        .unwrap_or_else(|| t("No lyrics for this song yet.").into()),
                )
                .into_any_element();
        }
        let owner = cx.weak_entity();
        let manual = owner.clone();
        let controller = self.immersive_scroll.on_user_scroll(move |_, cx| {
            let _ = manual.update(cx, |this, cx| {
                this.lyrics_follow = false;
                cx.notify();
            });
        });
        let count = self.lyrics.len() + 4;
        let untimed = !self.lyrics.iter().any(|l| l.seconds.is_some());
        let list = uniform_list("immersive-lyrics", count, move |range, _, cx| {
            owner
                .update(cx, |this, cx| {
                    range
                        .map(|row| {
                            if row < 2 || row >= this.lyrics.len() + 2 {
                                return div().h(this.lyric_height()).into_any_element();
                            }
                            let index = row - 2;
                            let line = &this.lyrics[index];
                            let time = line.seconds;
                            let active = this.lyrics_active == Some(index);
                            content_button(("immersive-line", index))
                                .w_full()
                                .h(this.lyric_height())
                                .justify_center()
                                .items_center()
                                .text_center()
                                .overflow_hidden()
                                .px_3()
                                .accessibility_label(tr!("Seek to {}", line.text))
                                .disabled(time.is_none())
                                .child(this.lyric_label(
                                    line,
                                    active || untimed,
                                    rgb(0xffffff).into(),
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(time) = time {
                                        this.commit_seek(
                                            (time
                                                - this.selected.as_ref().map_or(0., |t| {
                                                    this.library.lyrics.offset(&t.id)
                                                }))
                                            .max(0.),
                                            cx,
                                        );
                                    }
                                }))
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .py_8()
            .gap_3()
            .child(div().flex_1().min_h_0().child(controller.bind_list(list)))
            .when(!self.lyrics_follow, |body| {
                body.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px_3()
                        .text_xs()
                        .text_color(rgb(0xffffff).opacity(0.5))
                        .child(
                            Button::new("immersive-follow")
                                .ghost()
                                .small()
                                .label(t("Follow song"))
                                .text_color(rgb(0xffffff))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.lyrics_follow = true;
                                    this.lyrics_active = None;
                                    this.lyrics_geometry = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .into_any_element()
    }
}
