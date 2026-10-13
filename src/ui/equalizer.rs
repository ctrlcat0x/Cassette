use super::*;

const LABELS: [&str; 5] = ["60 Hz", "230 Hz", "910 Hz", "3.6 kHz", "14 kHz"];
const HEIGHT: f32 = 196.;
const INSET: f32 = 16.;

// The graph represents band gains, not a measured frequency response.
fn gain_at(y: f32, height: f32) -> f64 {
    if height <= 0. {
        return 0.;
    }
    (((1. - y / height) * 24. - 12.).clamp(-12., 12.) * 2.).round() as f64 / 2.
}
fn band_at(x: f32, width: f32) -> usize {
    if width <= 0. {
        return 0;
    }
    (x / width * 5. - 0.5).round().clamp(0., 4.) as usize
}
fn curve(builder: &mut PathBuilder, points: &[Point<Pixels>; 5]) {
    for pair in points.windows(2) {
        let mid = (pair[0].x + pair[1].x) / 2.;
        builder.cubic_bezier_to(pair[1], point(mid, pair[0].y), point(mid, pair[1].y));
    }
}
impl Raag {
    fn set_eq_gain(&mut self, band: usize, gain: f64, cx: &mut Context<Self>) {
        let gain = gain.clamp(-12., 12.);
        if self.library.audio.equalizer[band] == gain {
            return;
        }
        self.library.audio.equalizer[band] = gain;
        // Quantized live changes; save once at the end of the gesture.
        self.player
            .command(PlayerCommand::Configure(self.library.audio.clone()));
        self.content_revision += 1;
        cx.notify();
    }
    pub(super) fn equalizer_graph(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let gains = self.library.audio.equalizer;
        let active = self.eq_band;
        let focused = self.eq_focus.is_focused(window);
        let accent = cx.theme().primary;
        let grid = cx.theme().border;
        let background = cx.theme().background;
        let foreground = cx.theme().foreground;
        let owner = cx.weak_entity();
        div().id("equalizer-graph").track_focus(&self.eq_focus)
            .flex().flex_col().gap_3().p_4().rounded_lg().border_1()
            .border_color(if focused { accent } else { grid }).bg(background)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.control || event.keystroke.modifiers.platform || event.keystroke.modifiers.alt { return; }
                match event.keystroke.key.as_str() {
                    "left" => this.eq_band = this.eq_band.saturating_sub(1),
                    "right" => this.eq_band = (this.eq_band + 1).min(4),
                    "up" | "down" | "home" => {
                        let gain = match event.keystroke.key.as_str() {
                            "up" => this.library.audio.equalizer[this.eq_band] + 0.5,
                            "down" => this.library.audio.equalizer[this.eq_band] - 0.5,
                            _ => 0.,
                        };
                        this.set_eq_gain(this.eq_band, gain, cx);
                        this.save();
                    }
                    _ => return,
                }
                this.content_revision += 1;
                cx.notify();
                cx.stop_propagation();
            }))
            .child(div().flex().items_center().justify_between().gap_3()
                .child(div().flex().flex_col().gap_1()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(t("Equalizer")))
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(t("Shape your sound · −12 to +12 dB"))))
                .child(Button::new("eq-reset").ghost().small().label(t("Reset"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.library.audio.equalizer = [0.; 5];
                        this.eq_drag = None;
                        this.apply_audio(cx);
                    }))))
            .child(div().flex().gap_2()
                .child(div().w(ui_px(34.)).flex_shrink_0().h(px(HEIGHT)).py(px(INSET))
                    .flex().flex_col().justify_between().text_xs().text_color(cx.theme().muted_foreground)
                    .children(["+12", "+6", "0 dB", "−6", "−12"]))
                .child(div().flex_1().min_w_0().cursor_pointer().child(canvas(|_, _, _| (), move |bounds, _, window, _| {
                    let plot = Bounds::new(bounds.origin + point(px(INSET), px(INSET)),
                        size((bounds.size.width - px(INSET * 2.)).max(ui_px(1.)), px(HEIGHT - INSET * 2.)));
                    let points: [Point<Pixels>; 5] = std::array::from_fn(|i| point(
                        plot.origin.x + plot.size.width * (0.1 + i as f32 / 5.),
                        plot.origin.y + plot.size.height * ((12. - gains[i]) as f32 / 24.)));
                    for row in 0..5 {
                        let y = plot.origin.y + plot.size.height * (row as f32 / 4.);
                        window.paint_quad(fill(Bounds::new(point(plot.origin.x, y), size(plot.size.width, ui_px(1.))), grid.opacity(if row == 2 { 0.95 } else { 0.45 })));
                    }
                    for p in points {
                        window.paint_quad(fill(Bounds::new(point(p.x, plot.origin.y), size(ui_px(1.), plot.size.height)), grid.opacity(0.4)));
                    }
                    let mut area = PathBuilder::fill();
                    area.move_to(points[0]); curve(&mut area, &points);
                    area.line_to(point(points[4].x, plot.bottom()));
                    area.line_to(point(points[0].x, plot.bottom())); area.close();
                    if let Ok(path) = area.build() { window.paint_path(path, accent.opacity(0.07)); }
                    let mut line = PathBuilder::stroke(ui_px(2.5));
                    line.move_to(points[0]); curve(&mut line, &points);
                    if let Ok(path) = line.build() { window.paint_path(path, accent); }
                    for (index, p) in points.iter().enumerate() {
                        if index == active && focused {
                            window.paint_quad(quad(Bounds::new(*p - point(ui_px(12.), ui_px(12.)), size(ui_px(24.), ui_px(24.))), ui_px(12.), accent.opacity(0.15), ui_px(1.), accent.opacity(0.5), BorderStyle::default()));
                        }
                        window.paint_quad(quad(Bounds::new(*p - point(ui_px(6.), ui_px(6.)), size(ui_px(12.), ui_px(12.))), ui_px(6.), background, ui_px(2.), accent, BorderStyle::default()));
                        window.paint_quad(quad(Bounds::new(*p - point(ui_px(2.), ui_px(2.)), size(ui_px(4.), ui_px(4.))), ui_px(2.), foreground, ui_px(0.), accent, BorderStyle::default()));
                    }
                    let clip = window.content_mask().bounds;
                    let down = owner.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if phase.bubble() && event.button == MouseButton::Left && bounds.contains(&event.position) && clip.contains(&event.position) {
                            let band = band_at(f32::from(event.position.x - plot.origin.x), f32::from(plot.size.width));
                            let gain = if event.click_count >= 2 { 0. } else { gain_at(f32::from(event.position.y - plot.origin.y), f32::from(plot.size.height)) };
                            let _ = down.update(cx, |this, cx| {
                                window.focus(&this.eq_focus, cx);
                                this.eq_band = band;
                                this.eq_drag = (event.click_count < 2).then_some(band);
                                this.set_eq_gain(band, gain, cx);
                                this.content_revision += 1;
                                if event.click_count >= 2 { this.save(); }
                                cx.notify();
                            });
                            cx.stop_propagation();
                        }
                    });
                    let drag = owner.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase.bubble() {
                            let _ = drag.update(cx, |this, cx| {
                                if let Some(band) = this.eq_drag {
                                    if event.dragging() {
                                        this.set_eq_gain(band, gain_at(f32::from(event.position.y - plot.origin.y), f32::from(plot.size.height)), cx);
                                        cx.stop_propagation();
                                    } else {
                                        // A release outside the window must not leave a stuck drag.
                                        this.eq_drag = None;
                                        this.save();
                                    }
                                }
                            });
                        }
                    });
                    let up = owner.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase.capture() && event.button == MouseButton::Left {
                            let _ = up.update(cx, |this, cx| {
                                if this.eq_drag.take().is_some() { this.save(); cx.stop_propagation(); }
                            });
                        }
                    });
                }).w_full().h(px(HEIGHT)))))
            .child(div().flex().pl(px(42. + INSET)).pr(px(INSET)).children(LABELS.into_iter().enumerate().map(|(band, label)| {
                div().flex_1().min_w_0().flex().flex_col().items_center().gap_1().child(
                Button::new(("eq-band", band)).ghost().small()
                    .selected(focused && active == band)
                    .label(label)
                    .accessibility_label(format!("{label}, {:+.1} decibels. Select and use up or down arrow to adjust.", gains[band]))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.eq_band = band; window.focus(&this.eq_focus, cx);
                        this.content_revision += 1; cx.notify();
                    })))
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).text_color(accent).child(format!("{:+.1} dB", gains[band])))
            })))
            .child(div().pl(ui_px(42.)).text_xs().text_color(cx.theme().muted_foreground)
                .child(t("Drag or click to adjust · Arrow keys for fine tuning · Double-click a point to reset")))
            .into_any_element()
    }
}
