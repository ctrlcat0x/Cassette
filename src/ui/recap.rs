use super::*;
impl Raag {
    pub(super) fn recap_screen(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let width = (self.content_width(window) - 96.).max(240.);
        let compact = width < 850.;
        // Poster palette follows the supplied chart reference, independently
        // of the surrounding app theme, with explicit text roles throughout.
        let blue: Hsla = rgb(0x2879ed).into();
        let yellow: Hsla = rgb(0xffdf70).into();
        let ink: Hsla = rgb(0x101c32).into();
        let period = (self.recap_period > 0).then(|| {
            self.library
                .stats
                .period(if self.recap_period == 1 { 30 } else { 365 })
        });
        let stats = period.as_ref().unwrap_or(&self.library.stats);
        let top = stats.top_tracks();
        let artists = stats.ranking(true);
        let albums = stats.ranking(false);
        let mut poster = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap_7()
            .p_7()
            .bg(blue)
            .text_color(ink)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .text_lg()
                            .font_family(raag::i18n::font())
                            .font_weight(FontWeight::BOLD)
                            .child("Cassette / listening story"),
                    )
                    .child(
                        div().flex().flex_wrap().gap_2().children(
                            [t("All time"), t("30 days"), t("365 days")]
                                .into_iter()
                                .enumerate()
                                .map(|(index, label)| {
                                    Button::new(("recap-period", index))
                                        .ghost()
                                        .label(label)
                                        .selected(self.recap_period == index)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.recap_period = index;
                                            this.content_revision += 1;
                                            cx.notify();
                                        }))
                                }),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(px((width * 0.12).clamp(48., 180.)))
                    .line_height(relative(0.94))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .font_family("Segoe UI Black")
                    .text_color(yellow)
                    .child(if width < 520. {
                        t("YOUR\nCHARTS")
                    } else {
                        t("YOUR CHARTS")
                    }),
            );
        let mut details = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_6()
            .child(
                div()
                    .text_2xl()
                    .font_family(raag::i18n::font())
                    .line_height(relative(1.15))
                    .font_weight(FontWeight::BOLD)
                    .text_color(yellow)
                    .child(t("Your music, on repeat.")),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_7()
                    .child(self.recap_metric(
                        &(stats.seconds / 60.).floor().to_string(),
                        t("Minutes listened"),
                        yellow,
                    ))
                    .child(self.recap_metric(&top.len().to_string(), t("Songs"), yellow))
                    .child(self.recap_metric(&artists.len().to_string(), t("Artists"), yellow)),
            );
        if let Some(favourite) = top.first() {
            let mut rankings = div()
                .flex()
                .w_full()
                .min_w_0()
                .gap_6()
                .when(compact, |div| div.flex_col());
            let songs: Vec<_> = top
                .iter()
                .take(5)
                .map(|entry| (entry.track.title.clone(), entry.seconds))
                .collect();
            for (title, entries, artist) in [
                (t("Top artists"), &artists, true),
                (t("Top songs"), &songs, false),
                (t("Top albums"), &albums, false),
            ] {
                let mut column = div().flex().flex_col().flex_1().min_w_0().gap_3().child(
                    div()
                        .text_lg()
                        .font_family(raag::i18n::font())
                        .font_weight(FontWeight::BOLD)
                        .child(title),
                );
                for (index, (name, _)) in entries.iter().take(5).enumerate() {
                    let identity = if artist {
                        self.artist_link(&Track::song(format!("chart-artist:{name}"), "", name), cx)
                    } else {
                        div().truncate().child(name.clone()).into_any_element()
                    };
                    column = column.child(
                        div()
                            .flex()
                            .gap_2()
                            .min_w_0()
                            .text_sm()
                            .line_height(relative(1.25))
                            .child(div().w_4().flex_shrink_0().child((index + 1).to_string()))
                            .child(div().flex_1().min_w_0().overflow_hidden().child(identity)),
                    );
                }
                if entries.is_empty() {
                    column = column.child(
                        div()
                            .text_sm()
                            .child(t("Album details appear as you listen.")),
                    );
                }
                rankings = rankings.child(column);
            }
            details = details.child(rankings);
            let extent = (width * 0.25).clamp(150., 280.);
            let track = favourite.track.clone();
            let owned = track.clone();
            let feature = div()
                .id("chart-favourite")
                .flex()
                .flex_col()
                .flex_shrink_0()
                .w(px(extent))
                .gap_3()
                .child(self.cover(&track, rems(extent / f32::from(window.rem_size())), cx))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t("Your most played")),
                )
                .child(
                    div()
                        .text_lg()
                        .font_family(raag::i18n::font())
                        .font_weight(FontWeight::BOLD)
                        .line_height(relative(1.2))
                        .text_color(yellow)
                        .child(track.title.clone()),
                )
                .child(div().text_sm().child(self.artist_link(&track, cx)))
                .child(
                    Button::new("chart-play-favourite")
                        .label(t("Play again"))
                        .icon(icon("play"))
                        .bg(ink)
                        .text_color(yellow)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_track(owned.clone(), cx)),
                        ),
                );
            poster = poster.child(
                div()
                    .flex()
                    .gap_7()
                    .items_start()
                    .min_w_0()
                    .when(compact, |div| div.flex_col())
                    .child(feature)
                    .child(details),
            );
        } else {
            poster = poster.child(details).child(
                div()
                    .py_8()
                    .text_lg()
                    .font_family(raag::i18n::font())
                    .child(t(t(
                        "Play a song to start your charts. Your story grows as you listen.",
                    ))),
            );
        }
        self.scroll
            .bind(div().id("recap-scroll"))
            .size_full()
            .overflow_y_scroll()
            .px_8()
            .pt_5()
            .pb_8()
            .child(div().p_3().bg(yellow).child(poster))
            .into_any_element()
    }
    fn recap_metric(&self, value: &str, label: &str, color: Hsla) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .text_color(color)
            .child(
                div()
                    .text_3xl()
                    .font_family(raag::i18n::font())
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(value.to_owned()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x101c32))
                    .child(label.to_owned()),
            )
    }
}
