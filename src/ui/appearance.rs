use super::*;
impl Raag {
    pub(super) fn apply_appearance(&mut self, cx: &mut Context<Self>) {
        let colors = self
            .selected
            .as_ref()
            .and_then(|track| {
                self.artwork_colors
                    .get(track.artwork_key())
                    .or_else(|| self.artwork_colors.get(&track.id))
            })
            .copied();
        theme::apply(&self.library, colors, cx);
        self.content_revision += 1;
        self.navigation_revision = self.navigation_revision.wrapping_add(1);
        self.right_content.update(cx, |_, cx| cx.notify());
        self.playback_progress.update(cx, |_, cx| cx.notify());
        self.mini_progress.update(cx, |_, cx| cx.notify());
        for pane in &self.chrome {
            pane.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }
    pub(super) fn appearance_settings(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = theme::selected(&self.library);
        let available = (self.content_width(window) - 64.).clamp(220., 880.);
        let density_width = (self.layout_width(window) - 64.).clamp(220., 880.);
        let columns = if density_width >= 740. {
            4
        } else if density_width >= 550. {
            3
        } else {
            2
        };
        let extent = (available - (columns - 1) as f32 * 16.) / columns as f32;
        let mut tiles = div().flex().flex_wrap().gap_4().w(px(available));
        let palettes = theme::palettes();
        let selected_in_row = palettes
            .iter()
            .take(columns)
            .any(|palette| palette.id == selected.id);
        for (index, palette) in palettes.iter().enumerate() {
            if !self.show_all_themes
                && index >= columns - usize::from(!selected_in_row)
                && palette.id != selected.id
            {
                continue;
            }
            let id = palette.id.clone();
            let light = palette.light;
            let active = selected.id == palette.id;
            tiles = tiles.child(
                content_button(SharedString::from(format!("theme-{}", palette.id)))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .w(px(extent))
                    .justify_start()
                    .cursor_pointer()
                    .accessibility_label(format!(
                        "{} theme{}",
                        palette.label,
                        if active { ", selected" } else { "" }
                    ))
                    .child(self.theme_preview(palette, extent, active, cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if active {
                                cx.theme().primary
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(palette.label.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.library.theme = Some(id.clone());
                        this.library.light = light;
                        this.apply_appearance(cx);
                        this.save();
                    })),
            );
        }
        let mut appearance = div()
            .flex()
            .flex_col()
            .gap_5()
            .max_w(rems(55.))
            .child(
                div()
                    .text_xl()
                    .font_family("Manrope")
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Appearance"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Theme"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Choose a home for your music."),
                    ),
            )
            .child(tiles)
            .child(
                Button::new("browse-themes")
                    .ghost()
                    .label(if self.show_all_themes {
                        "Show fewer themes"
                    } else {
                        "Browse all themes"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_all_themes = !this.show_all_themes;
                        this.content_revision += 1;
                        cx.notify();
                    })),
            );
        if matches!(selected.id.as_str(), "light" | "dark") {
            let mut accents = div().flex().items_center().gap_3().flex_wrap();
            for (color, label) in theme::ACCENTS {
                let active =
                    !self.library.magic_accent && self.library.accent.unwrap_or(0xe11d48) == color;
                accents = accents.child(
                    content_button(("accent", color as usize))
                        .size_10()
                        .rounded_full()
                        .bg(rgb(color))
                        .border_2()
                        .border_color(if active {
                            cx.theme().foreground
                        } else {
                            rgb(color).into()
                        })
                        .accessibility_label(format!(
                            "{label} accent{}",
                            if active { ", selected" } else { "" }
                        ))
                        .when(active, |button| {
                            button.child(
                                icon("check").text_color(rgb(raag::artwork::on_accent(color))),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.library.accent = Some(color);
                            this.library.magic_accent = false;
                            this.apply_appearance(cx);
                            this.save();
                        })),
                );
            }
            accents = accents.child(
                Button::new("magic-accent")
                    .ghost()
                    .icon(icon("magic"))
                    .size_10()
                    .rounded_full()
                    .selected(self.library.magic_accent)
                    .tooltip("Magic accent · follows the current artwork")
                    .accessibility_label("Magic accent from current artwork")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.library.magic_accent = true;
                        this.apply_appearance(cx);
                        this.save();
                    })),
            );
            appearance = appearance.child(div().flex().flex_col().gap_3().mt_2()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Accent colour"))
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child(if self.library.magic_accent { "Magic follows your artwork. Without artwork, the accent is black in Light and white in Dark." } else { "Choose a colour, or let Magic follow the song." }))
                .child(accents));
        } else {
            appearance = appearance.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("This theme includes its own accent and component colours."),
            );
        }
        appearance.into_any_element()
    }
    fn theme_preview(
        &self,
        palette: &theme::Palette,
        width: f32,
        selected: bool,
        cx: &App,
    ) -> AnyElement {
        let c = |token| -> Hsla { rgb(palette.color(token)).into() };
        let mut sidebar = div()
            .w_7()
            .h_full()
            .p_2()
            .rounded_sm()
            .bg(rgb(palette.shell_color()))
            .flex()
            .flex_col()
            .gap_1();
        for index in 0..5 {
            sidebar = sidebar.child(div().h(px(3.)).w_full().rounded_full().bg(c(if index == 0 {
                "primary"
            } else {
                "border"
            })));
        }
        div()
            .relative()
            .flex()
            .gap_2()
            .p_3()
            .w(px(width))
            .h(px(96.))
            .rounded_lg()
            .bg(c("background"))
            .border_2()
            .border_color(if selected {
                cx.theme().primary
            } else {
                c("border")
            })
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_2()
                    .child(
                        div()
                            .h(px(5.))
                            .w(relative(0.45))
                            .rounded_full()
                            .bg(c("foreground")),
                    )
                    .child(div().h(px(3.)).w_full().rounded_full().bg(c("border")))
                    .child(
                        div()
                            .h(px(3.))
                            .w(relative(0.8))
                            .rounded_full()
                            .bg(c("border")),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(div().flex_1().h_4().rounded_sm().bg(c("card")))
                            .child(div().flex_1().h_4().rounded_sm().bg(c("card"))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(div().flex_1().h_3().rounded_sm().bg(c("card")))
                            .child(div().flex_1().h_3().rounded_sm().bg(c("card"))),
                    ),
            )
            .when(selected, |preview| {
                preview.child(
                    div()
                        .absolute()
                        .top_1()
                        .right_1()
                        .size_5()
                        .rounded_full()
                        .bg(cx.theme().primary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            icon("check")
                                .size_3()
                                .text_color(cx.theme().primary_foreground),
                        ),
                )
            })
            .into_any_element()
    }
}
