//! Display-paced wheel easing; precision trackpads retain native momentum.
use gpui_kit::*;
use std::{cell::RefCell, rc::Rc, time::Instant};

/// Small, finite row reveals; late materialized rows appear immediately.
pub fn stagger_phase(elapsed: f32, index: usize) -> f32 {
    if index >= 12 {
        return 1.;
    }
    let phase = ((elapsed - index as f32 * 0.018) / 0.18).clamp(0., 1.);
    1. - (1. - phase).powi(3)
}

#[derive(Default)]
struct Travel {
    target: f32,
    direction: f32,
    running: bool,
    sampled: Option<Instant>,
    epoch: u64,
    velocity: f32,
    frames: Vec<f32>,
}

type ScrollCallback = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct SmoothScroll {
    handle: ScrollHandle,
    travel: Rc<RefCell<Travel>>,
    list: UniformListScrollHandle,
    on_scroll: Option<ScrollCallback>,
    profile: bool,
}

/// Capture outside the scrolling content: a bubble listener on the viewport
/// runs AFTER GPUI's default scroll, causing a jump before easing.
#[derive(IntoElement)]
pub struct SmoothViewport<E: IntoElement + 'static> {
    element: E,
    controller: SmoothScroll,
}
impl<E: IntoElement + 'static + Styled> Styled for SmoothViewport<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}
impl<E: IntoElement + 'static + ParentElement> ParentElement for SmoothViewport<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements);
    }
}
impl<E: IntoElement + 'static + InteractiveElement> InteractiveElement for SmoothViewport<E> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}
impl<E: IntoElement + 'static + StatefulInteractiveElement> StatefulInteractiveElement
    for SmoothViewport<E>
{
}
impl<E: IntoElement + 'static> RenderOnce for SmoothViewport<E> {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let controller = self.controller;
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.element)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let view = window.current_view();
                        window.on_mouse_event(
                            move |event: &ScrollWheelEvent, phase, window, cx| {
                                if phase.capture() && bounds.contains(&event.position) {
                                    controller.wheel(event, view, window, cx);
                                }
                            },
                        );
                    },
                )
                .absolute()
                .inset_0(),
            )
    }
}

impl SmoothScroll {
    pub fn new() -> Self {
        let list = UniformListScrollHandle::new();
        let handle = list.0.borrow().base_handle.clone();
        Self {
            handle,
            travel: Rc::default(),
            list,
            on_scroll: None,
            profile: std::env::var_os("RAAG_PROFILE_SCROLL").is_some(),
        }
    }
    pub fn reset(&self) {
        self.cancel();
        self.handle.set_offset(point(px(0.), px(0.)));
    }
    pub fn on_user_scroll(&self, callback: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        let mut controller = self.clone();
        controller.on_scroll = Some(Rc::new(callback));
        controller
    }
    pub fn viewport_height(&self) -> f32 {
        f32::from(self.handle.bounds().size.height)
    }
    pub fn offset(&self) -> f32 {
        f32::from(self.handle.offset().y)
    }
    pub fn near_end(&self) -> bool {
        near_end(
            self.viewport_height(),
            self.max_offset(),
            f32::from(self.handle.offset().y),
        )
    }
    pub fn max_offset(&self) -> f32 {
        f32::from(self.handle.max_offset().y).max(0.)
    }
    pub fn scroll_to(&self, target: f32, view: EntityId, window: &mut Window, cx: &mut App) {
        let max = f32::from(self.handle.max_offset().y).max(0.);
        let target = target.clamp(-max, 0.);
        if cx.reduce_motion() {
            self.cancel();
            self.handle.set_offset(point(px(0.), px(target)));
            cx.notify(view);
            return;
        }
        let mut travel = self.travel.borrow_mut();
        travel.target = target;
        if !travel.running {
            travel.running = true;
            travel.sampled = Some(Instant::now());
            let epoch = travel.epoch;
            drop(travel);
            self.schedule(view, epoch, window);
        }
    }
    fn cancel(&self) {
        let epoch = self.travel.borrow().epoch.wrapping_add(1);
        *self.travel.borrow_mut() = Travel {
            epoch,
            ..Travel::default()
        };
    }
    pub fn bind(&self, viewport: Stateful<Div>) -> SmoothViewport<Stateful<Div>> {
        SmoothViewport {
            element: viewport.track_scroll(&self.handle),
            controller: self.clone(),
        }
    }
    pub fn bind_list(&self, list: UniformList) -> SmoothViewport<UniformList> {
        SmoothViewport {
            element: list.track_scroll(&self.list).size_full(),
            controller: self.clone(),
        }
    }
    fn wheel(&self, event: &ScrollWheelEvent, view: EntityId, window: &mut Window, cx: &mut App) {
        if let Some(callback) = &self.on_scroll {
            callback(window, cx);
        }
        if cx.reduce_motion() || !matches!(event.delta, ScrollDelta::Lines(_)) {
            self.cancel();
            return;
        }
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        let max = f32::from(self.handle.max_offset().y).max(0.);
        if delta == 0. || max == 0. {
            return;
        }
        let current = f32::from(self.handle.offset().y);
        let mut travel = self.travel.borrow_mut();
        if travel.direction != delta.signum() {
            travel.velocity = 0.;
        }
        travel.target = wheel_target(
            current,
            travel.target,
            travel.direction,
            delta,
            max,
            travel.running,
        );
        travel.direction = delta.signum();
        if !travel.running {
            travel.running = true;
            travel.sampled = Some(Instant::now());
            let epoch = travel.epoch;
            drop(travel);
            self.schedule(view, epoch, window);
        }
        cx.stop_propagation();
    }
    fn schedule(&self, view: EntityId, epoch: u64, window: &mut Window) {
        let controller = self.clone();
        window.on_next_frame(move |window, cx| {
            let mut travel = controller.travel.borrow_mut();
            if !travel.running || travel.epoch != epoch {
                return;
            }
            let now = Instant::now();
            let dt = travel
                .sampled
                .replace(now)
                .map(|before| now.duration_since(before).as_secs_f32())
                .unwrap_or(1. / 60.);
            if controller.profile {
                travel.frames.push(dt * 1000.);
            }
            let current = f32::from(controller.handle.offset().y);
            let max = f32::from(controller.handle.max_offset().y).max(0.);
            travel.target = travel.target.clamp(-max, 0.);
            let next = if cx.reduce_motion() {
                travel.velocity = 0.;
                travel.target
            } else {
                let (position, velocity) = spring(current, travel.velocity, travel.target, dt);
                travel.velocity = velocity;
                position.clamp(-max, 0.)
            };
            let settled = (next - travel.target).abs() < 0.25 && travel.velocity.abs() < 3.;
            controller.handle.set_offset(point(
                px(0.),
                px(if settled { travel.target } else { next }),
            ));
            travel.running = !settled;
            if settled && controller.profile {
                report_frames(std::mem::take(&mut travel.frames));
            }
            drop(travel);
            cx.notify(view);
            if !settled {
                controller.schedule(view, epoch, window);
            }
        });
    }
}
fn report_frames(mut frames: Vec<f32>) {
    if frames.is_empty() {
        return;
    }
    // Capture frame pacing without disk I/O in the animation callback.
    std::thread::spawn(move || {
        use std::io::Write;
        frames.sort_by(f32::total_cmp);
        let mean = frames.iter().sum::<f32>() / frames.len() as f32;
        let p95 = frames[((frames.len() as f32 * 0.95).ceil() as usize - 1).min(frames.len() - 1)];
        let delayed = frames.iter().filter(|ms| **ms > 25.).count();
        let path = raag::library::data_dir().join("scroll-profile.csv");
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(
                file,
                "{},{mean:.2},{p95:.2},{:.2},{delayed}",
                frames.len(),
                frames.last().unwrap()
            );
        }
    });
}
fn wheel_target(
    current: f32,
    target: f32,
    direction: f32,
    delta: f32,
    max: f32,
    running: bool,
) -> f32 {
    let base = if !running || direction != delta.signum() {
        current
    } else {
        target
    };
    (base + delta).clamp(-max, 0.)
}
fn spring(current: f32, velocity: f32, target: f32, seconds: f32) -> (f32, f32) {
    let dt = seconds.clamp(0., 0.1);
    let rate = 26.;
    let displacement = current - target;
    let impulse = velocity + rate * displacement;
    let decay = (-rate * dt).exp();
    (
        target + (displacement + impulse * dt) * decay,
        (velocity - rate * impulse * dt) * decay,
    )
}

/// Independent, critically damped channels preserve velocity on interruption.
/// Shell edges and the center viewport consume the same sampled geometry.
pub(crate) struct ShellMotion {
    states: [SpringState; 3],
    targets: [f32; 3],
    sampled: Instant,
}
impl ShellMotion {
    pub(crate) fn new(expanded: bool) -> Self {
        let targets = [if expanded { 256. } else { 0. }, 0., 0.];
        Self {
            states: targets.map(|position| SpringState {
                position,
                velocity: 0.,
            }),
            targets,
            sampled: Instant::now(),
        }
    }
    pub(crate) fn dimensions(&self) -> [f32; 3] {
        // Reuse cached page paint when the remaining movement is subpixel.
        // Simulation keeps full precision so interruptions preserve velocity.
        self.states
            .map(|state| (state.position.max(0.) * 2.).round() / 2.)
    }
    pub(crate) fn sample(&mut self, targets: [f32; 3], reduced: bool, now: Instant) -> bool {
        let seconds = now.duration_since(self.sampled).as_secs_f32();
        self.sampled = now;
        let config = SpringConfig::new(625., 50., 1.);
        let mut running = false;
        for (index, target) in targets.into_iter().enumerate() {
            // Advance the previous destination before applying a new one.
            // An idle interval must never jump a newly toggled pane to its end.
            let state = config.step(self.states[index], self.targets[index], seconds);
            self.targets[index] = target;
            self.states[index] = if reduced || config.is_settled(state, target, 0.5) {
                SpringState {
                    position: target,
                    velocity: 0.,
                }
            } else {
                running = true;
                state
            };
        }
        running
    }
}

/// One notch is 5%; accumulate precision-trackpad deltas instead of applying
/// a full step to every tiny packet. Right/up increases volume.
#[derive(Default)]
pub struct VolumeScroll {
    remainder: f32,
    sampled: Option<Instant>,
}
impl VolumeScroll {
    pub fn steps(&mut self, delta: ScrollDelta, now: Instant) -> i32 {
        if self
            .sampled
            .is_none_or(|at| now.duration_since(at).as_millis() > 350)
        {
            self.remainder = 0.;
        }
        self.sampled = Some(now);
        let amount = match delta {
            ScrollDelta::Lines(point) => {
                if point.x.abs() > point.y.abs() {
                    -point.x.signum()
                } else {
                    point.y.signum()
                }
            }
            ScrollDelta::Pixels(point) => {
                let x = f32::from(point.x);
                let y = f32::from(point.y);
                if x.abs() > y.abs() { -x / 32. } else { y / 32. }
            }
        };
        if amount.signum() != self.remainder.signum() && amount != 0. {
            self.remainder = 0.;
        }
        self.remainder += amount;
        let steps = self.remainder.trunc() as i32;
        self.remainder -= steps as f32;
        steps
    }
}

// Resolved viewport geometry also covers virtualized rows, trackpads and keyboard scrolling.
fn near_end(height: f32, max: f32, offset: f32) -> bool {
    height > 0. && max.is_finite() && offset.is_finite() && max + offset.min(0.) <= 320.
}
