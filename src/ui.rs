use crate::{
    service::{Backend, Event, Request, fetch_artwork},
    theme::{self, icon},
};
use gpui_kit::base::Button as ContentButton;
use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    slider::{Slider, SliderEvent, SliderState, SliderValue},
    switch::Switch,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use raag::{
    ItemKind, Track,
    account::{Edit, Section as AccountSection},
    catalog::LyricLine,
    clock,
    i18n::t,
    library::{Library, data_dir},
    playback::{Player, PlayerCommand, PlayerEvent},
    queue::{Queue, Repeat},
    rotation, tr,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

// GPUI's unstyled button defaults to a one-em line box, which clips descenders
// inside truncated labels. All content buttons share readable text geometry.
fn ui_px(value: f32) -> Pixels {
    px(value * raag::i18n::scale())
}
fn content_button(id: impl Into<ElementId>) -> ContentButton {
    ContentButton::new(id).line_height(relative(1.4))
}
fn playback_depth(button: Button) -> Button {
    button
        .border_1()
        .border_color(rgb(0xffffff).opacity(0.16))
        .shadow(vec![
            BoxShadow::new(ui_px(0.), ui_px(2.), rgb(0x000000).opacity(0.24).into())
                .blur_radius(ui_px(4.)),
            BoxShadow::new(ui_px(0.), ui_px(1.), rgb(0xffffff).opacity(0.24).into()).inset(),
            BoxShadow::new(ui_px(0.), ui_px(-1.), rgb(0x000000).opacity(0.14).into()).inset(),
        ])
}
fn artwork_frame(cx: &App) -> Div {
    div()
        .relative()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().foreground.opacity(0.16))
        .shadow(vec![
            BoxShadow::new(ui_px(0.), ui_px(6.), rgb(0x000000).opacity(0.28).into())
                .blur_radius(ui_px(18.)),
            BoxShadow::new(ui_px(0.), ui_px(1.), rgb(0xffffff).opacity(0.08).into()).inset(),
        ])
        .overflow_hidden()
}
mod account;
mod appearance;
mod artists;
mod chrome;
mod collections;
mod discovery;
mod enhancements;
mod equalizer;
mod features;
mod home;
mod immersive;
mod panels;
mod recap;
use chrome::{ChromePane, ChromePart};
use enhancements::{DragPreview, PlaylistDrag, TrackDrag};

actions!(
    raag,
    [
        TogglePlayback,
        NextTrack,
        PreviousTrack,
        FocusSearch,
        ToggleSidebar,
        ShowQueue,
        ShowLyrics,
        GoHome,
        GoBack,
        CloseOverlay,
        Fullscreen,
        ImportFiles,
        ToggleImmersiveArtwork,
        ToggleImmersiveLyrics,
        ExitImmersive,
        OpenSettings,
        OpenRecap,
        OpenLocal,
        OpenOffline,
        ShowRecent,
        SignIn,
        SignOut,
        CancelSignIn,
        ZoomIn,
        ZoomOut,
        ResetZoom
    ]
);
pub fn bind_keys(cx: &mut App) {
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-="), ZoomIn, Some("Raag")),
        KeyBinding::new(&format!("{modifier}-+"), ZoomIn, Some("Raag")),
        KeyBinding::new(&format!("{modifier}--"), ZoomOut, Some("Raag")),
        KeyBinding::new(&format!("{modifier}-0"), ResetZoom, Some("Raag")),
        KeyBinding::new("space", TogglePlayback, Some("Raag && !Input")),
        KeyBinding::new(&format!("{modifier}-right"), NextTrack, Some("Raag")),
        KeyBinding::new(&format!("{modifier}-left"), PreviousTrack, Some("Raag")),
        KeyBinding::new(&format!("{modifier}-k"), FocusSearch, Some("Raag")),
        KeyBinding::new(&format!("{modifier}-b"), ToggleSidebar, Some("Raag")),
        KeyBinding::new(&format!("{modifier}-q"), ShowQueue, Some("Raag")),
        KeyBinding::new(&format!("{modifier}-l"), ShowLyrics, Some("Raag")),
        KeyBinding::new(&format!("{modifier}-h"), GoHome, Some("Raag")),
        KeyBinding::new(&format!("{modifier}-o"), ImportFiles, Some("Raag")),
        KeyBinding::new("alt-left", GoBack, Some("Raag")),
        KeyBinding::new("escape", CloseOverlay, Some("Raag")),
        KeyBinding::new("f11", Fullscreen, Some("Raag")),
    ]);
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Screen {
    Home,
    Explore,
    Library,
    Liked,
    Recents,
    Local,
    Search,
    Collection(String, String, bool),
    Discover(String, String, String),
    Radio(String),
    PlaylistPicker,
    Queue,
    Lyrics,
    Settings,
    Offline,
    Recap,
}
impl Screen {
    fn title(&self) -> &str {
        match self {
            Self::Home => t("Home"),
            Self::Explore => t("Explore"),
            Self::Library => t("Your library"),
            Self::Liked => t("Liked songs"),
            Self::Recents => t("Recently played"),
            Self::Local => t("Local files"),
            Self::Search => t("Search"),
            Self::Collection(_, title, _) => title,
            Self::Discover(_, _, title) => t(title),
            Self::Radio(_) => t("Song radio"),
            Self::PlaylistPicker => t("Add to playlist"),
            Self::Queue => t("Next up"),
            Self::Lyrics => t("Lyrics"),
            Self::Settings => t("Settings"),
            Self::Offline => t("Offline"),
            Self::Recap => t("Your rhythm"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Panel {
    Playing,
    Recent,
    Queue,
    Lyrics,
}
struct ContentPane {
    owner: WeakEntity<Raag>,
    revision: u64,
}
impl Render for ContentPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Ui);
        let view = cx.entity_id();
        self.owner
            .update(cx, |this, cx| {
                this.images.update(cx, |cache, _| cache.begin_view(view));
                this.main_content(window, cx)
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}
struct RightPane {
    owner: WeakEntity<Raag>,
}
// Playback clocks have their own invalidation boundary. They do not rebuild
// page artwork, queue rows or sidebar controls on every time-pos event.
type ProgressSnapshot = (bool, bool, bool, bool, f64, Option<f64>, f64);
struct ProgressPane {
    owner: WeakEntity<Raag>,
    mini: bool,
    snapshot: Option<ProgressSnapshot>,
}
impl Render for ProgressPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner
            .update(cx, |this, cx| {
                if self.mini {
                    this.progress_content(cx)
                } else {
                    this.bottom_progress(cx)
                }
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}
impl Render for RightPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Ui);
        let view = cx.entity_id();
        self.owner
            .update(cx, |this, cx| {
                this.images.update(cx, |cache, _| cache.begin_view(view));
                this.right_panel(window, cx)
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}
struct NavigationPane {
    owner: WeakEntity<Raag>,
    revision: u64,
    screen: Option<Screen>,
    expanded: bool,
}
impl Render for NavigationPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Ui);
        let view = cx.entity_id();
        self.owner
            .update(cx, |this, cx| {
                this.images.update(cx, |cache, _| cache.begin_view(view));
                this.sidebar(self.expanded, cx)
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}
pub struct Raag {
    screen: Screen,
    content: Entity<ContentPane>,
    right_content: Entity<RightPane>,
    playback_progress: Entity<ProgressPane>,
    mini_progress: Entity<ProgressPane>,
    navigation: Entity<NavigationPane>,
    chrome: [Entity<ChromePane>; 9],
    artwork_revision: u64,
    navigation_revision: u64,
    content_revision: u64,
    panel: Option<Panel>,
    // Retain the outgoing pane while its shell slot contracts.
    last_panel: Option<Panel>,
    shell_motion: crate::motion::ShellMotion,
    shell_frame_pending: bool,
    panel_scroll: crate::motion::SmoothScroll,
    sidebar_scroll: crate::motion::SmoothScroll,
    lyrics_scroll: crate::motion::SmoothScroll,
    lyrics_active: Option<usize>,
    lyrics_word: Option<usize>,
    lyrics_geometry: Option<(f32, f32)>,
    lyrics_follow: bool,
    offline: Arc<Vec<Track>>,
    cache_notice: Option<String>,
    stream_quality: raag::playback::StreamQuality,
    source_offline: bool,
    listening: raag::stats::ListeningSession,
    private_session: bool,
    stats_saved: Instant,
    back: Vec<Screen>,
    forward: Vec<Screen>,
    focus: FocusHandle,
    seek_focus: FocusHandle,
    mini_seek_focus: FocusHandle,
    eq_focus: FocusHandle,
    eq_band: usize,
    eq_drag: Option<usize>,
    query: Entity<InputState>,
    browse_query: Entity<InputState>,
    browse_filter: String,
    browse_sort: usize,
    selecting: bool,
    selected_rows: std::collections::HashSet<String>,
    picker_tracks: Vec<Track>,
    pending_edits: std::collections::VecDeque<Edit>,
    queue_epoch: u64,
    recap_period: usize,
    organization_name: Entity<InputState>,
    smart_name: Entity<InputState>,
    smart_query: Entity<InputState>,
    smart_artist: Entity<InputState>,
    smart_album: Entity<InputState>,
    smart_plays: Entity<InputState>,
    editing_smart: Option<String>,
    smart_source: usize,
    smart_exclude_explicit: bool,
    playlist_name: Entity<InputState>,
    settings_category: usize,
    folder_editor: bool,
    folder_rename: bool,
    ytdlp_path: Entity<InputState>,
    timeline: Entity<SliderState>,
    mini_timeline: Entity<SliderState>,
    volume: Entity<SliderState>,
    volume_scroll: crate::motion::VolumeScroll,
    library: Library,
    sidebar_visible: bool,
    system: Option<crate::system::System>,
    media: Option<crate::media::Media>,
    downloads: crate::downloads::Downloads,
    download_status: crate::downloads::Status,
    instance: crate::system::Instance,
    startup_busy: bool,
    advanced_settings: bool,
    writable: bool,
    queue: Queue,
    player: Player,
    backend: Backend,
    auth: crate::auth_controller::AuthController,
    auth_generation: u64,
    sync_on_open: bool,
    account: crate::account_sync::AccountSync,
    library_section: AccountSection,
    playlist_editor_remote: bool,
    editorial: Arc<Vec<Track>>,
    catalog: Arc<Vec<Track>>,
    visible: Arc<Vec<Track>>,
    artwork: HashMap<String, PathBuf>,
    artwork_variants: HashMap<PathBuf, u8>,
    artwork_arrived: HashMap<String, Instant>,
    artwork_colors: HashMap<String, raag::artwork::ArtworkColors>,
    shelf_positions: HashMap<String, usize>,
    prefetched_next: Option<(u64, String)>,
    radio_pending: Option<u64>,
    radio_attempted: Option<u64>,
    prepared_id: Option<String>,
    audio_devices: Vec<(String, String)>,
    radio_waiting: bool,
    radio_mix: bool,
    resume_at: Option<f64>,
    sleep_until: Option<Instant>,
    sleep_end: bool,
    immersive: bool,
    immersive_lyrics: bool,
    immersive_scroll: crate::motion::SmoothScroll,
    revision: u64,
    loading: bool,
    continuation: Option<String>,
    pagination_attempts: std::collections::HashSet<(Screen, AccountSection, String)>,
    search_filter: usize,
    suggestion_revision: u64,
    suggestion_pending: Option<(String, Instant)>,
    suggestions: Vec<String>,
    search_query: String,
    selected: Option<Track>,
    images: Entity<crate::images::ResidentImages>,
    missing_artwork: std::rc::Rc<std::cell::RefCell<crate::artwork_queue::PendingArtwork>>,
    artwork_attempted: std::rc::Rc<std::cell::RefCell<HashMap<String, Instant>>>,
    collection_reveal_at: Instant,
    previous_track: Option<Track>,
    track_changed_at: Instant,
    playing: bool,
    buffering: bool,
    failed: bool,
    position: f64,
    duration: f64,
    muted_volume: f64,
    notice: Option<String>,
    notice_seen: Option<String>,
    notice_since: Instant,
    show_all_themes: bool,
    lyrics: Vec<LyricLine>,
    lyrics_id: String,
    lyrics_loading: bool,
    lyrics_revision: u64,
    lyrics_error: Option<String>,
    editing_playlist: bool,
    editing_description: bool,
    page: raag::catalog::Page,
    seek_preview: Option<f64>,
    seek_pending: Option<(f64, Instant)>,
    scroll: crate::motion::SmoothScroll,
    collection_seed: Option<Track>,
    _subscriptions: Vec<Subscription>,
    _pump: Task<()>,
}

impl Raag {
    pub fn new(
        window: &mut Window,
        instance: crate::system::Instance,
        cx: &mut Context<Self>,
    ) -> Self {
        let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Ui);
        let (mut library, writable, notice) = match Library::load(&data_dir().join("library.json"))
        {
            Ok(library) => (library, true, None),
            Err(error) => (Library::default(), false, Some(error.to_string())),
        };
        raag::i18n::set_language(&library.language);
        let system = crate::system::System::new().ok();
        let media = crate::media::Media::new(window);
        raag::cache::configure(library.audio_budget, library.offline_only);
        if std::env::args_os().any(|arg| arg == "--background") {
            if system.is_some() && crate::system::hide_supported(window) {
                crate::system::hide(window, cx);
            } else {
                crate::system::show(window, cx);
            }
        }
        let owner = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            owner
                .update(cx, |this, cx| this.should_close(window, cx))
                .unwrap_or(true)
        });
        raag::i18n::set_language(&library.language);
        raag::i18n::set_scale(library.ui_scale);
        gpui_kit::component::set_locale(&library.language);
        window.set_rem_size(px(16. * library.ui_scale));
        theme::apply(&library, None, cx);
        let owner = cx.weak_entity();
        let content = cx.new(|_| ContentPane {
            owner: owner.clone(),
            revision: 0,
        });
        let navigation = cx.new(|_| NavigationPane {
            owner: owner.clone(),
            revision: 0,
            screen: None,
            expanded: library.sidebar,
        });
        let playback_progress = cx.new(|_| ProgressPane {
            owner: owner.clone(),
            mini: false,
            snapshot: None,
        });
        let mini_progress = cx.new(|_| ProgressPane {
            owner: owner.clone(),
            mini: true,
            snapshot: None,
        });
        let chrome = ChromePane::new(owner.clone(), cx);
        let right_content = cx.new(|_| RightPane { owner });
        let focus = cx.focus_handle();
        let seek_focus = cx.focus_handle().tab_stop(true);
        let mini_seek_focus = cx.focus_handle().tab_stop(true);
        let eq_focus = cx.focus_handle().tab_stop(true);
        window.focus(&focus, cx);
        let query = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("Search songs, artists, albums…")));
        let playlist_name =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("Playlist name")));
        let organization_name =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("Folder name")));
        let smart_name =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("Smart playlist name")));
        let smart_query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("Song title, artist or album")));
        let smart_artist = cx.new(|cx| InputState::new(window, cx).placeholder(t("Any artist")));
        let smart_album = cx.new(|cx| InputState::new(window, cx).placeholder(t("Any album")));
        let smart_plays = cx.new(|cx| InputState::new(window, cx).placeholder("0"));
        let browse_query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("Filter this collection…")));
        let browse_subscription = cx.subscribe(&browse_query, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.browse_filter = input.read(cx).value().to_lowercase();
                this.scroll.reset();
                this.refresh_visible();
                cx.notify();
            }
        });
        let ytdlp_path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("Automatic (yt-dlp on PATH or bundled)"))
                .default_value(library.ytdlp_path.clone())
        });
        let timeline = cx.new(|_| SliderState::new().step(0.1));
        let mini_timeline = cx.new(|_| SliderState::new().step(0.1));
        let volume = cx.new(|_| SliderState::new().default_value(library.volume as f32));
        let player = Player::with_cookies(library.ytdlp_path.clone(), library.cookies_path.clone());
        player.command(PlayerCommand::Configure(library.audio.clone()));
        let restored_queue = std::mem::take(&mut library.session.queue);
        let restored_track = restored_queue.current().cloned();
        let restored_position = library.session.position;
        let auth = crate::auth_controller::AuthController::new();
        let backend = Backend::new(auth.access());
        let downloads = crate::downloads::Downloads::new(backend.events_sender.clone());
        let account = crate::account_sync::AccountSync::new(auth.access());
        let _ = backend.requests.send(Request::Home(1));
        let editorial = Arc::new(rotation());
        // Startup warms the first screen, not the entire lifetime library.
        let mut saved_art = editorial.as_ref().clone();
        saved_art.extend(library.history.iter().take(9).cloned());
        saved_art.extend(library.liked.iter().take(32).cloned());
        saved_art.extend(
            library
                .playlists
                .iter()
                .take(32)
                .filter_map(|p| p.tracks.first())
                .cloned(),
        );
        saved_art.extend(
            library
                .stats
                .top_tracks()
                .into_iter()
                .take(5)
                .map(|entry| entry.track.clone()),
        );
        fetch_artwork(saved_art, backend.events_sender.clone());
        let search_subscription =
            cx.subscribe_in(&query, window, |this, input, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.suggestion_revision = this.suggestion_revision.wrapping_add(1);
                    this.suggestions.clear();
                    let query = input.read(cx).value().to_string();
                    this.suggestion_pending = (query.trim().chars().count() >= 2
                        && query.len() <= 512
                        && !query.starts_with("https://")
                        && !query.starts_with("cassette://")
                        && !query.starts_with("spotify:"))
                    .then(|| (query, Instant::now() + Duration::from_millis(350)));
                    cx.notify();
                }
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let query = input.read(cx).value().to_string();
                    if query.trim().is_empty() {
                        return;
                    }
                    if query.starts_with("https://")
                        || query.starts_with("cassette://")
                        || query.starts_with("spotify:")
                    {
                        match raag::links::Link::parse(query.trim()) {
                            Ok(link) => this.open_link(link, cx),
                            Err(error) => this.notice = Some(error.to_string()),
                        };
                        this.suggestions.clear();
                        window.focus(&this.focus, cx);
                        cx.notify();
                        return;
                    }
                    this.suggestion_pending = None;
                    this.suggestions.clear();
                    this.suggestion_revision = this.suggestion_revision.wrapping_add(1);
                    this.navigate(Screen::Search, cx);
                    this.search_query = query.trim().to_owned();
                    this.pagination_attempts.clear();
                    this.search_filter = 0;
                    this.catalog = Arc::new(vec![]);
                    this.page = raag::catalog::Page::default();
                    this.scroll.reset();
                    this.refresh_visible();
                    this.request(|id| Request::Search(id, query, None), cx);
                    window.focus(&this.focus, cx);
                }
            });
        let seek_subscription = cx.subscribe_in(&timeline, window, |this, _, event, _, cx| {
            if this.duration <= 0. || this.buffering {
                return;
            }
            match event {
                SliderEvent::Change(SliderValue::Single(value)) => {
                    this.seek_preview =
                        Some((f64::from(*value) / 100. * this.duration).clamp(0., this.duration));
                    cx.notify();
                }
                SliderEvent::Release(SliderValue::Single(value)) => {
                    this.commit_seek(f64::from(*value) / 100. * this.duration, cx);
                }
                _ => {}
            }
        });
        let mini_seek_subscription =
            cx.subscribe_in(&mini_timeline, window, |this, _, event, _, cx| {
                if this.duration <= 0. || this.buffering {
                    return;
                }
                match event {
                    SliderEvent::Change(SliderValue::Single(value)) => {
                        this.seek_preview = Some(
                            (f64::from(*value) / 100. * this.duration).clamp(0., this.duration),
                        );
                        cx.notify();
                    }
                    SliderEvent::Release(SliderValue::Single(value)) => {
                        this.commit_seek(f64::from(*value) / 100. * this.duration, cx);
                    }
                    _ => {}
                }
            });
        let volume_subscription = cx.subscribe_in(&volume, window, |this, _, event, _, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                this.library.volume = f64::from(*value);
                if this.screen == Screen::Settings {
                    this.content_revision += 1;
                }
                this.player
                    .command(PlayerCommand::Volume(this.library.volume));
                cx.notify();
            }
            if matches!(event, SliderEvent::Release(_)) {
                this.save();
            }
        });
        let pump = cx.spawn_in(window, async move |view, cx| {
            let mut delay = Duration::from_millis(33);
            loop {
                cx.background_executor().timer(delay).await;
                match view.update_in(cx, |this, window, cx| {
                    this.pump(window, cx);
                    Duration::from_millis(
                        if (this.buffering || this.loading || this.lyrics_loading)
                            && window.is_window_active()
                        {
                            33
                        } else if this.playing && window.is_window_active() {
                            80
                        } else if this.playing
                            || this.buffering
                            || this.loading
                            || this.lyrics_loading
                        {
                            250
                        } else if window.is_window_active() {
                            150
                        } else {
                            350
                        },
                    )
                }) {
                    Ok(next) => delay = next,
                    Err(_) => break,
                }
            }
        });
        Self {
            screen: Screen::Home,
            content,
            right_content,
            playback_progress,
            mini_progress,
            chrome,
            artwork_revision: 0,
            content_revision: 1,
            panel: None,
            last_panel: None,
            shell_motion: crate::motion::ShellMotion::new(library.sidebar),
            sidebar_visible: library.sidebar,
            shell_frame_pending: false,
            panel_scroll: crate::motion::SmoothScroll::new(),
            sidebar_scroll: crate::motion::SmoothScroll::new(),
            lyrics_scroll: crate::motion::SmoothScroll::new(),
            lyrics_active: None,
            lyrics_word: None,
            lyrics_geometry: None,
            lyrics_follow: true,
            offline: Arc::new(vec![]),
            cache_notice: None,
            stream_quality: raag::playback::StreamQuality::default(),
            source_offline: false,
            listening: raag::stats::ListeningSession::default(),
            private_session: false,
            stats_saved: Instant::now(),
            back: vec![],
            forward: vec![],
            focus,
            seek_focus,
            mini_seek_focus,
            eq_focus,
            eq_band: 2,
            eq_drag: None,
            query,
            browse_query,
            browse_filter: String::new(),
            browse_sort: 0,
            selecting: false,
            selected_rows: Default::default(),
            picker_tracks: vec![],
            pending_edits: Default::default(),
            queue_epoch: 0,
            recap_period: 0,
            organization_name,
            smart_name,
            smart_query,
            smart_artist,
            smart_album,
            smart_plays,
            editing_smart: None,
            smart_source: 0,
            smart_exclude_explicit: false,
            playlist_name,
            settings_category: 0,
            folder_editor: false,
            folder_rename: false,
            ytdlp_path,
            timeline,
            mini_timeline,
            volume,
            volume_scroll: Default::default(),
            library,
            system,
            media,
            downloads,
            download_status: Default::default(),
            instance,
            startup_busy: false,
            advanced_settings: false,
            writable,
            queue: restored_queue,
            player,
            backend,
            auth,
            auth_generation: 0,
            sync_on_open: false,
            account,
            library_section: AccountSection::Playlists,
            playlist_editor_remote: false,
            editorial: editorial.clone(),
            catalog: Arc::new(vec![]),
            visible: editorial,
            artwork: HashMap::new(),
            artwork_variants: HashMap::new(),
            artwork_arrived: HashMap::new(),
            artwork_colors: HashMap::new(),
            shelf_positions: HashMap::new(),
            prefetched_next: None,
            radio_pending: None,
            radio_attempted: None,
            prepared_id: None,
            audio_devices: vec![],
            radio_waiting: false,
            radio_mix: false,
            resume_at: restored_track.as_ref().map(|_| restored_position),
            sleep_until: None,
            sleep_end: false,
            immersive: false,
            immersive_lyrics: true,
            immersive_scroll: crate::motion::SmoothScroll::new(),
            revision: 1,
            loading: true,
            continuation: None,
            pagination_attempts: Default::default(),
            search_filter: 0,
            search_query: String::new(),
            suggestion_revision: 0,
            suggestion_pending: None,
            suggestions: vec![],
            selected: restored_track.clone(),
            images: crate::images::ResidentImages::new(cx),
            missing_artwork: Default::default(),
            artwork_attempted: Default::default(),
            collection_reveal_at: Instant::now(),
            previous_track: None,
            track_changed_at: Instant::now(),
            playing: false,
            buffering: false,
            failed: restored_track.is_some(),
            position: restored_position,
            duration: restored_track.map_or(0., |t| t.duration as f64),
            muted_volume: 70.,
            notice,
            notice_seen: None,
            notice_since: Instant::now(),
            show_all_themes: false,
            navigation,
            navigation_revision: 0,
            lyrics: vec![],
            lyrics_id: String::new(),
            lyrics_loading: false,
            lyrics_revision: 0,
            lyrics_error: None,
            editing_playlist: false,
            editing_description: false,
            page: raag::catalog::Page::default(),
            seek_preview: None,
            seek_pending: None,
            scroll: crate::motion::SmoothScroll::new(),
            collection_seed: None,
            _subscriptions: vec![
                search_subscription,
                browse_subscription,
                seek_subscription,
                mini_seek_subscription,
                volume_subscription,
            ],
            _pump: pump,
        }
    }
    fn main_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.screen {
            Screen::Home => self.home(window, cx),
            Screen::Explore | Screen::Discover(..) => self.explore_screen(window, cx),
            Screen::Search => self.search_screen(window, cx),
            Screen::Collection(..) => self.collection_screen(window, cx),
            Screen::Library => self.library_screen(window, cx),
            Screen::Lyrics => self.lyrics_screen(cx),
            Screen::Settings => self.settings(window, cx),
            Screen::Recap => self.recap_screen(window, cx),
            Screen::PlaylistPicker => self.playlist_picker(cx),
            _ => self.listing_screen(cx),
        }
    }
    fn save(&self) {
        if self.writable {
            self.backend.save(self.snapshot());
        }
    }
    fn snapshot(&self) -> Library {
        let mut library = self.library.clone();
        library.session = if self.private_session {
            Default::default()
        } else {
            raag::queue::Session {
                queue: self.queue.clone(),
                position: self.position,
            }
        };
        library
    }
    fn request(&mut self, request: impl FnOnce(u64) -> Request, cx: &mut Context<Self>) {
        self.revision += 1;
        self.loading = true;
        self.content_revision += 1;
        self.notice = None;
        let _ = self.backend.requests.send(request(self.revision));
        cx.notify();
    }
    fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notice != self.notice_seen {
            self.notice_seen.clone_from(&self.notice);
            self.notice_since = Instant::now();
        } else if self.writable
            && !self.auth.busy()
            && self.notice.is_some()
            && self.notice_since.elapsed() >= Duration::from_secs(12)
        {
            self.notice = None;
            self.notice_seen = None;
            cx.notify();
        }
        if self
            .suggestion_pending
            .as_ref()
            .is_some_and(|(_, deadline)| Instant::now() >= *deadline)
        {
            let (query, _) = self.suggestion_pending.take().unwrap();
            if self.library.offline_only {
                let query = query.to_lowercase();
                let mut seen = std::collections::HashSet::new();
                self.suggestions = self
                    .library
                    .local
                    .iter()
                    .chain(&self.library.liked)
                    .chain(&self.library.history)
                    .filter(|t| {
                        t.title.to_lowercase().contains(&query) && seen.insert(t.title.clone())
                    })
                    .take(8)
                    .map(|t| t.title.clone())
                    .collect();
                cx.notify();
            } else {
                let _ = self
                    .backend
                    .suggestions
                    .send((self.suggestion_revision, query));
            }
        }
        if !self.account.busy_edits()
            && let Some(edit) = self.pending_edits.pop_front()
        {
            self.account_edit(edit, cx);
        }
        self.pump_media(window, cx);
        while let Ok(status) = self.downloads.events.try_recv() {
            self.download_status = status;
            if !self.download_status.active {
                let sender = self.backend.events_sender.clone();
                cx.background_spawn(async move {
                    let _ = sender.send(Event::Offline(raag::cache::offline_tracks()));
                })
                .detach();
            }
            self.content_revision += 1;
            cx.notify();
        }
        if self
            .sleep_until
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.sleep_until = None;
            self.sleep_end = false;
            self.player.command(PlayerCommand::Pause(true));
            self.playing = false;
            self.content_revision += 1;
            cx.notify();
        }
        let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Ui);
        while let Some(command) = self
            .instance
            .poll()
            .or_else(|| self.system.as_ref().and_then(crate::system::System::poll))
        {
            match command {
                crate::system::Command::Show => {
                    crate::system::show(window, cx);
                    self.sync_on_open = true;
                }
                crate::system::Command::TogglePlayback => self.toggle_playback(cx),
                crate::system::Command::Quit => {
                    self.backend.flush(self.snapshot());
                    cx.quit();
                    return;
                }
                crate::system::Command::Open(link) => {
                    crate::system::show(window, cx);
                    self.open_link(link, cx);
                }
            }
        }
        let mut changed = false;
        let mut panel_changed = false;
        while let Some(event) = self.auth.poll() {
            match event {
                crate::auth_controller::AuthEvent::Changed(generation, notice)
                    if generation == self.auth.snapshot().generation =>
                {
                    self.notice = notice;
                }
                crate::auth_controller::AuthEvent::Finished(_, Err(message)) => {
                    self.notice = Some(message)
                }
                _ => {}
            }
            self.content_revision += 1;
            changed = true;
        }
        let auth_snapshot = self.auth.snapshot();
        if auth_snapshot.generation != self.auth_generation {
            let auth_notice = self.notice.take();
            self.auth_generation = auth_snapshot.generation;
            self.navigation_revision = self.navigation_revision.wrapping_add(1);
            self.invalidate_collection();
            self.backend
                .download_epoch
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.downloads.cancel();
            self.account.reset();
            self.pending_edits.clear();
            self.playlist_editor_remote = false;
            if auth_snapshot.session.is_some() {
                // Session restoration/sign-in performs the full startup sync.
                self.sync_on_open = false;
                for section in AccountSection::ALL {
                    self.account.read(
                        auth_snapshot.clone(),
                        crate::account_sync::Key::Section(section),
                        false,
                    );
                }
            }
            raag::playback::clear_source_cache();
            self.prefetched_next = None;
            self.reset_player(cx);
            self.catalog = Arc::new(vec![]);
            self.page = Default::default();
            self.collection_seed = None;
            self.screen = Screen::Home;
            self.back.clear();
            self.forward.clear();
            self.continuation = None;
            self.pagination_attempts.clear();
            self.editing_playlist = false;
            self.scroll.reset();
            self.shelf_positions.clear();
            self.refresh_visible();
            // Always request, including when Home was already visible. This also rejects queued old-account Page events.
            self.request(Request::Home, cx);
            self.notice = auth_notice;
            panel_changed = true;
            changed = true;
        }
        for _ in 0..8 {
            let Some(update) = self.account.poll(&self.auth.access()) else {
                break;
            };
            self.account_update(update, cx);
            changed = true;
            panel_changed = true;
        }
        let needed = self.missing_artwork.borrow_mut().drain(48);
        if !needed.is_empty() {
            crate::service::fetch_visible_artwork(needed, self.backend.events_sender.clone());
        }
        // Cached page paint does not revisit visibility callbacks on its own.
        // Repaint expired attempts so dropped/failed work gets another chance.
        let retry_artwork = {
            let mut attempted = self.artwork_attempted.borrow_mut();
            let before = attempted.len();
            attempted.retain(|_, at| at.elapsed() < Duration::from_secs(15));
            attempted.len() < before
        };
        if retry_artwork {
            self.content_revision += 1;
            self.artwork_revision = self.artwork_revision.wrapping_add(1);
            self.navigation_revision = self.navigation_revision.wrapping_add(1);
            panel_changed = true;
            changed = true;
        }
        // A burst of artwork/cache results must not monopolize a UI frame.
        let batch_started = Instant::now();
        for _ in 0..32 {
            if batch_started.elapsed() >= Duration::from_millis(4) {
                break;
            }
            let Ok(event) = self.backend.events.try_recv() else {
                break;
            };
            match event {
                Event::Suggestions(revision, suggestions)
                    if revision == self.suggestion_revision =>
                {
                    self.suggestions = suggestions;
                    changed = true;
                }
                Event::Page(id, mut page, append) if id == self.revision => {
                    if self.catalog.is_empty() && !append {
                        self.collection_reveal_at = Instant::now();
                    }
                    self.loading = false;
                    self.continuation = page.continuation.clone();
                    if append {
                        for section in page.sections.drain(..) {
                            if let Some(existing) = self
                                .page
                                .sections
                                .iter_mut()
                                .find(|existing| existing.title == section.title)
                            {
                                let mut seen: std::collections::HashSet<_> =
                                    existing.tracks.iter().map(Self::row_key).collect();
                                existing.tracks.extend(
                                    section
                                        .tracks
                                        .into_iter()
                                        .filter(|track| seen.insert(Self::row_key(track))),
                                );
                            } else {
                                self.page.sections.push(section);
                            }
                        }
                        let tracks = Arc::make_mut(&mut self.catalog);
                        let mut seen: std::collections::HashSet<_> =
                            tracks.iter().map(Self::row_key).collect();
                        tracks.extend(
                            page.tracks
                                .into_iter()
                                .filter(|track| seen.insert(Self::row_key(track))),
                        );
                        self.page.visitor_data =
                            page.visitor_data.or(self.page.visitor_data.take());
                    } else {
                        self.catalog = Arc::new(std::mem::take(&mut page.tracks));
                        self.page = page;
                    }
                    self.page.continuation = self.continuation.clone();
                    self.refresh_visible();
                    changed = true;
                }
                Event::Failure(id, message) if id == self.revision => {
                    self.loading = false;
                    self.content_revision += 1;
                    self.notice = Some(message);
                    changed = true;
                }
                Event::Artwork(images, variants) => {
                    let variants_changed = variants.iter().any(|(path, mask)| {
                        self.artwork_variants
                            .get(path)
                            .is_some_and(|previous| previous != mask)
                    });
                    self.artwork_variants.extend(variants);
                    if variants_changed {
                        self.content_revision += 1;
                        panel_changed = true;
                        changed = true;
                    }
                    let mut added = false;
                    // Keep the original reveal time when an existing item receives
                    // a new URL/alias. Refresh must never fade back to a placeholder.
                    let prior_arrivals: HashMap<_, _> = images
                        .iter()
                        .filter(|(id, _)| self.artwork.contains_key(*id))
                        .map(|(id, path)| {
                            (
                                path.clone(),
                                self.artwork_arrived
                                    .get(id)
                                    .copied()
                                    .unwrap_or_else(|| Instant::now() - Duration::from_secs(1)),
                            )
                        })
                        .collect();
                    let mut page_changed = false;
                    let collection_cover = matches!(self.screen, Screen::Collection(..))
                        .then(|| self.collection_art());

                    for (id, path) in images {
                        self.artwork_attempted.borrow_mut().remove(&id);
                        if self.artwork.get(&id) != Some(&path) {
                            let selected = self
                                .selected
                                .as_ref()
                                .is_some_and(|track| track.artwork_key() == id);
                            panel_changed |= match self.panel {
                                Some(Panel::Queue) => {
                                    self.queue.tracks().any(|track| track.artwork_key() == id)
                                }
                                Some(Panel::Recent) => self
                                    .library
                                    .history
                                    .iter()
                                    .any(|track| track.artwork_key() == id),
                                Some(Panel::Playing | Panel::Lyrics) => selected,
                                None => false,
                            };
                            page_changed |= collection_cover
                                .as_ref()
                                .is_some_and(|cover| cover.artwork_key() == id || cover.id == id)
                                || (self.screen == Screen::Home
                                    && self
                                        .library
                                        .history
                                        .iter()
                                        .take(9)
                                        .any(|track| track.artwork_key() == id))
                                || self.visible.iter().any(|track| track.artwork_key() == id)
                                || self
                                    .collection_seed
                                    .as_ref()
                                    .is_some_and(|track| track.artwork_key() == id)
                                || self
                                    .page
                                    .artwork
                                    .as_ref()
                                    .is_some_and(|url| id == raag::artwork::identity(url))
                                || self.screen == Screen::Recap
                                || (self.screen == Screen::Library
                                    && self.library.playlists.iter().any(|playlist| {
                                        playlist
                                            .tracks
                                            .first()
                                            .is_some_and(|track| track.artwork_key() == id)
                                    }));
                            self.artwork_arrived.entry(id.clone()).or_insert_with(|| {
                                prior_arrivals
                                    .get(&path)
                                    .copied()
                                    .unwrap_or_else(Instant::now)
                            });
                            self.artwork.insert(id, path);
                            added = true;
                        }
                    }
                    if added {
                        self.navigation_revision = self.navigation_revision.wrapping_add(1);
                        self.trim_artwork();
                        if page_changed {
                            self.content_revision += 1;
                        }
                        changed = true; // Sidebar art also depends on this shared cache.
                    }
                    if added || variants_changed {
                        if variants_changed && !added {
                            self.navigation_revision = self.navigation_revision.wrapping_add(1);
                        }
                        self.artwork_revision = self.artwork_revision.wrapping_add(1);
                        let paths: std::collections::HashSet<_> = self.artwork.values().collect();
                        self.artwork_variants.retain(|path, _| paths.contains(path));
                    }
                }
                Event::ArtworkColors(colors) => {
                    let selected_changed = self.selected.as_ref().is_some_and(|track| {
                        colors
                            .get(track.artwork_key())
                            .or_else(|| colors.get(&track.id))
                            .is_some_and(|colors| {
                                self.artwork_colors
                                    .get(track.artwork_key())
                                    .or_else(|| self.artwork_colors.get(&track.id))
                                    != Some(colors)
                            })
                    });
                    self.artwork_colors.extend(colors);
                    if self.artwork_colors.len() > 1024 {
                        self.artwork_colors.retain(|id, _| {
                            self.artwork.contains_key(id)
                                || self
                                    .selected
                                    .as_ref()
                                    .is_some_and(|track| track.artwork_key() == id)
                        });
                    }
                    if selected_changed {
                        if self.library.magic_accent {
                            self.apply_appearance(cx);
                        }
                        panel_changed = true;
                        changed = true;
                    }
                }
                Event::Offline(tracks) => {
                    self.offline = Arc::new(tracks);
                    if self.screen == Screen::Offline {
                        self.refresh_visible();
                    }
                    changed = true;
                }
                Event::Radio(generation, result) => {
                    if self.radio_pending == Some(generation)
                        && generation == self.player.generation()
                    {
                        self.radio_pending = None;
                        if self.library.autoplay || self.radio_mix {
                            let mut known: std::collections::HashSet<_> =
                                self.queue.tracks().map(|t| t.id.clone()).collect();
                            if let Ok(tracks) = result {
                                for track in tracks
                                    .into_iter()
                                    .filter(|t| t.kind == ItemKind::Song)
                                    .take(30)
                                {
                                    let familiar =
                                        self.library.stats.tracks.contains_key(&track.id)
                                            || self.library.liked(&track.id);
                                    let allowed = match self.library.discovery_mode {
                                        1 => familiar,
                                        2 => !familiar,
                                        _ => true,
                                    };
                                    if allowed
                                        && !self.library.hidden.contains(&track.id)
                                        && known.insert(track.id.clone())
                                    {
                                        self.queue.enqueue(track, false);
                                    }
                                }
                            }
                            if self.radio_waiting {
                                self.radio_waiting = false;
                                self.next(false, cx);
                            } else {
                                self.prefetch_next();
                            }
                        }
                        changed = true;
                    }
                }
                Event::Collection(epoch, download, result) => {
                    match result {
                        Ok(tracks)
                            if download
                                && epoch
                                    == self
                                        .backend
                                        .download_epoch
                                        .load(std::sync::atomic::Ordering::Acquire) =>
                        {
                            self.download_tracks(tracks, cx)
                        }
                        Ok(tracks) if !download && epoch == self.queue_epoch => {
                            let mut known: std::collections::HashSet<_> =
                                self.queue.tracks().map(Self::row_key).collect();
                            for track in tracks {
                                if track.playable() && known.insert(Self::row_key(&track)) {
                                    self.queue.enqueue(track, false);
                                }
                            }
                            self.save();
                        }
                        Err(error)
                            if if download {
                                epoch
                                    == self
                                        .backend
                                        .download_epoch
                                        .load(std::sync::atomic::Ordering::Acquire)
                            } else {
                                epoch == self.queue_epoch
                            } =>
                        {
                            self.notice = Some(error)
                        }
                        _ => {}
                    }
                    changed = true;
                }
                Event::CacheStatus(message) => {
                    self.cache_notice = message;
                    panel_changed = true;
                    changed = true;
                }
                Event::Saved(Err(message)) => {
                    self.notice = Some(message);
                    changed = true;
                }
                Event::Imported(library, count) => {
                    for folder in library.local_folders {
                        if !self.library.local_folders.contains(&folder) {
                            self.library.local_folders.push(folder);
                        }
                    }
                    for track in library.local {
                        if let Some(existing) =
                            self.library.local.iter_mut().find(|t| t.id == track.id)
                        {
                            *existing = track;
                        } else {
                            self.library.local.push(track);
                        }
                    }
                    self.notice = Some(tr!("Added {count} local tracks"));
                    self.loading = false;
                    self.save();
                    self.refresh_visible();
                    changed = true;
                }
                Event::Lyrics(id, result) if id == self.lyrics_id => {
                    self.lyrics_revision = self.lyrics_revision.wrapping_add(1);
                    self.lyrics_loading = false;
                    self.lyrics_active = None;
                    self.lyrics_geometry = None;
                    panel_changed = true;
                    match result {
                        Ok(lyrics) => {
                            self.lyrics = lyrics.lines;
                            self.lyrics_error = lyrics
                                .instrumental
                                .then(|| t("Lyrics aren’t available for this recording.").into());
                        }
                        Err(message) => {
                            self.lyrics.clear();
                            self.lyrics_error = Some(message);
                        }
                    }
                    changed = true;
                }
                _ => {}
            }
        }
        let mut clock_changed = false;
        // A stalled frame must not turn a backlog of IPC events into another
        // unbounded foreground task. Clock widgets update once after the batch.
        for _ in 0..128 {
            let Ok(event) = self.player.events.try_recv() else {
                break;
            };
            if event.generation != self.player.generation() {
                continue;
            }
            match event.event {
                PlayerEvent::Source(quality, offline) => {
                    self.stream_quality = quality;
                    self.source_offline = offline;
                    panel_changed = true;
                    changed = true;
                }
                PlayerEvent::EngineReady => {}
                PlayerEvent::Buffering(value) => {
                    self.buffering = value;
                    panel_changed = true;
                    changed = true;
                }
                PlayerEvent::Prepared(id) => {
                    self.prepared_id = Some(id);
                }
                PlayerEvent::Devices(devices) => {
                    self.audio_devices = devices;
                    if self.screen == Screen::Settings {
                        self.content_revision += 1;
                        changed = true;
                    }
                }
                PlayerEvent::Loaded => {
                    self.buffering = false;
                    self.playing = true;
                    if let Some(position) = self.resume_at.take().filter(|p| *p > 0.) {
                        self.player.command(PlayerCommand::Seek(position));
                    }
                    panel_changed = true;
                    if let Some(track) = self.selected.clone() {
                        if !self.private_session {
                            self.library.stats.started(&track);
                        }
                        if track.kind == ItemKind::Song {
                            let persistent = self
                                .library
                                .stats
                                .tracks
                                .get(&track.id)
                                .is_some_and(|stats| stats.qualified_plays >= 2);
                            let _ = self.backend.caches.send((
                                track.clone(),
                                persistent,
                                self.library.ytdlp_path.clone(),
                                self.playback_cookies(),
                            ));
                        }
                        if !self.private_session {
                            self.library.played(track);
                        }
                        if self.screen == Screen::Home {
                            self.content_revision += 1;
                        }
                        self.save();
                    }
                    changed = true;
                }
                PlayerEvent::Position(position) => {
                    if self.buffering || self.seek_preview.is_some() {
                        continue;
                    }
                    if let Some((target, since)) = self.seek_pending {
                        if (position - target).abs() > 1.5
                            && since.elapsed() < Duration::from_secs(2)
                        {
                            continue;
                        }
                        self.seek_pending = None;
                    }
                    if let Some(track) = &self.selected {
                        let seconds = self.listening.sample(
                            position,
                            Instant::now(),
                            self.playing && !self.buffering,
                        );
                        if !self.private_session {
                            self.library.stats.listen(track, seconds);
                        }
                        if !self.listening.qualified
                            && self.duration > 0.
                            && self.listening.seconds >= self.duration * 0.7
                        {
                            self.listening.qualified = true;
                            if self.library.sync_history
                                && !self.private_session
                                && self.signed_in()
                                && !self.library.offline_only
                                && track.kind == ItemKind::Song
                            {
                                self.account.edit(
                                    self.auth.snapshot(),
                                    Edit::History {
                                        track: track.clone(),
                                    },
                                );
                            }
                            let persistent =
                                !self.private_session && self.library.stats.qualify(&track.id);
                            if track.kind == ItemKind::Song {
                                let _ = self.backend.caches.send((
                                    track.clone(),
                                    persistent,
                                    self.library.ytdlp_path.clone(),
                                    self.playback_cookies(),
                                ));
                            }
                            self.save();
                        }
                        if self.stats_saved.elapsed() >= Duration::from_secs(15)
                            && !self.shell_frame_pending
                        {
                            self.stats_saved = Instant::now();
                            self.save();
                            if self.screen == Screen::Recap {
                                self.content_revision += 1;
                            }
                        }
                    }
                    self.position = position.clamp(0., self.duration.max(position));
                    if !self.sleep_end
                        && self.library.audio.crossfade > 0.
                        && self.queue.repeat != Repeat::One
                        && self.duration - position <= self.library.audio.crossfade
                        && self.duration - position > 0.
                        && self
                            .queue
                            .next_track(true)
                            .is_some_and(|t| self.prepared_id.as_deref() == Some(&t.id))
                    {
                        self.next(true, cx);
                        continue;
                    }
                    clock_changed = true;
                }
                PlayerEvent::Duration(duration) => {
                    self.duration = duration;
                    panel_changed = true;
                    changed = true;
                }
                PlayerEvent::Paused(paused) => {
                    self.playing = !paused;
                    panel_changed = true;
                    changed = true;
                }
                PlayerEvent::Ended => {
                    if self.sleep_end {
                        self.sleep_end = false;
                        self.playing = false;
                        self.buffering = false;
                        self.failed = true;
                        self.resume_at = Some(0.);
                        self.position = 0.;
                        self.save();
                    } else {
                        self.next(true, cx);
                    }
                    panel_changed = true;
                    changed = true;
                }
                PlayerEvent::Error(error) => {
                    self.buffering = false;
                    self.playing = false;
                    self.failed = true;
                    self.notice = Some(error);
                    panel_changed = true;
                    changed = true;
                }
                PlayerEvent::SettingsError(error) => {
                    self.notice = Some(error);
                    changed = true;
                }
            }
        }
        if clock_changed {
            self.prefetch_next();
            if !self.immersive {
                self.playback_progress.update(cx, |_, cx| cx.notify());
            }
            if self.immersive || self.screen == Screen::Lyrics {
                self.mini_progress.update(cx, |_, cx| cx.notify());
            }
        }
        if self.panel == Some(Panel::Lyrics)
            || (self.immersive && self.immersive_lyrics)
            || self.screen == Screen::Lyrics
        {
            self.follow_lyrics(window, cx);
        }
        // Tray/relaunch requests coalesce until the current reads settle. Never
        // lose the reopen sync just because authentication or a page was loading.
        if self.sync_on_open && !self.auth.busy() && !self.loading && !self.account.syncing() {
            self.sync_on_open = false;
            if self.signed_in() {
                self.sync_youtube_music(cx);
            }
        }
        self.auto_load_more(window, cx);
        if panel_changed {
            self.right_content.update(cx, |_, cx| cx.notify());
        }
        if changed {
            cx.notify();
        }
    }
    fn auto_load_more(&mut self, window: &Window, cx: &mut Context<Self>) {
        // Geometry comes from the actual clipped scroll region, including virtual lists.
        let paginated = matches!(
            &self.screen,
            Screen::Home
                | Screen::Library
                | Screen::Liked
                | Screen::Explore
                | Screen::Discover(..)
                | Screen::Search
                | Screen::PlaylistPicker
                | Screen::Collection(_, _, false)
        );
        if !paginated
            || !window.is_window_active()
            || self.library.offline_only
            || self.loading
            || self.visible.is_empty()
            || self.visible.len() >= 10_000
            || self.pagination_attempts.len() >= 128
            || !self.scroll.near_end()
        {
            return;
        }
        let token = self
            .account_view_key()
            .and_then(|key| {
                self.account
                    .page(&key)
                    .and_then(|page| page.continuation.clone())
            })
            .or_else(|| self.continuation.clone());
        let Some(token) = token else {
            return;
        };
        // Repeated/cycling tokens or failed responses must never create a request loop.
        if !self.pagination_attempts.insert((
            self.screen.clone(),
            self.library_section,
            token.clone(),
        )) {
            return;
        }
        if let Some(key) = self.account_view_key() {
            self.account_read(key, true, cx);
        } else {
            let search = self.screen == Screen::Search;
            let visitor = self.page.visitor_data.clone();
            self.request(|id| Request::More(id, token, search, visitor), cx);
        }
    }
    fn trim_artwork(&mut self) {
        // Each cover can have a content key and a song-ID alias. Preserve
        // the original 512-cover capacity instead of counting each alias twice.
        const ENTRY_LIMIT: usize = 1024;
        if self.artwork.len() <= ENTRY_LIMIT {
            return;
        }
        let collection_cover =
            matches!(self.screen, Screen::Collection(..)).then(|| self.collection_art());
        let avatar_cover = self.auth.account().and_then(|account| {
            account.avatar_url.map(|url| {
                let mut track = Track::song("profile", "", "");
                track.artwork = Some(url);
                track
            })
        });
        let sidebar_playlists = self.account.page(&crate::account_sync::Key::Section(
            AccountSection::Playlists,
        ));
        let protected: std::collections::HashSet<_> = self
            .visible
            .iter()
            .take(256)
            .chain(self.library.history.iter().take(9))
            .chain(self.selected.iter())
            .chain(self.previous_track.iter())
            .chain(collection_cover.iter())
            .chain(avatar_cover.iter())
            .chain(
                sidebar_playlists
                    .into_iter()
                    .flat_map(|page| page.tracks.iter().take(80)),
            )
            .chain(
                self.library
                    .playlists
                    .iter()
                    .filter_map(|playlist| playlist.tracks.first()),
            )
            .flat_map(|track| [track.artwork_key(), track.id.as_str()])
            .collect();
        let mut oldest: Vec<_> = self
            .artwork_arrived
            .iter()
            .filter(|(id, _)| !protected.contains(id.as_str()))
            .map(|(id, at)| (id.clone(), *at))
            .collect();
        oldest.sort_by_key(|(_, at)| *at);
        for (id, _) in oldest
            .into_iter()
            .take(self.artwork.len().saturating_sub(ENTRY_LIMIT))
        {
            self.artwork.remove(&id);
            self.artwork_arrived.remove(&id);
            self.artwork_colors.remove(&id);
        }
    }
    fn refresh_visible(&mut self) {
        let saved = self.account_tracks(AccountSection::Songs);
        let liked = self.account_tracks(AccountSection::Liked);
        let history = self.account_tracks(AccountSection::History);
        self.library
            .refresh_smart_playlists_with(&saved, &liked, &history);
        self.content_revision += 1;
        let tracks = match &self.screen {
            Screen::Home => {
                if self.catalog.is_empty() {
                    self.editorial.clone()
                } else {
                    self.catalog.clone()
                }
            }
            Screen::Liked if self.signed_in() => {
                Arc::new(self.account_tracks(AccountSection::Liked))
            }
            Screen::Library if self.signed_in() => {
                Arc::new(self.account_tracks(self.library_section))
            }
            Screen::Liked | Screen::Library => Arc::new(self.library.liked.clone()),
            Screen::Recents => Arc::new(self.recent_tracks()),
            Screen::Local => Arc::new(self.library.local.clone()),
            Screen::Offline => self.offline.clone(),
            Screen::Collection(id, _, true) => Arc::new(
                self.library
                    .playlists
                    .iter()
                    .find(|p| &p.id == id)
                    .map(|p| p.tracks.clone())
                    .unwrap_or_default(),
            ),
            Screen::Queue => Arc::new(self.queue.tracks().cloned().collect()),
            Screen::Collection(..) if self.account_collection_key().is_some() => Arc::new(
                self.account
                    .page(&self.account_collection_key().unwrap())
                    .map_or_else(Vec::new, |p| p.tracks.clone()),
            ),
            _ => self.catalog.clone(),
        };
        if self.screen == Screen::Search && self.search_filter > 0 {
            let kind = [
                ItemKind::Song,
                ItemKind::Song,
                ItemKind::Album,
                ItemKind::Artist,
                ItemKind::Playlist,
            ][self.search_filter]
                .clone();
            self.visible = Arc::new(
                tracks
                    .iter()
                    .filter(|track| track.kind == kind)
                    .cloned()
                    .collect(),
            );
        } else {
            self.visible = tracks;
        }
        if matches!(
            self.screen,
            Screen::Library
                | Screen::Liked
                | Screen::Local
                | Screen::Offline
                | Screen::Collection(..)
        ) {
            let mut tracks = self.visible.as_ref().clone();
            self.filter_and_sort(&mut tracks);
            self.visible = Arc::new(tracks);
        }
        if matches!(
            self.screen,
            Screen::Home | Screen::Radio(_) | Screen::Explore | Screen::Discover(..)
        ) && !self.library.hidden.is_empty()
        {
            self.visible = Arc::new(
                self.visible
                    .iter()
                    .filter(|t| !self.library.hidden.contains(&t.id))
                    .cloned()
                    .collect(),
            );
        }
    }
    fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        match screen {
            Screen::Queue => {
                self.toggle_panel(Panel::Queue, cx);
                return;
            }
            Screen::Recents => {
                self.toggle_panel(Panel::Recent, cx);
                return;
            }
            Screen::Lyrics => {
                self.toggle_panel(Panel::Lyrics, cx);
                return;
            }
            _ => {}
        }
        if self.screen != screen {
            self.selected_rows.clear();
            self.selecting = false;
            self.back.push(self.screen.clone());
            self.forward.clear();
            self.screen = screen;
            self.collection_reveal_at = Instant::now();
            self.revision += 1;
            self.loading = false;
            self.continuation = None;
            self.pagination_attempts.clear();
            self.catalog = Arc::new(vec![]);
            self.page = raag::catalog::Page::default();
            self.collection_seed = None;
            self.scroll.reset();
            self.search_filter = 0;
            self.notice = None;
            self.editing_playlist = false;
            self.playlist_editor_remote = false;
            self.editing_description = false;
            self.editing_smart = None;
            self.folder_editor = false;
            self.refresh_visible();
            fetch_artwork(
                self.visible.iter().take(9).cloned().collect(),
                self.backend.events_sender.clone(),
            );
            if matches!(self.screen, Screen::Home | Screen::Explore) {
                self.reload(cx);
            }
            if self.signed_in() && matches!(self.screen, Screen::Library | Screen::Liked) {
                let section = if self.screen == Screen::Liked {
                    AccountSection::Liked
                } else {
                    self.library_section
                };
                self.account_read(crate::account_sync::Key::Section(section), false, cx);
            }
            if self.screen == Screen::Lyrics {
                self.load_lyrics();
            }
        }
        cx.notify();
    }
    fn history_navigation(&mut self, forward: bool, cx: &mut Context<Self>) {
        let next = if forward {
            self.forward.pop()
        } else {
            self.back.pop()
        };
        if let Some(screen) = next {
            if forward {
                self.back.push(self.screen.clone());
            } else {
                self.forward.push(self.screen.clone());
            }
            self.screen = screen;
            self.collection_reveal_at = Instant::now();
            self.revision += 1;
            self.loading = false;
            self.catalog = Arc::new(vec![]);
            self.editing_playlist = false;
            self.playlist_editor_remote = false;
            self.editing_description = false;
            self.continuation = None;
            self.pagination_attempts.clear();
            self.notice = None;
            self.page = raag::catalog::Page::default();
            self.collection_seed = None;
            self.scroll.reset();
            self.refresh_visible();
            self.reload(cx);
            cx.notify();
        }
    }
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.pagination_attempts.clear();
        if let Some(key) = self.account_collection_key() {
            self.account_read(key, false, cx);
            return;
        }
        if self.signed_in() && matches!(self.screen, Screen::Library | Screen::Liked) {
            let section = if self.screen == Screen::Liked {
                AccountSection::Liked
            } else {
                self.library_section
            };
            self.account_read(crate::account_sync::Key::Section(section), false, cx);
            return;
        }
        match self.screen.clone() {
            Screen::Home => self.request(Request::Home, cx),
            Screen::Explore => self.request(Request::Explore, cx),
            Screen::Search => {
                let query = self.search_query.clone();
                self.request(|id| Request::Search(id, query, None), cx);
            }
            Screen::Radio(id) => self.request(|generation| Request::Radio(generation, id), cx),
            Screen::Collection(id, title, false) => {
                if id.starts_with("artist:") {
                    self.request(|generation| Request::Artist(generation, title), cx);
                } else {
                    self.request(|generation| Request::Browse(generation, id, title), cx);
                }
            }
            Screen::Discover(id, params, title) => self.request(
                |generation| Request::Discover(generation, id, params, title),
                cx,
            ),
            _ => {}
        }
    }
    fn open_track(&mut self, track: Track, cx: &mut Context<Self>) {
        if self.signed_in() && track.kind == ItemKind::Playlist {
            self.open_account_playlist(track, cx);
            return;
        }
        if track.playable() {
            let mut tracks = self.visible.as_ref().clone();
            if !tracks.iter().any(|song| song.id == track.id) {
                // Recent Home tiles may originate from account history rather than the catalog shelves.
                tracks.insert(0, track.clone());
            }
            self.queue.replace_selected(tracks, &track);
            self.invalidate_collection();
            self.complete_collection(false);
            self.start(track, cx);
        } else {
            let seed = track.clone();
            self.navigate(
                Screen::Collection(track.id.clone(), track.title.clone(), false),
                cx,
            );
            self.collection_seed = Some(seed);
            self.request(|id| Request::Browse(id, track.id, track.title), cx);
        }
    }
    fn start(&mut self, track: Track, cx: &mut Context<Self>) {
        self.right_content.update(cx, |_, cx| cx.notify());
        self.listening = raag::stats::ListeningSession::default();
        self.content_revision += 1;
        self.stream_quality = raag::playback::StreamQuality::default();
        self.source_offline = false;
        self.lyrics_active = None;
        self.lyrics_geometry = None;
        self.lyrics_follow = true;
        self.lyrics_scroll.reset();
        self.previous_track = self.selected.clone();
        self.track_changed_at = Instant::now();
        self.duration = track.duration as f64;
        self.position = 0.;
        self.radio_pending = None;
        self.radio_attempted = None;
        self.radio_waiting = false;
        self.radio_mix = false;
        self.prepared_id = None;
        self.resume_at = None;
        self.seek_preview = None;
        self.seek_pending = None;
        for timeline in [&self.timeline, &self.mini_timeline] {
            timeline.update(cx, |slider, cx| {
                *slider = SliderState::new().step(0.1);
                cx.notify();
            });
        }
        self.playing = false;
        self.buffering = true;
        self.failed = false;
        self.notice = None;
        self.lyrics.clear();
        self.lyrics_id.clear();
        self.lyrics_error = None;
        self.player
            .command(PlayerCommand::Load(track.clone(), self.library.volume));
        crate::service::fetch_visible_artwork(
            vec![track.clone()],
            self.backend.events_sender.clone(),
        );
        self.selected = Some(track);
        self.save();
        if self.library.magic_accent {
            self.apply_appearance(cx);
        }
        if self.panel == Some(Panel::Lyrics) || (self.immersive && self.immersive_lyrics) {
            self.load_lyrics();
        }
        cx.notify();
    }
    fn next(&mut self, ended: bool, cx: &mut Context<Self>) {
        if let Some(track) = self.queue.advance(ended).cloned() {
            self.start(track, cx);
        } else if ended {
            self.playing = false;
            self.buffering = false;
            self.failed = true;
            self.resume_at = Some(0.);
            if self.library.autoplay
                && self
                    .selected
                    .as_ref()
                    .is_some_and(|t| t.kind == ItemKind::Song)
            {
                self.radio_waiting = true;
                self.request_radio();
            }
            self.save();
            cx.notify();
        }
        if self.screen == Screen::Queue {
            self.refresh_visible();
        }
    }
    fn previous(&mut self, cx: &mut Context<Self>) {
        if self.position > 3. {
            self.commit_seek(0., cx);
        } else if let Some(track) = self.queue.previous().cloned() {
            self.start(track, cx);
        }
    }
    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        self.right_content.update(cx, |_, cx| cx.notify());
        if self.selected.is_none() {
            if let Some(track) = self.visible.iter().find(|t| t.playable()).cloned() {
                self.open_track(track, cx);
            }
        } else if self.buffering {
            self.player.command(PlayerCommand::Stop);
            self.buffering = false;
            self.failed = true;
            cx.notify();
        } else if self.failed {
            if let Some(track) = self.selected.clone() {
                let resume = self.resume_at;
                self.start(track, cx);
                self.resume_at = resume;
            }
        } else {
            self.player.command(PlayerCommand::Pause(self.playing));
        }
    }
    fn load_lyrics(&mut self) {
        let Some(track) = self.selected.clone() else {
            return;
        };
        let prefix = format!("{}:{:?}:", track.id, self.library.lyrics.source);
        if self.lyrics_id.starts_with(&prefix) {
            return;
        }
        let key = format!("{prefix}{}", self.lyrics_revision);
        self.lyrics_id = key.clone();
        self.lyrics_loading = true;
        self.lyrics_revision = self.lyrics_revision.wrapping_add(1);
        self.lyrics_error = None;
        let _ = self
            .backend
            .lyrics
            .send((key, track, self.library.lyrics.source, false));
    }
    fn like(&mut self, track: Track, cx: &mut Context<Self>) {
        if self.signed_in() && track.kind == ItemKind::Song {
            let liked = !self.is_liked(&track);
            self.account_edit(Edit::Like { track, liked }, cx);
            return;
        }
        self.library.toggle_like(track);
        self.right_content.update(cx, |_, cx| cx.notify());
        self.save();
        self.refresh_visible();
        cx.notify();
    }
    fn prefetch_next(&mut self) {
        if !self.playing
            || self.buffering
            || self.duration <= 0.
            || (self.position < 5. && self.duration - self.position > 20.)
            || self.seek_pending.is_some()
        {
            return;
        }
        let Some(track) = self.queue.next_track(true) else {
            if self.library.autoplay {
                self.request_radio();
            }
            return;
        };
        let generation = self.player.generation();
        if self
            .prefetched_next
            .as_ref()
            .is_some_and(|(id, next)| *id == generation && next == &track.id)
        {
            return;
        }
        let key = (generation, track.id.clone());
        let track = track.clone();
        self.prefetched_next = Some(key);
        self.player.command(PlayerCommand::Prefetch(track.clone()));
        fetch_artwork(vec![track.clone()], self.backend.events_sender.clone());
    }
    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.library.sidebar = !self.library.sidebar;
        self.sidebar_scroll.reset();
        self.save();
        cx.notify();
    }
    fn commit_seek(&mut self, target: f64, cx: &mut Context<Self>) {
        if self.buffering || self.failed || self.duration <= 0. || !target.is_finite() {
            return;
        }
        let target = target.clamp(0., self.duration);
        self.listening.reset_sample();
        self.seek_preview = None;
        self.seek_pending = Some((target, Instant::now()));
        self.position = target;
        for timeline in [&self.timeline, &self.mini_timeline] {
            timeline.update(cx, |slider, cx| {
                *slider = SliderState::new()
                    .step(0.1)
                    .default_value((target / self.duration * 100.) as f32);
                cx.notify();
            });
        }
        self.player.command(PlayerCommand::Seek(target));
        cx.notify();
    }
    fn sidebar_expanded(&self, window: &Window) -> bool {
        self.library.sidebar
            && f32::from(window.bounds().size.width) / self.library.ui_scale
                >= if self.panel.is_some() { 900. } else { 760. }
    }
    fn right_sidebar_rems(&self, window: &Window) -> f32 {
        let width = f32::from(window.bounds().size.width) / self.library.ui_scale;
        if width < 750. {
            return 12.;
        }
        if width >= 1500. {
            22.
        } else if width >= 1200. {
            19.
        } else {
            16.
        }
    }
    fn content_width(&self, window: &Window) -> f32 {
        let [left, right, _] = self.shell_motion.dimensions();
        let left = left + 12. * (left / (256. * raag::i18n::scale())).clamp(0., 1.);
        (f32::from(window.bounds().size.width) - 24. - left - right).max(0.)
    }
    /// Density changes with the window, not with transient pane visibility.
    fn layout_width(&self, window: &Window) -> f32 {
        let right =
            f32::from(rems(self.right_sidebar_rems(window)).to_pixels(window.rem_size())) + 12.;
        (f32::from(window.bounds().size.width) / self.library.ui_scale
            - 24.
            - 268.
            - right / self.library.ui_scale)
            .max(280.)
    }
    fn import(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(t("Add music files or a folder").into()),
        });
        cx.spawn(async move |view, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await {
                let _ = view.update(cx, |this, cx| {
                    this.navigate(Screen::Local, cx);
                    this.loading = true;
                    let mut library = this.library.clone();
                    let sender = this.backend.events_sender.clone();
                    std::thread::spawn(move || {
                        let count = library.import_paths(&paths);
                        let _ = sender.send(Event::Imported(Box::new(library), count));
                    });
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn reset_player(&mut self, cx: &mut Context<Self>) {
        let replacement =
            Player::with_cookies(self.library.ytdlp_path.clone(), self.playback_cookies());
        let previous = std::mem::replace(&mut self.player, replacement);
        previous.command(PlayerCommand::Stop);
        // Drop joins audio and extractor workers. Account/tool changes must not
        // wait for those workers on the GPUI foreground executor.
        cx.background_spawn(async move { drop(previous) }).detach();
        self.player
            .command(PlayerCommand::Configure(self.library.audio.clone()));
        self.playing = false;
        self.buffering = false;
        self.failed = self.selected.is_some();
        self.save();
    }
    fn playback_cookies(&self) -> String {
        self.auth.snapshot().session.map_or_else(
            || self.library.cookies_path.clone(),
            |s| s.playback_path().into(),
        )
    }
    fn sign_in(&mut self, cx: &mut Context<Self>) {
        self.auth.sign_in(None);
        self.content_revision += 1;
        self.notice = Some(
            t("Complete sign-in in the Google window. Cassette never receives your password.")
                .into(),
        );
        cx.notify();
    }
    fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.library.cookies_path.clear();
        self.auth.sign_out();
        self.content_revision += 1;
        self.save();
        cx.notify();
    }
    fn choose_cookies(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Choose your Netscape-format YouTube cookie file").into()),
        });
        cx.spawn(async move |view, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(path) = paths.first()
            {
                let path = path.clone();
                let _ = view.update(cx, |this, cx| {
                    this.auth.sign_in(Some(path));
                    this.content_revision += 1;
                    this.right_content.update(cx, |_, cx| cx.notify());
                    this.notice = Some(t("Validating your YouTube session…").into());
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn save_playlist_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_description {
            if let Screen::Collection(id, _, false) = &self.screen {
                self.account_edit(
                    Edit::Description {
                        playlist: id.clone(),
                        text: self.playlist_name.read(cx).value().to_string(),
                    },
                    cx,
                );
                window.focus(&self.focus, cx);
            }
            return;
        }
        let name = self.playlist_name.read(cx).value().to_string();
        if self.playlist_editor_remote && self.signed_in() {
            let edit = if let Some(crate::account_sync::Key::Playlist(playlist)) =
                self.account_collection_key()
            {
                Edit::Rename { playlist, name }
            } else {
                Edit::Create { name }
            };
            self.account_edit(edit, cx);
            window.focus(&self.focus, cx);
            return;
        }
        let result = if let Screen::Collection(id, _, true) = &self.screen {
            self.library.rename_playlist(id, &name).map(|_| id.clone())
        } else {
            self.library.create_playlist(&name)
        };
        match result {
            Ok(id) => {
                self.library
                    .playlist_metadata
                    .entry(id.clone())
                    .or_insert_with(|| raag::organization::PlaylistMetadata {
                        folder: self.library.active_folder.clone(),
                        cover: None,
                    });
                self.navigation_revision = self.navigation_revision.wrapping_add(1);
                self.save();
                self.editing_playlist = false;
                self.navigate(Screen::Collection(id, name.trim().into(), true), cx);
                window.focus(&self.focus, cx);
            }
            Err(error) => {
                self.notice = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn playlist_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        gpui_kit::div()
            .flex()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .p_4()
            .rounded_lg()
            .bg(cx.theme().muted)
            .child(
                div()
                    .w_full()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.editing_description {
                        t("Playlist description")
                    } else if self.playlist_editor_remote {
                        t("YouTube Music playlist")
                    } else {
                        t("On this computer")
                    }),
            )
            .child(Input::new(&self.playlist_name).flex_1().min_w(ui_px(160.)))
            .child(
                Button::new("create-playlist")
                    .primary()
                    .label(if self.editing_description {
                        t("Save description")
                    } else {
                        t("Save playlist")
                    })
                    .disabled(self.playlist_editor_remote && self.account.busy_edits())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.save_playlist_name(window, cx)),
                    ),
            )
            .child(
                Button::new("cancel-playlist")
                    .ghost()
                    .label(t("Cancel"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.editing_playlist = false;
                        this.editing_description = false;
                        this.content_revision += 1;
                        window.focus(&this.focus, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    fn export_playlist(&mut self, id: String, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_new_path(&data_dir(), Some("playlist.raag.json"));
        let library = self.library.clone();
        cx.spawn(async move |view, cx| {
            if let Ok(Ok(Some(path))) = prompt.await {
                let result = cx
                    .background_spawn(async move {
                        library
                            .export_playlist(&id, &path)
                            .map_err(|e| e.to_string())
                    })
                    .await;
                let _ = view.update(cx, |this, cx| {
                    this.notice = Some(match result {
                        Ok(()) => t("Playlist exported").into(),
                        Err(error) => error,
                    });
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn import_playlist(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Import a Cassette playlist").into()),
        });
        cx.spawn(async move |view, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(path) = paths.first()
            {
                let path = path.clone();
                let library = view.update(cx, |this, _| this.library.clone());
                if let Ok(mut library) = library {
                    let result = cx
                        .background_spawn(async move {
                            library
                                .import_playlist(&path)
                                .map(|id| {
                                    library
                                        .playlists
                                        .iter()
                                        .find(|p| p.id == id)
                                        .unwrap()
                                        .clone()
                                })
                                .map_err(|e| e.to_string())
                        })
                        .await;
                    let _ = view.update(cx, |this, cx| {
                        match result {
                            Ok(playlist) => {
                                let screen = Screen::Collection(
                                    playlist.id.clone(),
                                    playlist.name.clone(),
                                    true,
                                );
                                this.library.playlists.push(playlist);
                                this.navigation_revision = this.navigation_revision.wrapping_add(1);
                                this.save();
                                this.navigate(screen, cx);
                            }
                            Err(error) => this.notice = Some(error),
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
    fn remove_playlist(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = window.prompt(
            PromptLevel::Warning,
            t("Remove this playlist?"),
            Some(t(
                "This removes the playlist from Cassette. Your local audio files are kept.",
            )),
            &[t("Cancel"), t("Remove playlist")],
            cx,
        );
        cx.spawn(async move |view, cx| {
            if prompt.await == Ok(1) {
                let _ = view.update(cx, |this, cx| {
                    this.library.remove_playlist(&id);
                    this.navigation_revision = this.navigation_revision.wrapping_add(1);
                    this.refresh_visible();
                    this.save();
                    this.navigate(Screen::Library, cx);
                });
            }
        })
        .detach();
    }
    fn title(&self, subtitle: &str, cx: &App) -> Div {
        gpui_kit::div()
            .flex()
            .flex_col()
            .gap_2()
            .mb_6()
            .child(
                gpui_kit::div()
                    .text_3xl()
                    .font_family(raag::i18n::font())
                    .font_weight(FontWeight::BOLD)
                    .child(self.screen.title().to_owned()),
            )
            .child(
                gpui_kit::div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(subtitle.to_owned()),
            )
    }
    fn artwork_path(&self, track: &Track) -> Option<&PathBuf> {
        self.artwork.get(track.artwork_key()).or_else(|| {
            track
                .artwork
                .is_none()
                .then(|| self.artwork.get(&track.id))
                .flatten()
        })
    }
    fn artwork_arrival(&self, track: &Track) -> Option<Instant> {
        self.artwork_arrived
            .get(track.artwork_key())
            .or_else(|| self.artwork_arrived.get(&track.id))
            .copied()
    }
    fn artwork_variant(&self, path: &std::path::Path, index: usize) -> PathBuf {
        if self
            .artwork_variants
            .get(path)
            .is_some_and(|mask| mask & (1 << index) != 0)
        {
            path.with_extension(["128.jpg", "256.jpg", "hero-v2.jpg"][index])
        } else {
            path.to_owned()
        }
    }
    fn request_missing_artwork(&self, track: &Track) {
        if (track.artwork.is_none()
            && (track.kind != ItemKind::Song
                || raag::catalog::validate_video_id(&track.id).is_err()))
            || self.artwork_path(track).is_some_and(|path| {
                !track.id.starts_with("page-hero:")
                    || self
                        .artwork_variants
                        .get(path)
                        .is_some_and(|mask| mask & 4 != 0)
            })
        {
            return;
        }
        let mut attempted = self.artwork_attempted.borrow_mut();
        if attempted
            .get(track.artwork_key())
            .is_some_and(|time| time.elapsed() < Duration::from_secs(15))
        {
            return;
        }
        if attempted.len() >= 512 {
            attempted.retain(|_, time| time.elapsed() < Duration::from_secs(15));
        }
        if attempted.len() < 512 {
            attempted.insert(track.artwork_key().to_owned(), Instant::now());
            self.missing_artwork.borrow_mut().push(track.clone(), 0);
        }
    }
    fn art_source(&self, track: &Track) -> ImageSource {
        if let Some(path) = track
            .artwork
            .as_ref()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            return path.into();
        }
        self.request_missing_artwork(track);
        self.artwork_path(track)
            .map(|path| path.clone().into())
            .unwrap_or_else(|| {
                let hash = track
                    .id
                    .bytes()
                    .fold(0usize, |hash, byte| hash + byte as usize);
                format!("raag/covers/cover-{}.png", hash % 6).into()
            })
    }
    fn cover(&self, track: &Track, extent: Rems, cx: &App) -> AnyElement {
        let radius = if track.kind == ItemKind::Artist {
            extent.to_pixels(cx.theme().font_size) / 2.
        } else {
            cx.theme().radius
        };
        if let Some(path) = track
            .artwork
            .as_ref()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            return div()
                .size(extent)
                .flex_shrink_0()
                .rounded(radius)
                .overflow_hidden()
                .child(
                    img(path)
                        .size_full()
                        .rounded(radius)
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element();
        }
        let hash = track
            .id
            .bytes()
            .fold(0usize, |sum, byte| sum + byte as usize);
        let placeholder: ImageSource = format!("raag/covers/cover-{}.png", hash % 6).into();
        let container = div()
            .size(extent)
            .relative()
            .flex_shrink_0()
            .rounded(radius)
            .overflow_hidden()
            .bg(cx.theme().muted);
        let Some(path) = self.artwork_path(track) else {
            let pending = self.missing_artwork.clone();
            let attempted = self.artwork_attempted.clone();
            let track = track.clone();
            return container
                .child(
                    img(placeholder)
                        .size_full()
                        .rounded(radius)
                        .object_fit(ObjectFit::Cover),
                )
                .child(
                    canvas(
                        move |bounds, window, _| {
                            // Prepaint sees the real clipping viewport, including carousel and scroll masks.
                            if (track.artwork.is_some()
                                || (track.kind == ItemKind::Song
                                    && raag::catalog::validate_video_id(&track.id).is_ok()))
                                && bounds.intersects(&window.content_mask().bounds)
                            {
                                let mut attempted = attempted.borrow_mut();
                                if attempted
                                    .get(track.artwork_key())
                                    .is_none_or(|at| at.elapsed() >= Duration::from_secs(15))
                                {
                                    attempted
                                        .retain(|_, at| at.elapsed() < Duration::from_secs(15));
                                    if attempted.len() < 512 {
                                        attempted
                                            .insert(track.artwork_key().to_owned(), Instant::now());
                                        pending.borrow_mut().push(track.clone(), 0);
                                    }
                                }
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0()
                    .size_full(),
                )
                .into_any_element();
        };
        let arrived = self.artwork_arrival(track);
        let elapsed = arrived
            .map(|time| time.elapsed().as_secs_f32())
            .unwrap_or(1.);
        let pixels = extent.to_pixels(cx.theme().font_size);
        let sharp = if pixels <= ui_px(64.) {
            self.artwork_variant(path, 0)
        } else if pixels <= ui_px(384.) {
            self.artwork_variant(path, 1)
        } else {
            path.clone()
        };
        if elapsed >= 0.26 || cx.reduce_motion() {
            return container
                .child(crate::images::picture_keyed(
                    sharp,
                    self.images.clone(),
                    SharedString::from(format!("cover-image-{}", track.id)),
                    radius,
                ))
                .into_any_element();
        }
        let soft = path.with_extension("soft.jpg");
        container
            .with_animation(
                SharedString::from(format!("art-reveal-{}-{}", track.id, path.display())),
                Animation::new(Duration::from_millis(260)).with_easing(ease_out_quint()),
                move |div, _| {
                    // Arrival time survives reparenting and bounds changes.
                    let phase = ease_out_quint()(
                        arrived
                            .map(|time| time.elapsed().as_secs_f32() / 0.26)
                            .unwrap_or(1.)
                            .min(1.),
                    );
                    div.child(div_placeholder(placeholder.clone(), 1. - phase, radius))
                        .child(div_placeholder(
                            soft.clone().into(),
                            (1. - phase) * phase * 2.,
                            radius,
                        ))
                        .child(div_placeholder(sharp.clone().into(), phase, radius))
                },
            )
            .into_any_element()
    }
    fn playing_cover(&self, track: &Track, extent: Rems, cx: &App) -> AnyElement {
        let sharp = self.art_source(track);
        let radius = if extent.0 <= 4. {
            cx.theme().radius
        } else {
            cx.theme().radius_lg
        };
        let soft = self
            .artwork_path(track)
            .map(|path| path.with_extension("soft.jpg"));
        let key = format!(
            "player-cover-{}-{}",
            self.player.generation(),
            soft.as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        let previous = self.previous_track.as_ref().map(|track| {
            self.artwork_path(track)
                .map(|p| p.with_extension("soft.jpg").into())
                .unwrap_or_else(|| self.art_source(track))
        });
        let muted = cx.theme().muted;
        let started = self
            .artwork_arrival(track)
            .unwrap_or(self.track_changed_at)
            .max(self.track_changed_at);
        let container = gpui_kit::div()
            .size(extent)
            .relative()
            .flex_shrink_0()
            .rounded(radius)
            .overflow_hidden()
            .bg(muted);
        if cx.reduce_motion() || started.elapsed() >= Duration::from_millis(240) {
            return container
                .child(
                    img(sharp)
                        .size_full()
                        .rounded(radius)
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element();
        }
        container
            .with_animation(
                SharedString::from(key),
                Animation::new(Duration::from_millis(if cx.reduce_motion() {
                    1
                } else {
                    240
                }))
                .with_easing(ease_out_quint()),
                move |div, _| {
                    let phase = ease_out_quint()((started.elapsed().as_secs_f32() / 0.24).min(1.));
                    div.when_some(previous.as_ref(), |div, source: &ImageSource| {
                        div.child(
                            gpui_kit::div()
                                .absolute()
                                .size_full()
                                .opacity(1. - phase)
                                .child(
                                    img(source.clone())
                                        .size_full()
                                        .rounded(radius)
                                        .object_fit(ObjectFit::Cover),
                                ),
                        )
                    })
                    .when_some(soft.as_ref(), |div, path| {
                        div.child(
                            gpui_kit::div()
                                .absolute()
                                .size_full()
                                .opacity(1. - phase)
                                .child(
                                    img(path.clone())
                                        .size_full()
                                        .rounded(radius)
                                        .object_fit(ObjectFit::Cover),
                                ),
                        )
                    })
                    .child(
                        gpui_kit::div().absolute().size_full().opacity(phase).child(
                            img(sharp.clone())
                                .size_full()
                                .rounded(radius)
                                .object_fit(ObjectFit::Cover),
                        ),
                    )
                },
            )
            .into_any_element()
    }
    fn playing_identity(&self, track: &Track, cx: &mut Context<Self>) -> AnyElement {
        let started = self.track_changed_at;
        let reduced = cx.reduce_motion();
        let muted = if self.immersive {
            rgb(0xffffff).opacity(0.65).into()
        } else {
            cx.theme().muted_foreground
        };
        let foreground = if self.immersive {
            rgb(0xffffff).into()
        } else {
            cx.theme().foreground
        };
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(foreground)
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(track.title.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .truncate()
                    .child(self.artist_link(track, cx)),
            )
            .with_animation(
                ("player-identity", self.player.generation() as usize),
                Animation::new(Duration::from_millis(if cx.reduce_motion() {
                    1
                } else {
                    220
                }))
                .with_easing(ease_out_quint()),
                move |div, _| {
                    let phase = if reduced {
                        1.
                    } else {
                        ease_out_quint()((started.elapsed().as_secs_f32() / 0.22).min(1.))
                    };
                    div.opacity(phase).mt(px((1. - phase) * 2.))
                },
            )
            .into_any_element()
    }
    fn toolbar(&self, expanded: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let account = self.auth.account();
        let signed_in = account.is_some();
        let busy = self.auth.busy();
        let can_cancel = self.auth.can_cancel();
        let can_sign_out = self.auth.can_sign_out();
        let syncing = self.loading || self.account.syncing();
        let owner = cx.weak_entity();
        let avatar_track = account.as_ref().and_then(|account| {
            let mut track = Track::song(format!("profile:{}", account.cache_scope()), "", "");
            track.artwork.clone_from(&account.avatar_url);
            track.artwork.as_ref()?;
            Some(track)
        });
        let avatar_path = avatar_track.as_ref().and_then(|track| {
            self.request_missing_artwork(track);
            self.artwork_path(track).cloned()
        });
        let initial = account
            .as_ref()
            .and_then(|a| a.name.chars().next())
            .map_or_else(|| "C".into(), |c| c.to_uppercase().to_string());
        gpui_kit::div()
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .h(ui_px(72.))
            .px_8()
            .gap_4()
            .flex_shrink_0()
            .child(
                gpui_kit::div()
                    .flex()
                    .items_center()
                    .flex_1()
                    .min_w(ui_px(108.))
                    .gap_1()
                    .child(
                        Button::new("toggle-sidebar")
                            .ghost()
                            .icon(icon(if expanded {
                                "sidebar-collapse"
                            } else {
                                "sidebar-expand"
                            }))
                            .tooltip(t("Toggle sidebar · Ctrl+B"))
                            .accessibility_label(t("Toggle sidebar"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                    )
                    .child(
                        Button::new("back")
                            .ghost()
                            .icon(icon("nav-arrow-left"))
                            .tooltip(t("Back · Alt+Left"))
                            .accessibility_label(t("Back"))
                            .disabled(self.back.is_empty())
                            .on_click(
                                cx.listener(|this, _, _, cx| this.history_navigation(false, cx)),
                            ),
                    )
                    .child(
                        Button::new("forward")
                            .ghost()
                            .icon(icon("nav-arrow-right"))
                            .tooltip(t("Forward"))
                            .accessibility_label(t("Forward"))
                            .disabled(self.forward.is_empty())
                            .on_click(
                                cx.listener(|this, _, _, cx| this.history_navigation(true, cx)),
                            ),
                    ),
            )
            .child(
                gpui_kit::div()
                    .flex()
                    .items_center()
                    .w(relative(0.55))
                    .max_w(rems(32.))
                    .min_w_0()
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .min_w_0()
                            .child(
                                Input::new(&self.query)
                                    .prefix(icon("search"))
                                    .cleanable(true)
                                    .rounded_full()
                                    .w_full(),
                            )
                            .when(
                                !self.suggestions.is_empty()
                                    && self.query.read(cx).focus_handle(cx).is_focused(window),
                                |body| {
                                    body.child(
                                        deferred(self.search_suggestions(cx)).with_priority(2),
                                    )
                                },
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .flex_1()
                    .min_w(ui_px(108.))
                    .child(
                        Button::new("profile")
                            .ghost()
                            .rounded_full()
                            .size(ui_px(34.))
                            .p_0()
                            .overflow_hidden()
                            .bg(cx.theme().muted)
                            .child(
                                div()
                                    .size_full()
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(initial)
                                    .when_some(avatar_path, |avatar, path| {
                                        avatar.child(
                                            div()
                                                .absolute()
                                                .inset_0()
                                                .rounded_full()
                                                .overflow_hidden()
                                                .child(
                                                    img(path.with_extension("128.jpg"))
                                                        .size_full()
                                                        .rounded_full()
                                                        .object_fit(ObjectFit::Cover),
                                                ),
                                        )
                                    }),
                            )
                            .tooltip(t("Your music and settings"))
                            .accessibility_label(t("Open profile menu"))
                            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                let menu = if can_cancel {
                                    menu.menu(t("Cancel sign-in"), Box::new(CancelSignIn))
                                } else if busy {
                                    menu
                                } else if signed_in {
                                    menu.menu(t("Switch YouTube account"), Box::new(SignIn))
                                        .menu(t("Sign out"), Box::new(SignOut))
                                } else if can_sign_out {
                                    menu.menu(t("Retry sign out"), Box::new(SignOut))
                                } else {
                                    menu.menu(t("Sign in to YouTube Music"), Box::new(SignIn))
                                };
                                let owner = owner.clone();
                                menu.item(
                                    PopupMenuItem::new(if syncing {
                                        t("Syncing YouTube Music…")
                                    } else {
                                        t("Sync YouTube Music")
                                    })
                                    .disabled(syncing || busy)
                                    .on_click(
                                        move |_, _, cx| {
                                            let _ = owner
                                                .update(cx, |this, cx| this.sync_youtube_music(cx));
                                        },
                                    ),
                                )
                                .separator()
                                .menu(t("Listening story"), Box::new(OpenRecap))
                                .menu(t("Recently played"), Box::new(ShowRecent))
                                .menu(t("Local files"), Box::new(OpenLocal))
                                .menu(t("Offline music"), Box::new(OpenOffline))
                                .separator()
                                .menu(t("Settings"), Box::new(OpenSettings))
                            }),
                    ),
            )
            .into_any_element()
    }
    fn sidebar(&self, expanded: bool, cx: &mut Context<Self>) -> AnyElement {
        let mut sidebar = gpui_kit::div()
            .flex()
            .flex_col()
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .gap_2()
            .pt_6()
            .pb_5()
            .when(expanded, |div| div.w_64().px_4())
            .when(!expanded, |div| div.w_16().px_2());
        let mut navigation = self
            .sidebar_scroll
            .bind(div().id("sidebar-navigation"))
            .flex()
            .flex_col()
            .gap_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        for (id, label, image, screen) in [
            ("home", t("Home"), "home-simple", Screen::Home),
            ("explore", t("Explore"), "compass", Screen::Explore),
            ("library", t("Your library"), "library", Screen::Library),
            ("liked", t("Liked songs"), "heart", Screen::Liked),
        ] {
            navigation = navigation.child(
                Button::new(id)
                    .ghost()
                    .w_full()
                    .flex_shrink_0()
                    .selected(self.screen == screen)
                    .tooltip(label)
                    .accessibility_label(label)
                    .child(
                        gpui_kit::div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(icon(image))
                            .when(expanded, |div| div.child(label)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(screen.clone(), cx))),
            );
        }
        if expanded {
            sidebar = sidebar.child(navigation.child(self.sidebar_playlists(cx)));
        } else {
            sidebar = sidebar.child(navigation);
        }
        sidebar.into_any_element()
    }
    fn like_button(
        &self,
        track: &Track,
        id: impl Into<ElementId>,
        cx: &mut Context<Self>,
    ) -> Button {
        let track = track.clone();
        Button::new(id)
            .ghost()
            .small()
            .icon(icon("heart"))
            .selected(self.is_liked(&track))
            .disabled(self.account.editing(&Edit::Like {
                track: track.clone(),
                liked: true,
            }))
            .tooltip(t("Toggle like"))
            .accessibility_label(t("Toggle like"))
            .on_click(cx.listener(move |this, _, _, cx| this.like(track.clone(), cx)))
    }
    fn basic_controls(&self, location: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let sidebar = location == "bottom" && self.sidebar_visible;
        let foreground: Hsla = if self.immersive {
            rgb(0xffffff).into()
        } else {
            cx.theme().foreground
        };
        gpui_kit::div()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h(ui_px(40.))
            .gap_3()
            .when(sidebar, |row| row.justify_start().h(ui_px(28.)).gap_1())
            .child(
                Button::new(SharedString::from(format!("{location}-previous")))
                    .ghost()
                    .when(sidebar, |button| button.small().size(ui_px(28.)))
                    .icon(icon("skip-prev"))
                    .text_color(foreground)
                    .tooltip(t("Previous · Ctrl+Left"))
                    .accessibility_label(t("Previous track"))
                    .disabled(self.selected.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.previous(cx))),
            )
            .child(
                Button::new(SharedString::from(format!("{location}-play-pause")))
                    .when(!sidebar, |button| {
                        button
                            .primary()
                            .rounded_full()
                            .size(ui_px(40.))
                            .map(playback_depth)
                    })
                    .when(sidebar, |button| button.ghost().small().size(ui_px(28.)))
                    .when(self.immersive, |button| {
                        button.bg(rgb(0xffffff)).text_color(rgb(0x23232b))
                    })
                    .when(!self.buffering, |button| {
                        button.icon(icon(if self.playing { "pause" } else { "play" }))
                    })
                    .when(self.buffering, |button| {
                        button.child(gpui_kit::component::spinner::Spinner::new().small())
                    })
                    .tooltip(if self.buffering {
                        t("Cancel loading")
                    } else if self.failed {
                        t("Retry playback")
                    } else {
                        t("Play / pause · Space")
                    })
                    .accessibility_label(if self.buffering {
                        t("Cancel loading")
                    } else if self.failed {
                        t("Retry playback")
                    } else {
                        t("Play or pause")
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_playback(cx))),
            )
            .child(
                Button::new(SharedString::from(format!("{location}-next")))
                    .ghost()
                    .when(sidebar, |button| button.small().size(ui_px(28.)))
                    .icon(icon("skip-next"))
                    .text_color(foreground)
                    .tooltip(t("Next · Ctrl+Right"))
                    .accessibility_label(t("Next track"))
                    .disabled(self.selected.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.next(false, cx))),
            )
            .into_any_element()
    }
    fn extra_controls(&self, fullscreen: bool, cx: &mut Context<Self>) -> AnyElement {
        gpui_kit::div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("shuffle")
                    .ghost()
                    .small()
                    .icon(icon("shuffle"))
                    .selected(self.queue.shuffle)
                    .tooltip(t("Shuffle"))
                    .accessibility_label(t("Shuffle"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.queue.toggle_shuffle();
                        this.right_content.update(cx, |_, cx| cx.notify());
                        self_refresh(this, cx);
                    })),
            )
            .child(
                Button::new("repeat")
                    .ghost()
                    .small()
                    .icon(icon("repeat"))
                    .when(self.queue.repeat == Repeat::One, |button| button.label("1"))
                    .selected(self.queue.repeat != Repeat::Off)
                    .tooltip(tr!("Repeat: {:?}", self.queue.repeat))
                    .accessibility_label(t("Cycle repeat mode"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.queue.repeat = this.queue.repeat.cycle();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("lyrics")
                    .ghost()
                    .small()
                    .icon(icon("lyrics"))
                    .selected(self.panel == Some(Panel::Lyrics))
                    .tooltip(t("Lyrics · Ctrl+L"))
                    .accessibility_label(t("Lyrics"))
                    .on_click(cx.listener(|this, _, _, cx| this.navigate(Screen::Lyrics, cx))),
            )
            .when(fullscreen, |div| {
                div.child(
                    Button::new("fullscreen")
                        .ghost()
                        .small()
                        .icon(icon("expand"))
                        .tooltip(t("Fullscreen · F11"))
                        .accessibility_label(t("Fullscreen"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_immersive(window, cx)),
                        ),
                )
            })
            .into_any_element()
    }
    fn seek_bar(&self, mini: bool, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::base::{Slider as SeekSlider, SliderIndicator, SliderThumb, SliderTrack};
        let timeline = if mini {
            &self.mini_timeline
        } else {
            &self.timeline
        };
        let disabled =
            self.selected.is_none() || self.buffering || self.failed || self.duration <= 0.;
        let fraction = timeline.read(cx).percentage().end.clamp(0., 1.);
        let dragging = self.seek_preview.is_some();
        let foreground: Hsla = if self.immersive {
            rgb(0xffffff).into()
        } else {
            cx.theme().foreground
        };
        let primary = if self.immersive {
            foreground
        } else {
            cx.theme().primary
        };
        div()
            .id(if mini {
                "mini-timeline"
            } else {
                "playback-timeline"
            })
            .track_focus(if mini {
                &self.mini_seek_focus
            } else {
                &self.seek_focus
            })
            .aria_label(t("Playback position; arrow keys seek five seconds"))
            .w_full()
            .h(ui_px(28.))
            .px(ui_px(6.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.focus(
                        if mini {
                            &this.mini_seek_focus
                        } else {
                            &this.seek_focus
                        },
                        cx,
                    );
                }),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if disabled
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.alt
                {
                    return;
                }
                let target = match event.keystroke.key.as_str() {
                    "left" | "down" => this.position - 5.,
                    "right" | "up" => this.position + 5.,
                    "home" => 0.,
                    "end" => this.duration,
                    _ => return,
                };
                this.commit_seek(target, cx);
                cx.stop_propagation();
            }))
            .child(
                SeekSlider::new(timeline)
                    .disabled(disabled)
                    .w_full()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(
                        SliderTrack::new(timeline)
                            .disabled(disabled)
                            .relative()
                            .w_full()
                            .h_full()
                            .flex()
                            .items_center()
                            .cursor(if disabled {
                                CursorStyle::Arrow
                            } else {
                                CursorStyle::PointingHand
                            })
                            .child(
                                SliderIndicator::new(timeline)
                                    .relative()
                                    .w_full()
                                    .h(ui_px(4.))
                                    .rounded_full()
                                    .bg(if self.immersive {
                                        foreground.opacity(0.20)
                                    } else {
                                        cx.theme().border
                                    })
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .top_0()
                                            .h_full()
                                            .w(relative(fraction))
                                            .rounded_full()
                                            .bg(primary),
                                    )
                                    // The grab target is 24px; the visible thumb is 10px.
                                    // Endpoint inset keeps the entire thumb inside the control.
                                    .child(
                                        SliderThumb::new(timeline)
                                            .disabled(disabled)
                                            .absolute()
                                            .left(relative(fraction))
                                            .top(ui_px(-10.))
                                            .ml(ui_px(-12.))
                                            .size(ui_px(24.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .cursor(if dragging {
                                                CursorStyle::ClosedHand
                                            } else {
                                                CursorStyle::OpenHand
                                            })
                                            .child(
                                                div()
                                                    .size(px(if dragging { 12. } else { 10. }))
                                                    .rounded_full()
                                                    .bg(foreground),
                                            ),
                                    ),
                            ),
                    ),
            )
            .opacity(if disabled { 0.45 } else { 1. })
            .into_any_element()
    }
    fn refresh_progress(&self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = (
            self.selected.is_some(),
            self.buffering,
            self.failed,
            self.immersive,
            self.duration,
            self.seek_preview,
            self.position,
        );
        for (pane, timeline, visible) in [
            (&self.playback_progress, &self.timeline, !self.immersive),
            (
                &self.mini_progress,
                &self.mini_timeline,
                self.immersive || self.screen == Screen::Lyrics,
            ),
        ] {
            if !visible {
                continue;
            }
            let changed = pane.update(cx, |pane, cx| {
                if pane.snapshot == Some(snapshot) {
                    return false;
                }
                pane.snapshot = Some(snapshot);
                cx.notify();
                true
            });
            if changed && self.seek_preview.is_none() {
                let value = if self.duration > 0. {
                    (self.position / self.duration * 100.).clamp(0., 100.) as f32
                } else {
                    0.
                };
                timeline.update(cx, |slider, cx| slider.set_value(value, window, cx));
            }
        }
    }
    fn progress(&self, _: &mut Context<Self>) -> AnyElement {
        self.mini_progress
            .clone()
            .cached(StyleRefinement::default().w_full().h(ui_px(46.)))
            .into_any_element()
    }
    fn progress_content(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .child(self.seek_bar(true, cx))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .px(ui_px(6.))
                    .text_color(if self.immersive {
                        rgb(0xffffff).opacity(0.65).into()
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(clock(self.seek_preview.unwrap_or(self.position)))
                    .child(clock(self.duration)),
            )
            .into_any_element()
    }
    fn bottom_progress(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(
                div()
                    .w(rems(2.5))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(clock(self.seek_preview.unwrap_or(self.position))),
            )
            .child(div().flex_1().min_w_0().child(self.seek_bar(false, cx)))
            .child(
                div()
                    .w(rems(2.5))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(clock(self.duration)),
            )
            .into_any_element()
    }
    fn bottom_player(&self, window: &Window, _: &mut Context<Self>) -> AnyElement {
        let logical_width = f32::from(window.bounds().size.width) / self.library.ui_scale;
        let compact = logical_width < 1100.;
        div()
            .id("bottom-player")
            .flex()
            .items_center()
            .gap_4()
            .h(rems(4.))
            .px_5()
            .w_full()
            .child(
                self.chrome_view(
                    ChromePart::Identity,
                    StyleRefinement::default()
                        .w(rems(if logical_width < 700. {
                            3.
                        } else if compact {
                            12.
                        } else {
                            15.
                        }))
                        .h(rems(4.)),
                ),
            )
            .child(
                self.chrome_view(
                    ChromePart::Transport,
                    StyleRefinement::default()
                        .w(px(40. + 5.5 * f32::from(window.rem_size())))
                        .h(ui_px(40.)),
                ),
            )
            .child(
                div().flex().items_center().flex_1().min_w_0().child(
                    self.playback_progress
                        .clone()
                        .cached(StyleRefinement::default().w_full().h(ui_px(28.))),
                ),
            )
            .child(
                self.chrome_view(
                    ChromePart::Actions,
                    StyleRefinement::default()
                        .w(rems(if compact {
                            9.
                        } else {
                            19.25
                                + if self.queue.repeat == Repeat::One {
                                    2.
                                } else {
                                    0.
                                }
                        }))
                        .h(rems(4.)),
                ),
            )
            .into_any_element()
    }
    fn sidebar_player(&self, _: &mut Context<Self>) -> AnyElement {
        div()
            .id("sidebar-player")
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .pt_4()
            .pb_3()
            .child(self.chrome_view(
                ChromePart::Identity,
                StyleRefinement::default().w_full().h(ui_px(52.)),
            ))
            .child(
                self.playback_progress
                    .clone()
                    .cached(StyleRefinement::default().w_full().h(ui_px(28.))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .h(ui_px(28.))
                    .child(self.chrome_view(
                        ChromePart::Transport,
                        StyleRefinement::default().w(ui_px(92.)).h(ui_px(28.)),
                    ))
                    .child(self.chrome_view(
                        ChromePart::Actions,
                        StyleRefinement::default().size(ui_px(28.)),
                    )),
            )
            .into_any_element()
    }
    fn bottom_identity(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(track) = &self.selected else {
            return div().into_any_element();
        };
        let logical_width = f32::from(window.bounds().size.width) / self.library.ui_scale;
        let compact = logical_width < 1100.;
        content_button("bottom-identity")
            .flex()
            .items_center()
            .gap_3()
            .when(self.sidebar_visible, |identity| identity.w_full())
            .when(!self.sidebar_visible, |identity| {
                identity
                    .w(rems(if logical_width < 700. {
                        3.
                    } else if compact {
                        12.
                    } else {
                        15.
                    }))
                    .overflow_hidden()
            })
            .min_w_0()
            .h_full()
            .accessibility_label(t("Open now playing"))
            .child(
                div()
                    .size(rems(2.75))
                    .flex_shrink_0()
                    .rounded_lg()
                    .overflow_hidden()
                    .child(self.playing_cover(track, rems(2.75), cx)),
            )
            .child(self.playing_identity(track, cx))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(Panel::Playing, cx)))
            .into_any_element()
    }
    fn bottom_actions(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(track) = &self.selected else {
            return div().into_any_element();
        };
        if self.sidebar_visible {
            return Button::new("sidebar-queue")
                .ghost()
                .small()
                .size(ui_px(28.))
                .icon(icon("list"))
                .tooltip(t("Queue"))
                .accessibility_label(t("Queue"))
                .selected(self.panel == Some(Panel::Queue))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(Panel::Queue, cx)))
                .into_any_element();
        }
        let compact = f32::from(window.bounds().size.width) / self.library.ui_scale < 1100.;
        div()
            .flex()
            .items_center()
            .gap_4()
            .h_full()
            .w_full()
            .child(
                Button::new("mute")
                    .ghost()
                    .small()
                    .icon(icon(if self.library.volume > 0. {
                        "sound-high"
                    } else {
                        "sound-off"
                    }))
                    .tooltip(t("Mute"))
                    .accessibility_label(t("Mute"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.library.volume > 0. {
                            this.muted_volume = this.library.volume;
                            this.library.volume = 0.;
                        } else {
                            this.library.volume = this.muted_volume.max(1.);
                        }
                        this.volume.update(cx, |slider, cx| {
                            slider.set_value(this.library.volume as f32, window, cx)
                        });
                        this.player
                            .command(PlayerCommand::Volume(this.library.volume));
                        this.save();
                        cx.notify();
                    })),
            )
            .when(!compact, |container| {
                container.child(self.volume_control(cx))
            })
            .child(self.like_button(track, "bottom-like", cx))
            .child(
                Button::new("bottom-queue")
                    .ghost()
                    .small()
                    .icon(icon("list"))
                    .tooltip(t("Queue"))
                    .accessibility_label(t("Queue"))
                    .selected(self.panel == Some(Panel::Queue))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_panel(Panel::Queue, cx))),
            )
            .when(!compact, |div| div.child(self.extra_controls(true, cx)))
            .when(compact, |div| {
                div.child(
                    Button::new("compact-fullscreen")
                        .ghost()
                        .small()
                        .icon(icon("expand"))
                        .tooltip(t("Fullscreen player · F11"))
                        .accessibility_label(t("Fullscreen player"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_immersive(window, cx)),
                        ),
                )
            })
            .into_any_element()
    }
    fn volume_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let volume = self.volume.clone();
        div()
            .id("volume-control")
            .w_16()
            .py_2()
            .hoverable_tooltip(move |_, cx| {
                let volume = volume.clone();
                cx.new(|cx| {
                    let subscription = cx.observe(&volume, |_, _, cx| cx.notify());
                    cx.on_release(move |_, _| drop(subscription)).detach();
                    gpui_kit::component::tooltip::Tooltip::element(move |_, cx| {
                        div().child(tr!("Volume: {:.0}%", volume.read(cx).value().start()))
                    })
                })
                .into()
            })
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                cx.stop_propagation();
                let steps = this.volume_scroll.steps(event.delta, Instant::now());
                if steps != 0 {
                    let level = (this.library.volume + f64::from(steps) * 5.).clamp(0., 100.);
                    this.library.volume = level;
                    this.volume
                        .update(cx, |slider, cx| slider.set_value(level as f32, window, cx));
                    this.player.command(PlayerCommand::Volume(level));
                    this.save();
                    cx.notify();
                }
            }))
            .child(Slider::new(&self.volume))
            .into_any_element()
    }

    fn track_row(&self, track: &Track, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let owned = track.clone();
        let queued = self.screen == Screen::Queue;
        let current = if queued {
            self.queue.cursor() == Some(ix)
        } else {
            self.selected
                .as_ref()
                .is_some_and(|selected| selected.id == track.id)
        };
        let collection = matches!(self.screen, Screen::Collection(..));
        let album = collection
            && self
                .collection_seed
                .as_ref()
                .is_some_and(|t| t.kind == ItemKind::Album);
        let row = gpui_kit::div()
            .id(SharedString::from(format!("song-row-{}-{ix}", track.id)))
            .when(self.browse_filter.is_empty() && self.browse_sort == 0 && !self.selecting && (queued || matches!(&self.screen, Screen::Collection(id, _, true) if self.library.playlists.iter().any(|p| p.id == *id && p.smart.is_none())) || matches!(self.screen, Screen::Collection(_, _, false)) && self.page.editable), |row| {
                let screen = self.screen.clone();
                row.on_drag(TrackDrag { track: track.clone(), screen: screen.clone(), index: ix }, |drag, _, _, cx| cx.new(|_| DragPreview(drag.track.title.clone())))
                    .drag_over::<TrackDrag>(|style, _, _, cx| style.border_t_2().border_color(cx.theme().primary))
                    .on_drop(cx.listener(move |this, drag: &TrackDrag, _, cx| this.move_track_to(drag, ix, &screen, cx)))
            })
            .hover(|row| row.bg(cx.theme().muted))
            .flex()
            .items_center()
            .gap_3()
            .h(ui_px(if collection { 58. } else { 64. }))
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_2()
            .rounded_lg()
            .when(collection, |row| {
                row.border_b_1()
                    .border_color(cx.theme().border.opacity(0.45))
            })
            .when(current, |div| div.bg(cx.theme().muted))
            .when(self.selecting, |row| {
                let key = Self::row_key(track);
                row.child(
                    Button::new(SharedString::from(format!("select-row-{ix}")))
                        .ghost()
                        .small()
                        .label(if self.selected_rows.contains(&key) {
                            "☑"
                        } else {
                            "□"
                        })
                        .accessibility_label(tr!("Select {}", track.title))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.selected_rows.remove(&key) {
                                this.selected_rows.insert(key.clone());
                            }
                            this.content_revision += 1;
                            cx.notify();
                        })),
                )
            })
            .child(
                gpui_kit::div()
                    .w_6()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(if current {
                        cx.theme().primary
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(if current {
                        "♫".into()
                    } else {
                        format!("{}", ix + 1)
                    }),
            )
            .child(
                gpui_kit::div().flex_1().min_w_0().child(
                    content_button(SharedString::from(format!("row-{}-{ix}", track.id)))
                        .flex()
                        .items_center()
                        .px_2()
                        .rounded_lg()
                        .h(rems(3.5))
                        .w_full()
                        .min_w_0()
                        .justify_start()
                        .accessibility_label(format!(
                            "{} {}",
                            if track.playable() { t("Play") } else { t("Open") },
                            track.title
                        ))
                        .child(
                            gpui_kit::div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_3()
                                .when(!album, |row| row.child(self.cover(track, rems(2.5), cx)))
                                .child(
                                    gpui_kit::div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w_0()
                                        .gap(ui_px(2.))
                                        .text_left()
                                        .child(
                                            gpui_kit::div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .truncate()
                                                .child(track.title.clone()),
                                        )
                                        .when(!track.artist.trim().is_empty(), |identity| {
                                            identity.child(
                                                gpui_kit::div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .truncate()
                                                    .child(self.artist_link(track, cx)),
                                            )
                                        }),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if queued {
                                this.queue.select_at(ix);
                                this.start(owned.clone(), cx);
                            } else {
                                this.open_track(owned.clone(), cx);
                            }
                        })),
                ),
            )
            .child(
                gpui_kit::div()
                    .w_12()
                    .flex_shrink_0()
                    .text_xs()
                    .text_right()
                    .text_color(cx.theme().muted_foreground)
                    .child(if track.duration > 0 {
                        clock(track.duration as f64)
                    } else {
                        String::new()
                    }),
            )
            .when(track.playable(), |div| {
                div.child(self.like_button(
                    track,
                    SharedString::from(format!("like-{}-{ix}", track.id)),
                    cx,
                ))
            })
            .when(track.playable(), |row| {
                row.child(self.track_menu(track, ix, queued, current, cx))
            });
        // One anchored reveal per navigation, never restarted by scrolling or resizing.
        let started = self.collection_reveal_at;
        if collection && ix < 12 && !cx.reduce_motion() && started.elapsed().as_secs_f32() < 0.4 {
            row.with_animation(
                SharedString::from(format!("track-reveal-{}-{ix}", self.revision)),
                Animation::new(Duration::from_millis(400)),
                move |row, _| {
                    let phase = crate::motion::stagger_phase(started.elapsed().as_secs_f32(), ix);
                    row.relative().top(px(5. * (1. - phase))).opacity(phase)
                },
            )
            .into_any_element()
        } else {
            row.into_any_element()
        }
    }
    fn track_menu(
        &self,
        track: &Track,
        ix: usize,
        queued: bool,
        current: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let owner = cx.weak_entity();
        let track = track.clone();
        let playlist = match &self.screen {
            Screen::Collection(id, _, true)
                if self
                    .library
                    .playlists
                    .iter()
                    .any(|p| p.id == *id && p.smart.is_none()) =>
            {
                Some((id.clone(), false))
            }
            Screen::Collection(id, _, false)
                if self.account_collection_key().is_some() && self.page.editable =>
            {
                Some((id.clone(), true))
            }
            _ => None,
        };
        let last = ix + 1 >= self.visible.len();
        Button::new(SharedString::from(format!("track-menu-{}-{ix}", track.id)))
            .ghost()
            .small()
            .icon(icon("more"))
            .tooltip(t("More song actions"))
            .accessibility_label(t("More song actions"))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _window, _cx| {
                let commands = if queued {
                    vec![
                        (t("Move up"), 3, ix == 0),
                        (t("Move down"), 4, last),
                        (t("Remove from queue"), 5, current),
                    ]
                } else {
                    vec![
                        (t("Play next"), 0, false),
                        (t("Add to queue"), 1, false),
                        (t("Start song radio"), 2, track.kind != ItemKind::Song),
                    ]
                };
                for (label, command, disabled) in commands {
                    let owner = owner.clone();
                    let track = track.clone();
                    menu = menu.item(PopupMenuItem::new(label).disabled(disabled).on_click(
                        move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                match command {
                                    0 | 1 => {
                                        this.queue.enqueue(track.clone(), command == 0);
                                        this.notice = None;
                                    }
                                    2 => {
                                        this.navigate(Screen::Radio(track.id.clone()), cx);
                                        this.request(
                                            |generation| {
                                                Request::Radio(generation, track.id.clone())
                                            },
                                            cx,
                                        );
                                    }
                                    3 => this.queue.move_at(ix, -1),
                                    4 => this.queue.move_at(ix, 1),
                                    _ => {
                                        this.queue.remove_at(ix);
                                    }
                                }
                                if command != 2 {
                                    this.invalidate_collection();
                                    this.save();
                                }
                                this.right_content.update(cx, |_, cx| cx.notify());
                                self_refresh(this, cx);
                            });
                        },
                    ));
                }
                if playlist.is_some() {
                    for (label, delta) in [(t("Move earlier"), -1isize), (t("Move later"), 1isize)]
                    {
                        let owner = owner.clone();
                        let track = track.clone();
                        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            let _ = owner
                                .update(cx, |this, cx| this.move_playlist_track(&track, delta, cx));
                        }));
                    }
                }
                if track.kind == ItemKind::Song {
                    let url = raag::links::Link::Track(track.id.clone()).public_url();
                    menu = menu.item(PopupMenuItem::new(t("Copy music link")).on_click(
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                        },
                    ));
                }
                if track.playable() {
                    let download_owner = owner.clone();
                    let download_track = track.clone();
                    menu = menu.item(
                        PopupMenuItem::new(t("Download for offline"))
                            .disabled(track.kind != ItemKind::Song)
                            .on_click(move |_, _, cx| {
                                let _ = download_owner.update(cx, |this, cx| {
                                    this.download_tracks(vec![download_track.clone()], cx)
                                });
                            }),
                    );
                    let hide_owner = owner.clone();
                    let hide_track = track.clone();
                    menu = menu.item(PopupMenuItem::new(t("Hide from recommendations")).on_click(
                        move |_, _, cx| {
                            let _ = hide_owner.update(cx, |this, cx| {
                                if !this.library.hidden.contains(&hide_track.id) {
                                    this.library.hidden.push(hide_track.id.clone());
                                }
                                this.save();
                                this.refresh_visible();
                                cx.notify();
                            });
                        },
                    ));
                    if track.kind == ItemKind::Song {
                        let dislike_owner = owner.clone();
                        let disliked = track.clone();
                        menu =
                            menu.item(PopupMenuItem::new(t("Dislike on YouTube Music")).on_click(
                                move |_, _, cx| {
                                    let _ = dislike_owner.update(cx, |this, cx| {
                                        this.account_edit(
                                            Edit::Dislike {
                                                track: disliked.clone(),
                                            },
                                            cx,
                                        )
                                    });
                                },
                            ));
                    }
                    for (label, token) in [
                        (t("Save to YouTube library"), track.library_add.clone()),
                        (
                            t("Remove from YouTube library"),
                            track.library_remove.clone(),
                        ),
                    ] {
                        if let Some(token) = token {
                            let owner = owner.clone();
                            menu =
                                menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        this.account_edit(
                                            Edit::Feedback {
                                                token: token.clone(),
                                            },
                                            cx,
                                        )
                                    });
                                }));
                        }
                    }
                    let remove_owner = owner.clone();
                    let remove_id = track.id.clone();
                    menu = menu.item(PopupMenuItem::new(t("Remove offline download")).on_click(
                        move |_, _, cx| {
                            let _ = remove_owner.update(cx, |this, _| {
                                this.downloads.submit(crate::downloads::Job::Remove(
                                    Some(remove_id.clone()),
                                    false,
                                ));
                            });
                        },
                    ));
                }
                if track.playable() {
                    let owner = owner.clone();
                    let track = track.clone();
                    menu =
                        menu.separator()
                            .item(PopupMenuItem::new(t("Add to playlist…")).on_click(
                                move |_, window, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        this.choose_destination(vec![track.clone()], window, cx)
                                    });
                                },
                            ));
                }
                if let Some((playlist, remote)) = &playlist {
                    let playlist = playlist.clone();
                    let remote = *remote;
                    let track = track.clone();
                    let owner = owner.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new(t("Remove from playlist"))
                            .disabled(remote && track.set_video_id.is_none())
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    if remote {
                                        this.account_edit(
                                            Edit::Remove {
                                                playlist: playlist.clone(),
                                                track: track.clone(),
                                            },
                                            cx,
                                        );
                                        return;
                                    }
                                    this.library.remove_from_playlist(&playlist, &track.id);
                                    this.navigation_revision =
                                        this.navigation_revision.wrapping_add(1);
                                    this.save();
                                    self_refresh(this, cx);
                                });
                            }),
                    );
                }
                menu
            })
    }
    fn empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let (title, detail) = match self.screen {
            Screen::Liked => (
                t("Keep the songs you love."),
                t("Tap the heart beside a song to find it here."),
            ),
            Screen::Recents => (
                t("Your listening story starts here."),
                t("Play a song and it will appear in your history."),
            ),
            Screen::Local => (
                t("Your files. Your favourites."),
                t("Add a folder or audio files from this computer."),
            ),
            Screen::Queue => (
                t("A little quiet, for now."),
                t("Play a song or add tracks to the queue."),
            ),
            Screen::Search => (
                t("Find your next favourite."),
                t("Search songs, artists, albums and playlists above."),
            ),
            _ => (
                t("There's room for more music."),
                t("Add songs to this collection or sync from your profile menu."),
            ),
        };
        gpui_kit::div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .flex_1()
            .gap_4()
            .p_8()
            .child(
                icon("music-double-note")
                    .size_12()
                    .text_color(cx.theme().primary),
            )
            .child(
                gpui_kit::div()
                    .text_xl()
                    .font_family(raag::i18n::font())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if self.loading {
                        t("Finding your music…")
                    } else {
                        title
                    }),
            )
            .child(
                gpui_kit::div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail),
            )
            .when(self.screen == Screen::Local, |div| {
                div.child(
                    Button::new("import-empty")
                        .label(t("Add music…"))
                        .icon(icon("plus"))
                        .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
                )
            })
            .into_any_element()
    }
    fn track_list(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.visible.is_empty() {
            if self.screen == Screen::Offline {
                return div().p_5().text_sm().text_color(cx.theme().muted_foreground)
                .child(t("Download a song or collection from its menu. Recently cached songs also appear here.")).into_any_element();
            }
            return self.empty_state(cx);
        }
        let entity = cx.entity();
        self.scroll
            .bind_list(uniform_list(
                "tracks",
                self.visible.len(),
                move |range, _, cx| {
                    entity.update(cx, |this, cx| {
                        range
                            .filter_map(|ix| {
                                this.visible
                                    .get(ix)
                                    .map(|track| div().pb_2().child(this.track_row(track, ix, cx)))
                            })
                            .collect::<Vec<_>>()
                    })
                },
            ))
            .flex_1()
            .min_h_0()
            .into_any_element()
    }
    fn listing_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = gpui_kit::div()
            .flex()
            .flex_col()
            .size_full()
            .px_8()
            .pb_6()
            .gap_4()
            .child(self.title(t("Play something that stays with you."), cx));
        body = body.child(self.collection_tools(cx));
        if self.screen == Screen::Offline {
            body = body.child(self.offline_controls(cx));
        }
        let mut actions = gpui_kit::div().flex().items_center().gap_2();
        if self.screen == Screen::Search {
            for (ix, label) in [
                t("All"),
                t("Songs"),
                t("Albums"),
                t("Artists"),
                t("Playlists"),
            ]
            .into_iter()
            .enumerate()
            {
                actions = actions.child(
                    Button::new(("filter", ix))
                        .ghost()
                        .label(label)
                        .selected(self.search_filter == ix)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.search_filter = ix;
                            let query = this.query.read(cx).value().to_string();
                            let filter = if ix == 0 {
                                None
                            } else {
                                Some(
                                    [
                                        ItemKind::Song,
                                        ItemKind::Song,
                                        ItemKind::Album,
                                        ItemKind::Artist,
                                        ItemKind::Playlist,
                                    ][ix]
                                        .clone(),
                                )
                            };
                            this.catalog = Arc::new(vec![]);
                            this.refresh_visible();
                            this.request(|id| Request::Search(id, query, filter), cx);
                        })),
                );
            }
        } else {
            actions = actions.child(
                Button::new("play-list")
                    .primary()
                    .map(playback_depth)
                    .icon(icon("play"))
                    .label(t("Play"))
                    .disabled(!self.visible.iter().any(|track| track.playable()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(track) =
                            this.visible.iter().find(|track| track.playable()).cloned()
                        {
                            this.open_track(track, cx);
                        }
                    })),
            );
        }
        if self.screen == Screen::Local {
            actions = actions.child(
                Button::new("add-local")
                    .ghost()
                    .icon(icon("plus"))
                    .label(t("Add music…"))
                    .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
            );
        }
        if self.screen == Screen::Queue {
            actions = actions.child(
                Button::new("clear-upcoming")
                    .ghost()
                    .label(t("Clear upcoming"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.queue.clear_upcoming();
                        this.invalidate_collection();
                        this.save();
                        this.right_content.update(cx, |_, cx| cx.notify());
                        self_refresh(this, cx);
                    })),
            );
        }
        if let Screen::Collection(playlist_id, _, true) = &self.screen {
            let id = playlist_id.clone();
            actions = actions.child(
                Button::new("add-playing")
                    .ghost()
                    .label(t("Add playing song"))
                    .disabled(self.selected.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(track) = this.selected.clone() {
                            if let Err(error) = this.library.add_to_playlist(&id, track) {
                                this.notice = Some(error.to_string());
                            }
                            this.navigation_revision = this.navigation_revision.wrapping_add(1);
                            this.save();
                            self_refresh(this, cx);
                        }
                    })),
            );
        }
        if let Screen::Collection(id, name, true) = &self.screen {
            let renamed = name.clone();
            let exported = id.clone();
            let removed = id.clone();
            actions = actions
                .child(
                    Button::new("rename-playlist")
                        .ghost()
                        .label(t("Rename"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.playlist_editor_remote = false;
                            this.editing_playlist = true;
                            this.editing_description = false;
                            this.content_revision += 1;
                            this.playlist_name.update(cx, |input, cx| {
                                input.set_value(renamed.clone(), window, cx);
                                input.focus(window, cx);
                            });
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("export-playlist")
                        .ghost()
                        .label(t("Export…"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.export_playlist(exported.clone(), cx)
                        })),
                )
                .child(
                    Button::new("remove-playlist")
                        .ghost()
                        .label(t("Remove…"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.remove_playlist(removed.clone(), window, cx)
                        })),
                );
        }
        body = body.child(actions);
        if self.editing_playlist {
            body = body.child(self.playlist_editor(cx));
        }
        body = body.child(self.track_list(cx));
        if self.continuation.is_some() {
            body = body.child(self.pagination_status(cx));
        }
        body.into_any_element()
    }
    fn lyrics_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(track) = &self.selected else {
            return self.empty_state(cx);
        };
        let mut lyrics = gpui_kit::div()
            .id("lyrics-scroll")
            .flex()
            .flex_col()
            .gap_6()
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .p_6();
        if self.lyrics_loading {
            lyrics = lyrics.child(
                gpui_kit::div()
                    .text_xl()
                    .font_family(raag::i18n::font())
                    .child(t("Finding the words…")),
            );
        } else if let Some(error) = &self.lyrics_error {
            lyrics = lyrics.child(
                gpui_kit::div()
                    .text_color(cx.theme().muted_foreground)
                    .child(error.clone()),
            );
        } else {
            let current = self.lyrics.iter().rposition(|line| {
                line.seconds
                    .is_some_and(|seconds| seconds <= self.lyric_time())
            });
            for (ix, line) in self.lyrics.iter().enumerate() {
                let seconds = line.seconds;
                lyrics = lyrics.child(
                    content_button(("line", ix))
                        .h_auto()
                        .py_2()
                        .justify_center()
                        .text_center()
                        .w_full()
                        .text_2xl()
                        .font_family(raag::i18n::font())
                        .child(self.lyric_label(line, Some(ix) == current, cx.theme().foreground))
                        .text_color(if Some(ix) == current {
                            cx.theme().foreground
                        } else {
                            cx.theme().muted_foreground
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(seconds) = seconds {
                                this.commit_seek(
                                    (seconds
                                        - this
                                            .selected
                                            .as_ref()
                                            .map_or(0., |t| this.library.lyrics.offset(&t.id)))
                                    .max(0.),
                                    cx,
                                );
                            }
                        })),
                );
            }
        }
        gpui_kit::div()
            .flex()
            .items_stretch()
            .size_full()
            .px_8()
            .pb_8()
            .gap_8()
            .child(
                gpui_kit::div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .w_72()
                    .flex_shrink_0()
                    .pt_6()
                    .child(
                        artwork_frame(cx)
                            .size(rems(18.))
                            .flex_shrink_0()
                            .child(self.cover(
                                track,
                                rems(18. - 2. / f32::from(cx.theme().font_size)),
                                cx,
                            )),
                    )
                    .child(
                        gpui_kit::div()
                            .text_2xl()
                            .font_family(raag::i18n::font())
                            .font_weight(FontWeight::BOLD)
                            .child(track.title.clone()),
                    )
                    .child(
                        gpui_kit::div()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.artist_link(track, cx)),
                    )
                    .child(self.progress(cx))
                    .child(self.basic_controls("immersive", cx)),
            )
            .child(lyrics)
            .into_any_element()
    }
    fn setting_row(
        &self,
        label: impl Into<SharedString>,
        control: impl IntoElement,
        cx: &App,
    ) -> Div {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.5))
            .child(div().text_sm().child(label.into()))
            .child(div().flex_shrink_0().child(control))
    }
    fn settings_section(&self, title: &'static str, description: &'static str, cx: &App) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .border_t_1()
            .border_color(cx.theme().border)
            .pt_6()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_family(raag::i18n::font())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(description),
                    ),
            )
    }
    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.library.close_to_tray && self.system.is_some() && crate::system::hide(window, cx) {
            false
        } else {
            self.backend.flush(self.snapshot());
            true
        }
    }
    fn set_start_at_login(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.startup_busy {
            return;
        }
        self.startup_busy = true;
        self.content_revision += 1;
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let result = cx
                .background_spawn(async move { crate::system::set_start_at_login(enabled) })
                .await;
            let _ = owner.update(cx, |this, cx| {
                this.startup_busy = false;
                match result {
                    Ok(()) => {
                        this.library.start_at_login = enabled;
                        this.save();
                    }
                    Err(error) => {
                        this.notice = Some(tr!("Could not update startup preference: {error}"))
                    }
                }
                this.content_revision += 1;
                cx.notify();
            });
        })
        .detach();
    }
    fn settings(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let tray_available = self.system.is_some() && crate::system::hide_supported(window);
        let account = self
            .settings_section("YouTube Music", t("Your account and library"), cx)
            .child(div().text_sm().child(self.auth.account().map_or_else(
                || t("Sign in to bring your playlists and liked songs here.").into(),
                |account| format!("{} {}", account.name, account.handle),
            )))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("youtube-sign-in")
                            .primary()
                            .label(if self.auth.busy() {
                                t("Signing in…")
                            } else if self.auth.account().is_some() {
                                t("Switch account")
                            } else {
                                t("Sign in to YouTube Music")
                            })
                            .disabled(self.auth.busy())
                            .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                    )
                    .when(self.auth.can_cancel(), |row| {
                        row.child(
                            Button::new("cancel-youtube-login")
                                .ghost()
                                .label(t("Cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auth.cancel();
                                    this.content_revision += 1;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("forget-cookies")
                            .ghost()
                            .label(t("Sign out"))
                            .disabled(
                                !self.auth.can_sign_out() && self.library.cookies_path.is_empty(),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.sign_out(cx))),
                    ),
            );
        let playback = self
            .settings_section(t("Playback"), t("Make the queue your own"), cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(div().w_24().text_sm().child(t("Volume")))
                    .child(
                        div()
                            .flex_1()
                            .max_w(rems(20.))
                            .child(Slider::new(&self.volume)),
                    )
                    .child(
                        div()
                            .w_12()
                            .text_sm()
                            .text_right()
                            .child(format!("{:.0}%", self.library.volume)),
                    ),
            )
            .child(
                self.setting_row(
                    t("Continue with similar songs"),
                    Switch::new("autoplay-setting")
                        .accessibility_label(t("Continue with similar songs"))
                        .checked(self.library.autoplay)
                        .on_change(cx.listener(|this, value, _, cx| {
                            this.library.autoplay = *value;
                            this.content_revision += 1;
                            this.save();
                            cx.notify();
                        })),
                    cx,
                ),
            );
        let system = self.settings_section(t("System"), t("Ready when you are"), cx)
            .child(self.setting_row(t("Music links"), Button::new("register-links").label(t("Enable Cassette links")).on_click(cx.listener(|this, _, _, cx| this.register_links(cx))), cx))
            .child(self.setting_row(t("Keep Cassette in the tray when closing"), Switch::new("close-to-tray").accessibility_label(t("Keep Cassette in the tray when closing"))
                .checked(self.library.close_to_tray).disabled(!tray_available).on_change(cx.listener(|this, value, _, cx| {
                    this.library.close_to_tray = *value; this.content_revision += 1; this.save(); cx.notify();
                })), cx))
            .child(self.setting_row(t("Start with your computer, in the tray"), Switch::new("start-at-login").accessibility_label(t("Start with your computer, in the tray"))
                .checked(self.library.start_at_login).disabled(!tray_available || self.startup_busy)
                .on_change(cx.listener(|this, value, _, cx| this.set_start_at_login(*value, cx))), cx))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(if self.media.is_some() { t("System media controls are connected.") } else { t("System media controls are unavailable in this session.") }))
            .child(div().text_sm().text_color(cx.theme().muted_foreground)
                .child(if tray_available { t("Open the tray menu to return to Cassette or quit completely.") }
                    else { t("Tray hiding is unavailable on this desktop. On Linux, use an X11 session with a system tray.") }));
        let advanced = div().flex().flex_col().items_start().gap_4()
            .child(Button::new("advanced-settings").ghost()
                .icon(icon(if self.advanced_settings { "nav-arrow-down" } else { "nav-arrow-right" }))
                .label(t("Advanced"))
                .on_click(cx.listener(|this, _, _, cx| { this.advanced_settings = !this.advanced_settings; this.content_revision += 1; cx.notify(); })))
            .when(self.advanced_settings, |section| section.child(
                div().flex().flex_col().gap_4().pl_3()
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(t("Playback tools are detected automatically. Change paths only when using a custom installation.")))
                    .child(div().flex().flex_col().gap_2().text_sm().child(t("yt-dlp executable")).child(Input::new(&self.ytdlp_path)))
                    .child(div().child(Button::new("save-tools").label(t("Apply paths")).on_click(cx.listener(|this, _, _, cx| {
                        this.library.ytdlp_path = this.ytdlp_path.read(cx).value().to_string();
                        this.reset_player(cx); this.save(); this.content_revision += 1;
                        this.notice = Some(t("Playback paths updated").into()); cx.notify();
                    }))))
                    .child(div().flex().flex_wrap().gap_2()
                        .child(Button::new("choose-cookies").ghost().label(t("Import cookies…")).disabled(self.auth.busy())
                            .on_click(cx.listener(|this, _, _, cx| this.choose_cookies(cx))))
                        .child(Button::new("cookie-help").ghost().label(t("Cookie import help")).on_click(|_, _, cx| cx.open_url("https://github.com/yt-dlp/yt-dlp/wiki/Extractors#exporting-youtube-cookies"))))));
        let mut tabs = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .pb_4()
            .border_b_1()
            .border_color(cx.theme().border);
        for (index, label) in [
            "Appearance",
            "Playback",
            "Lyrics",
            "Offline storage",
            "YouTube Music",
            "System",
        ]
        .into_iter()
        .enumerate()
        {
            tabs = tabs.child(
                Button::new(("settings-category", index))
                    .ghost()
                    .label(t(label))
                    .selected(self.settings_category == index)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_category = index;
                        this.scroll.reset();
                        this.content_revision += 1;
                        cx.notify();
                    })),
            );
        }
        let panel = match self.settings_category {
            0 => div()
                .flex()
                .flex_col()
                .gap_6()
                .child(self.appearance_settings(window, cx))
                .child(self.localization_settings(cx)),
            1 => div()
                .flex()
                .flex_col()
                .gap_6()
                .child(playback)
                .child(
                    self.settings_section(t("Sound"), t("Output, tone and transitions"), cx)
                        .child(self.audio_controls(window, cx)),
                )
                .child(
                    self.settings_section(
                        t("Discovery"),
                        t("Recommendations and listening history"),
                        cx,
                    )
                    .child(self.discovery_controls(cx)),
                ),
            2 => self
                .settings_section(t("Lyrics"), t("Providers, timing and text"), cx)
                .child(self.lyrics_preferences(cx)),
            3 => self
                .settings_section(
                    t("Offline storage"),
                    t("Pinned downloads and temporary cache"),
                    cx,
                )
                .child(self.offline_controls(cx)),
            4 => account,
            _ => div()
                .flex()
                .flex_col()
                .gap_6()
                .child(system)
                .child(advanced),
        };
        let content = div()
            .flex()
            .flex_col()
            .gap_6()
            .w_full()
            .max_w(rems(55.))
            .child(panel)
            .child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .pt_5()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(icon("cassette"))
                            .child("Cassette"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr!(
                                "Version {} · Built with Rust and GPUI · Icons by Iconoir",
                                env!("CARGO_PKG_VERSION")
                            )),
                    ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .px_8()
                    .pt_5()
                    .pb_3()
                    .child(
                        div()
                            .w_full()
                            .max_w(rems(55.))
                            .flex()
                            .flex_col()
                            .gap_6()
                            .child(self.page_heading(
                                t("Settings"),
                                t("Make yourself at home."),
                                cx,
                            ))
                            .child(tabs),
                    ),
            )
            .child(
                self.scroll
                    .bind(div().id("settings-scroll"))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .justify_center()
                            .w_full()
                            .px_8()
                            .pb_8()
                            .child(content),
                    ),
            )
            .into_any_element()
    }
}
fn self_refresh(this: &mut Raag, cx: &mut Context<Raag>) {
    this.refresh_visible();
    cx.notify();
}

impl Render for Raag {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sidebar_visible = self.sidebar_expanded(window);
        let _domain = raag::diagnostics::enter(raag::diagnostics::Domain::Ui);
        let view = cx.entity_id();
        self.images.update(cx, |cache, _| cache.begin_view(view));
        self.refresh_progress(window, cx);
        self.refresh_chrome(window, cx);
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        if self.immersive {
            self.images.update(cx, |cache, _| {
                for view in [
                    self.content.entity_id(),
                    self.navigation.entity_id(),
                    self.right_content.entity_id(),
                ] {
                    cache.release_view(view, window);
                }
            });
            return image_cache(self.images.clone())
                .size_full()
                .child(self.immersive_player(window, cx))
                .into_any_element();
        }
        let expanded = self.sidebar_expanded(window);
        let bottom = self.selected.is_some() && !expanded;
        let right_target = if self.panel.is_some() {
            f32::from(rems(self.right_sidebar_rems(window)).to_pixels(window.rem_size())) + 12.
        } else {
            0.
        };
        let running = self.shell_motion.sample(
            [
                if expanded {
                    256. * raag::i18n::scale()
                } else {
                    0.
                },
                right_target,
                if bottom {
                    80. * raag::i18n::scale()
                } else {
                    0.
                },
            ],
            cx.reduce_motion() || !window.is_window_active(),
            Instant::now(),
        );
        let [left_width, right_width, player_height] = self.shell_motion.dimensions();
        self.images.update(cx, |cache, _| {
            if left_width == 0. {
                cache.release_view(self.navigation.entity_id(), window);
            }
            if right_width == 0. {
                cache.release_view(self.right_content.entity_id(), window);
            }
        });
        if running && !self.shell_frame_pending {
            self.shell_frame_pending = true;
            let owner = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.shell_frame_pending = false;
                    cx.notify();
                });
            });
        }
        let revision = self.content_revision;
        self.content.update(cx, |pane, cx| {
            if pane.revision != revision {
                pane.revision = revision;
                cx.notify();
            }
        });
        let content = self
            .content
            .clone()
            .cached(StyleRefinement::default().size_full());
        // Position updates repaint the player, not every saved playlist.
        let navigation_revision = self.navigation_revision;
        let navigation_expanded = true;
        self.navigation.update(cx, |pane, cx| {
            if pane.revision != navigation_revision
                || pane.screen.as_ref() != Some(&self.screen)
                || pane.expanded != navigation_expanded
            {
                pane.revision = navigation_revision;
                pane.screen = Some(self.screen.clone());
                pane.expanded = navigation_expanded;
                cx.notify();
            }
        });
        let navigation = self
            .navigation
            .clone()
            .cached(StyleRefinement::default().w_full().flex_1().min_h_0());
        let shell = gpui_kit::div()
            .relative()
            .key_context("Raag")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .line_height(relative(1.4))
            .p_3()
            .pt_9()
            .pb_2()
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
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.query.update(cx, |input, cx| {
                    input.focus(window, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &SignIn, _, cx| this.sign_in(cx)))
            .on_action(cx.listener(|this, _: &SignOut, _, cx| this.sign_out(cx)))
            .on_action(cx.listener(|this, _: &CancelSignIn, _, cx| {
                this.auth.cancel();
                this.content_revision += 1;
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &OpenSettings, _, cx| this.navigate(Screen::Settings, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenRecap, _, cx| this.navigate(Screen::Recap, cx)))
            .on_action(cx.listener(|this, _: &OpenLocal, _, cx| this.navigate(Screen::Local, cx)))
            .on_action(
                cx.listener(|this, _: &OpenOffline, _, cx| this.navigate(Screen::Offline, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowRecent, _, cx| this.navigate(Screen::Recents, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowQueue, _, cx| this.navigate(Screen::Queue, cx)))
            .on_action(cx.listener(|this, _: &ShowLyrics, _, cx| this.navigate(Screen::Lyrics, cx)))
            .on_action(cx.listener(|this, _: &GoHome, _, cx| this.navigate(Screen::Home, cx)))
            .on_action(cx.listener(|this, _: &GoBack, _, cx| this.history_navigation(false, cx)))
            .on_action(cx.listener(|this, _: &CloseOverlay, window, cx| {
                if this.immersive {
                    this.exit_immersive(window, cx);
                    return;
                }
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
                this.editing_playlist = false;
                if this.panel.take().is_some() {
                    this.content_revision += 1;
                    cx.notify();
                } else {
                    this.navigate(Screen::Home, cx);
                }
                window.focus(&this.focus, cx);
            }))
            .on_action(
                cx.listener(|this, _: &Fullscreen, window, cx| this.toggle_immersive(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ImportFiles, _, cx| this.import(cx)))
            .child(
                gpui_kit::div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h_8()
                    .child(
                        gpui_kit::component::TitleBar::new()
                            .on_close_window(cx.listener(|this, _, window, cx| {
                                if this.should_close(window, cx) {
                                    window.remove_window();
                                }
                            }))
                            .bg(cx.theme().sidebar)
                            .border_b_0()
                            .h_8()
                            .w_full()
                            .when(cfg!(target_os = "windows"), |bar| {
                                bar.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(
                                            img("raag/app-icon.png")
                                                .size_4()
                                                .object_fit(ObjectFit::Contain),
                                        )
                                        .child("Cassette"),
                                )
                            }),
                    ),
            )
            .child(
                gpui_kit::div()
                    .flex()
                    .items_stretch()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .h_full()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .mr(px(
                                12. * (left_width / (256. * raag::i18n::scale())).clamp(0., 1.)
                            ))
                            .min_w_0()
                            .w(px(left_width))
                            .when(left_width > 0., |pane| {
                                pane.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .w(ui_px(256.))
                                        .h_full()
                                        .min_h_0()
                                        .child(navigation)
                                        .when(expanded && self.selected.is_some(), |sidebar| {
                                            sidebar.child(self.sidebar_player(cx))
                                        }),
                                )
                            }),
                    )
                    .child(
                        gpui_kit::div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .bg(cx.theme().background)
                            .rounded_3xl()
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .h_full()
                                    .flex_shrink_0()
                                    .w(px(self.content_width(window)))
                                    .child(self.chrome_view(
                                        ChromePart::Toolbar,
                                        StyleRefinement::default().w_full().h(ui_px(72.)),
                                    ))
                                    .child(
                                        gpui_kit::div()
                                            .relative()
                                            .flex_1()
                                            .min_h_0()
                                            .min_w_0()
                                            // Cached paint includes inherited opacity. Keep the
                                            // page opaque; animate individual art/text instead.
                                            .child(content)
                                            // A paint-only fade keeps scroll content continuous at the
                                            // header edge without adding hitboxes or per-frame captures.
                                            .when(
                                                !matches!(self.screen, Screen::Collection(..)),
                                                |page| {
                                                    page.child(
                                                        div()
                                                            .absolute()
                                                            .top_0()
                                                            .left_0()
                                                            .right_0()
                                                            .h(ui_px(28.))
                                                            .bg(linear_gradient(
                                                                180.,
                                                                linear_color_stop(
                                                                    cx.theme().background,
                                                                    0.,
                                                                ),
                                                                linear_color_stop(
                                                                    cx.theme()
                                                                        .background
                                                                        .opacity(0.),
                                                                    1.,
                                                                ),
                                                            )),
                                                    )
                                                },
                                            )
                                            .when_some(self.notice.as_ref(), |page, message| {
                                                page.child(
                                                    div()
                                                        .absolute()
                                                        .bottom_4()
                                                        .left_8()
                                                        .max_w(ui_px(400.))
                                                        .mr_8()
                                                        .border_1()
                                                        .border_color(cx.theme().border)
                                                        .flex()
                                                        .items_center()
                                                        .gap_3()
                                                        .p_3()
                                                        .rounded_lg()
                                                        .shadow_lg()
                                                        .bg(cx.theme().popover)
                                                        .text_color(cx.theme().popover_foreground)
                                                        .text_sm()
                                                        .line_height(relative(1.4))
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .child(message.clone()),
                                                        )
                                                        .child(
                                                            Button::new("dismiss-notice")
                                                                .ghost()
                                                                .small()
                                                                .icon(icon("xmark"))
                                                                .tooltip(t("Dismiss"))
                                                                .accessibility_label(t(t(
                                                                    "Dismiss message",
                                                                )))
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.notice = None;
                                                                        cx.notify();
                                                                    },
                                                                )),
                                                        ),
                                                )
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h_full()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .when(right_width > 0., |pane| {
                                pane.child(
                                    div()
                                        .w(rems(self.right_sidebar_rems(window)))
                                        .h_full()
                                        .ml_3()
                                        .child(
                                            self.right_content
                                                .clone()
                                                .cached(StyleRefinement::default().size_full()),
                                        ),
                                )
                            })
                            .min_w_0()
                            .w(px(right_width.max(0.))),
                    ),
            )
            .child(
                gpui_kit::div()
                    .w_full()
                    .flex_shrink_0()
                    .overflow_hidden()
                    // Padding belongs to the clipped inner content. Putting it on
                    // this shrinking box imposes an 8px floor, then a late jump.
                    .when(bottom && player_height > 0.1, |div| {
                        div.child(
                            gpui_kit::div()
                                .h(ui_px(80.))
                                .pt_2()
                                .child(self.bottom_player(window, cx)),
                        )
                    })
                    .min_h_0()
                    .h(px(player_height.max(0.))),
            )
            .into_any_element();
        image_cache(self.images.clone())
            .size_full()
            .child(shell)
            .into_any_element()
    }
}

impl Drop for Raag {
    fn drop(&mut self) {
        self.auth.shutdown();
        // Flush the last partial session before process shutdown.
        if self.writable {
            self.backend.flush(self.snapshot());
        }
    }
}

fn div_placeholder(source: ImageSource, opacity: f32, radius: Pixels) -> Div {
    div().absolute().inset_0().opacity(opacity).child(
        img(source)
            .size_full()
            .rounded(radius)
            .object_fit(ObjectFit::Cover),
    )
}
