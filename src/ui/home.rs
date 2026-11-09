use super::*;
impl Raag {
    pub(super) fn home(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        // Shelf covers retain size; toggles reveal or clip neighbouring items.
        let columns = ((self.content_width(window) - 64. + 20.) / 204.)
            .floor()
            .clamp(1., 8.) as usize;
        let pick_columns = ((self.content_width(window) - 64.) / 240.)
            .floor()
            .clamp(1., 4.) as usize;
        let recent = self.recent_tracks();
        let picks: Vec<_> = recent
            .iter()
            .filter(|track| track.playable())
            .take(pick_columns * 2)
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
                t("Your day, in music."),
                t("A familiar favourite. Something unexpected."),
                cx,
            ))
            .when(!picks.is_empty(), |page| {
                page.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .flex_shrink_0()
                        .child(self.section_heading(t("Recently played")))
                        .child(grid),
                )
            });
        if self.page.sections.is_empty() {
            page = page
                .child(self.home_shelf(
                    t("On repeat"),
                    self.editorial.as_ref(),
                    columns,
                    window,
                    cx,
                ))
                .child(self.home_shelf(
                    t("A little more energy"),
                    self.editorial.get(5..).unwrap_or_default(),
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
    fn home_shelf(
        &self,
        title: &str,
        tracks: &[Track],
        columns: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filtered;
        let tracks = if self.library.hidden.is_empty() && !self.library.offline_only {
            tracks
        } else {
            filtered = tracks
                .iter()
                .filter(|t| {
                    !self.library.hidden.contains(&t.id)
                        && (!self.library.offline_only
                            || t.kind == ItemKind::Local
                            || self.offline.iter().any(|cached| cached.id == t.id))
                })
                .cloned()
                .collect::<Vec<_>>();
            &filtered
        };
        if tracks.is_empty() {
            return div().into_any_element();
        }
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
            .min(end);
        let mut arrows = div().flex().items_center().gap_2();
        for (direction, glyph, label, disabled) in [
            (-1isize, "nav-arrow-left", t("Previous items"), start == 0),
            (1, "nav-arrow-right", t("Next items"), start >= end),
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
                    .accessibility_label(tr!("{label} in {title}"))
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
            .w(px((tracks.len() as f32 * (extent + 20.) - 20.).max(0.)));
        let first = start.saturating_sub(columns);
        if first > 0 {
            cards = cards.child(
                div()
                    .w(px(first as f32 * (extent + 20.) - 20.))
                    .flex_shrink_0(),
            );
        }
        for track in tracks
            .iter()
            .skip(first)
            .take((start + 2 * columns).saturating_sub(first))
        {
            cards = cards.child(
                div()
                    .w(px(extent))
                    .flex_shrink_0()
                    .min_w_0()
                    .child(self.home_card(track, extent, window, cx)),
            );
        }
        let offset = -(start as f32) * (extent + 20.);
        let cards = if cx.reduce_motion() {
            cards.relative().left(px(offset)).into_any_element()
        } else {
            cards
                .with_spring(
                    SharedString::from(format!("carousel-{key}")),
                    SpringAnimation::new(SpringConfig::new(400., 40., 1.))
                        .to(offset)
                        .with_epsilon(0.1),
                    |row, offset| row.relative().left(px(offset)),
                )
                .into_any_element()
        };
        let carousel = div().w_full().min_w_0().overflow_hidden().child(cards);
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
                            .font_family(raag::i18n::font())
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
                    .accessibility_label(tr!("Open {}", track.title))
                    .child(self.cover(track, rems(extent / f32::from(window.rem_size())), cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_track(owned.clone(), cx)))
                    .map(|button| HoverArtwork {
                        id: SharedString::from(format!("home-hover-{}", track.id)),
                        view: self.content.entity_id(),
                        child: button.into_any_element(),
                    }),
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
