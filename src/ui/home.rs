use super::*;
impl Raag {
    pub(super) fn home(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        // Shelf covers retain size; toggles reveal or clip neighbouring items.
        let columns = ((self.content_width(window) - 64. + 20.) / 204.)
            .floor()
            .clamp(1., 8.) as usize;
        let stacked = self.layout_width(window) < 680.;
        let picks_width = self.layout_width(window) - 64. - if stacked { 0. } else { 344. + 24. };
        let pick_columns = (picks_width / 190.).floor().clamp(1., 3.) as usize;
        let recent = self.recent_tracks();
        let picks: Vec<_> = recent
            .iter()
            .filter(|track| track.playable())
            .take(pick_columns * 3)
            .collect();
        let mut grid = div().flex().flex_col().gap_2().flex_1().min_w_0().w_full();
        for chunk in picks.chunks(pick_columns) {
            let mut row = div().flex().gap_3().w_full().min_w_0();
            for track in chunk {
                row = row.child(div().flex_1().min_w_0().child(self.song_tile(track, cx)));
            }
            for _ in chunk.len()..pick_columns {
                row = row.child(div().flex_1());
            }
            grid = grid.child(row);
        }
        if picks.is_empty() {
            grid = grid.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Your recently played songs will appear here."),
            );
        }
        let mut page = self
            .scroll
            .bind(div().id("home-scroll"))
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .px_8()
            .pb_8()
            .gap_7()
            .child(self.page_heading(
                "Your day, in music.",
                "A familiar favourite. Something unexpected.",
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_stretch()
                    .gap_6()
                    .flex_shrink_0()
                    .when(stacked, |row| row.flex_col())
                    .child(self.mix_cassette(window, cx))
                    .child(grid),
            );
        if self.page.sections.is_empty() {
            page = page
                .child(self.home_shelf("On repeat", self.editorial.as_ref(), columns, window, cx))
                .child(self.home_shelf(
                    "A little more energy",
                    &self.editorial[5..],
                    columns,
                    window,
                    cx,
                ));
        } else {
            for section in &self.page.sections {
                if !section.tracks.is_empty() {
                    page = page.child(self.home_shelf(
                        &section.title,
                        &section.tracks,
                        columns,
                        window,
                        cx,
                    ));
                }
            }
        }
        page.into_any_element()
    }
    fn mix_cassette(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let available = (self.content_width(window) - 64.).max(180.);
        // Keep the tape and its label steady when either sidebar changes width.
        let width = available.min(344.);
        div()
            .relative()
            .w(px(width))
            .h(px(width * 537. / 768.))
            .flex_shrink_0()
            .child(
                img("raag/cassette-home.png")
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            )
            .child(
                div()
                    .absolute()
                    // The first ruled line is at y=84 in the 768px-wide asset.
                    .top(px(width * 40. / 768.))
                    .left(px(width * 148. / 768.))
                    .right(relative(0.10))
                    .text_color(rgb(0x123c43))
                    .child(
                        div()
                            .font_family("Manrope")
                            .font_weight(FontWeight::BOLD)
                            .text_size(px((width * 0.048).clamp(12., 23.)))
                            .line_height(relative(1.1))
                            .child("Find your frequency."),
                    ),
            )
            .child(
                div()
                    .absolute()
                    // Give the second line its own baseline instead of flowing
                    // after the heading's unrelated font metrics.
                    .top(px(width * 85. / 768.))
                    .left(px(width * 148. / 768.))
                    .right(relative(0.10))
                    .text_color(rgb(0x123c43))
                    .child(
                        div()
                            .text_size(px((width * 0.033).clamp(10., 14.)))
                            .line_height(relative(1.1))
                            .child("A mix for the moment"),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .bottom(relative(0.075))
                    .left_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        Button::new("play-rotation")
                            .map(playback_depth)
                            .small()
                            .rounded_full()
                            .bg(rgb(0x0c3941))
                            .text_color(rgb(0xfff7e6))
                            .icon(icon("play"))
                            .label("Play mix")
                            .on_click(cx.listener(|this, _, _, cx| this.start_mix(cx))),
                    ),
            )
            .into_any_element()
    }
    fn home_shelf(
        &self,
        title: &str,
        tracks: &[Track],
        columns: usize,
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
        let key = format!(
            "{title}:{}",
            tracks
                .first()
                .map(|track| track.id.as_str())
                .unwrap_or("empty")
        );
        let end = tracks.len().saturating_sub(columns);
        let start = self
            .shelf_positions
            .get(&key)
            .copied()
            .unwrap_or_default()
            .min(tracks.len().saturating_sub(1));
        let mut arrows = div().flex().items_center().gap_2();
        for (direction, glyph, label, disabled) in [
            (-1isize, "nav-arrow-left", "Previous items", start == 0),
            (1, "nav-arrow-right", "Next items", start >= end),
        ] {
            let key = key.clone();
            arrows = arrows.child(
                Button::new(SharedString::from(format!("shelf-{key}-{direction}")))
                    .ghost()
                    .small()
                    .rounded_full()
                    .icon(icon(glyph))
                    .disabled(disabled)
                    .tooltip(label)
                    .accessibility_label(format!("{label} in {title}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let next = start
                            .saturating_add_signed(
                                direction * columns.saturating_sub(1).max(1) as isize,
                            )
                            .min(end);
                        this.shelf_positions.insert(key.clone(), next);
                        this.content_revision += 1;
                        cx.notify();
                    })),
            );
        }
        let extent = 184.;
        let mut cards = div()
            .id(SharedString::from(format!("shelf-track-{key}")))
            .flex()
            .gap_5()
            .items_start()
            .flex_shrink_0()
            .w(px(tracks.len() as f32 * (extent + 20.) - 20.));
        for (index, track) in tracks.iter().enumerate() {
            // Keep the current and neighbouring pages for the slide animation.
            // Distant cards keep their slots but do not create image/text trees.
            let nearby = index >= start.saturating_sub(columns) && index < start + 2 * columns;
            cards = cards.child(
                div()
                    .w(px(extent))
                    .flex_shrink_0()
                    .min_w_0()
                    .when(nearby, |cell| {
                        cell.child(self.home_card(track, extent, window, cx))
                    }),
            );
        }
        let carousel = div().w_full().min_w_0().overflow_hidden().child(
            cards.with_spring(
                SharedString::from(format!("carousel-{key}")),
                SpringAnimation::new(SpringConfig::new(400., 40., 1.))
                    .to(-(start as f32) * (extent + 20.))
                    .with_epsilon(0.1),
                |row, offset| row.relative().left(px(offset)),
            ),
        );
        div()
            .flex()
            .flex_col()
            .gap_4()
            .flex_shrink_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xl()
                            .font_family("Manrope")
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title.to_owned()),
                    )
                    .child(arrows),
            )
            .child(carousel)
            .into_any_element()
    }
    fn home_card(
        &self,
        track: &Track,
        extent: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owned = track.clone();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                content_button(SharedString::from(format!("home-art-{}", track.id)))
                    .p_0()
                    .w_full()
                    .h_auto()
                    .aspect_ratio(1.)
                    .rounded_lg()
                    .overflow_hidden()
                    .hover(|button| button.opacity(0.86))
                    .accessibility_label(format!("Open {}", track.title))
                    .child(self.cover(track, rems(extent / f32::from(window.rem_size())), cx))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_track(owned.clone(), cx)),
                    ),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .flex_col()
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
            .into_any_element()
    }
}
