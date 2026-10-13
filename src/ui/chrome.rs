use super::*;

#[derive(Clone, Copy)]
pub(super) enum ChromePart {
    Toolbar,
    Identity,
    Transport,
    Actions,
    ImmersiveArt,
    ImmersiveBackground,
    ImmersiveTitle,
    ImmersiveButtons,
    ImmersiveLyrics,
}
const PARTS: [ChromePart; 9] = [
    ChromePart::Toolbar,
    ChromePart::Identity,
    ChromePart::Transport,
    ChromePart::Actions,
    ChromePart::ImmersiveArt,
    ChromePart::ImmersiveBackground,
    ChromePart::ImmersiveTitle,
    ChromePart::ImmersiveButtons,
    ChromePart::ImmersiveLyrics,
];

#[derive(PartialEq)]
enum ChromeState {
    Toolbar {
        auth: u64,
        artwork: u64,
        suggestions: (u64, usize),
        expanded: bool,
        focused: bool,
        busy: (bool, bool, bool),
        syncing: bool,
        history: (bool, bool),
    },
    Identity(u64, u64, bool, bool, bool),
    Transport(bool, bool, bool, bool, bool),
    Actions(
        u64,
        (u64, u64),
        u64,
        bool,
        Repeat,
        Option<Panel>,
        bool,
        bool,
    ),
    Art(u64, u64),
    Background(u64, u64, bool, (u32, u32)),
    Title(u64, (u64, u64)),
    Buttons(bool, bool, bool, bool, Repeat),
    Lyrics(u64, u64, bool, Option<usize>, Option<usize>, bool),
}

pub(super) struct ChromePane {
    owner: WeakEntity<Raag>,
    part: ChromePart,
    state: Option<ChromeState>,
}
impl ChromePane {
    pub(super) fn new(owner: WeakEntity<Raag>, cx: &mut App) -> [Entity<Self>; 9] {
        std::array::from_fn(|index| {
            cx.new(|_| Self {
                owner: owner.clone(),
                part: PARTS[index],
                state: None,
            })
        })
    }
}
impl Render for ChromePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Ui);
        let view = cx.entity_id();
        self.owner
            .update(cx, |this, cx| {
                this.images.update(cx, |images, _| images.begin_view(view));
                match self.part {
                    ChromePart::Toolbar => this.toolbar(this.sidebar_expanded(window), window, cx),
                    ChromePart::Identity => this.bottom_identity(window, cx),
                    ChromePart::Transport => this.basic_controls("bottom", cx),
                    ChromePart::Actions => this.bottom_actions(window, cx),
                    ChromePart::ImmersiveArt => this.immersive_cover(window, cx),
                    ChromePart::ImmersiveBackground => this.immersive_background(cx),
                    ChromePart::ImmersiveTitle => this
                        .selected
                        .as_ref()
                        .map(|track| this.immersive_identity(track, cx))
                        .unwrap_or_else(|| div().into_any_element()),
                    ChromePart::ImmersiveButtons => this.immersive_controls(cx),
                    ChromePart::ImmersiveLyrics => this.immersive_lyric_list(cx),
                }
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}
impl Raag {
    // These views are siblings of progress, never a cached parent containing
    // progress. GPUI dirties ancestors, so caching the whole player would still
    // rebuild all its controls on every time-pos event.
    pub(super) fn chrome_view(&self, part: ChromePart, style: StyleRefinement) -> AnyElement {
        self.chrome[part as usize]
            .clone()
            .cached(style.flex_shrink_0())
            .into_any_element()
    }
    pub(super) fn refresh_chrome(&self, window: &mut Window, cx: &mut Context<Self>) {
        let generation = self.player.generation();
        let compact = f32::from(window.bounds().size.width) / self.library.ui_scale < 1100.;
        for (index, part) in PARTS.iter().enumerate() {
            let immersive_part = index >= ChromePart::ImmersiveArt as usize;
            if immersive_part != self.immersive
                || (index == ChromePart::ImmersiveLyrics as usize && !self.immersive_lyrics)
                || (index > ChromePart::Toolbar as usize
                    && !immersive_part
                    && self.selected.is_none())
            {
                self.images.update(cx, |cache, _| {
                    cache.release_view(self.chrome[index].entity_id(), window);
                });
                continue;
            }
            let state = match part {
                ChromePart::Toolbar => ChromeState::Toolbar {
                    auth: self.auth_generation,
                    artwork: self.artwork_revision,
                    suggestions: (self.suggestion_revision, self.suggestions.len()),
                    expanded: self.sidebar_expanded(window),
                    focused: self.query.read(cx).focus_handle(cx).is_focused(window),
                    busy: (
                        self.auth.busy(),
                        self.auth.can_cancel(),
                        self.auth.can_sign_out(),
                    ),
                    syncing: self.loading || self.account.syncing(),
                    history: (self.back.is_empty(), self.forward.is_empty()),
                },
                ChromePart::Identity => ChromeState::Identity(
                    generation,
                    self.artwork_revision,
                    compact,
                    self.sidebar_visible,
                    f32::from(window.bounds().size.width) / self.library.ui_scale < 700.,
                ),
                ChromePart::Transport => ChromeState::Transport(
                    self.selected.is_some(),
                    self.playing,
                    self.buffering,
                    self.failed,
                    self.sidebar_visible,
                ),
                ChromePart::Actions => ChromeState::Actions(
                    generation,
                    (self.content_revision, self.navigation_revision),
                    self.library.volume.to_bits(),
                    self.queue.shuffle,
                    self.queue.repeat,
                    self.panel,
                    compact,
                    self.sidebar_visible,
                ),
                ChromePart::ImmersiveArt => ChromeState::Art(generation, self.artwork_revision),
                ChromePart::ImmersiveBackground => ChromeState::Background(
                    generation,
                    self.artwork_revision,
                    self.library.immersive_artwork,
                    self.immersive_palette(),
                ),
                ChromePart::ImmersiveTitle => ChromeState::Title(
                    generation,
                    (self.content_revision, self.navigation_revision),
                ),
                ChromePart::ImmersiveButtons => ChromeState::Buttons(
                    self.playing,
                    self.buffering,
                    self.failed,
                    self.queue.shuffle,
                    self.queue.repeat,
                ),
                ChromePart::ImmersiveLyrics => ChromeState::Lyrics(
                    generation,
                    self.lyrics_revision,
                    self.lyrics_loading,
                    self.lyrics_active,
                    self.lyrics_word,
                    self.lyrics_follow,
                ),
            };
            self.chrome[index].update(cx, |pane, cx| {
                if pane.state.as_ref() != Some(&state) {
                    pane.state = Some(state);
                    cx.notify();
                }
            });
        }
    }
}
