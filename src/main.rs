#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use gpui_kit::*;
extern crate gpui_kit as gpui;
mod account_sync;
mod artwork_queue;
mod auth_controller;
mod downloads;
mod images;
mod jobs;
mod login_helper;
mod media;
mod motion;
mod service;
mod system;
mod theme;
mod ui;
#[cfg(feature = "allocation-profile")]
#[global_allocator]
static ALLOCATOR: raag::diagnostics::TrackedAllocator = raag::diagnostics::TrackedAllocator;
fn main() {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--login-helper")
    {
        std::process::exit(login_helper::main());
    }
    let background = std::env::args_os().any(|arg| arg == "--background");
    let quit = std::env::args_os().any(|arg| arg == "--quit");
    if let Err(error) = raag::auth::storage::prepare_data_dir() {
        eprintln!("Cassette could not protect local data: {error}");
        return;
    }
    let instance = match system::Instance::acquire(background, quit) {
        Ok(Some(instance)) => instance,
        Ok(None) => return,
        Err(error) => {
            eprintln!("Cassette could not start: {error:#}");
            return;
        }
    };
    if let Err(error) = raag::runtime::prepare() {
        eprintln!("Cassette could not prepare bundled playback tools: {error:#}");
        return;
    }
    if let Err(error) = raag::auth::storage::clear_abandoned_exports() {
        eprintln!("Cassette could not clear old playback credentials: {error}");
        return;
    }
    #[cfg(feature = "allocation-profile")]
    raag::diagnostics::start_reporter();
    let links = instance.link_sender();
    let application = gpui_kit::application().with_assets(theme::Assets);
    application.on_open_urls(move |urls| {
        for url in urls.into_iter().take(8) {
            if let Ok(link) = raag::links::Link::parse(&url) {
                let _ = links.try_send(system::Command::Open(link));
            }
        }
    });
    application.run(move |cx| {
        gpui_kit::init(cx);
        if let Err(error) = cx.text_system().add_fonts(vec![
            std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/Inter[opsz,wght].ttf")),
            std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/Manrope[wght].ttf")),
        ]) {
            eprintln!("Could not register bundled fonts: {error}");
        }
        ui::bind_keys(cx);
        let options = WindowOptions {
            show: !background,
            focus: !background,
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1440.), px(920.)),
                cx,
            ))),
            window_min_size: Some(size(px(860.), px(640.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Cassette".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            app_owns_titlebar_drag: true,
            window_decorations: Some(WindowDecorations::Client),
            app_id: Some("app.raag.desktop".into()),
            ..Default::default()
        };
        if let Err(error) = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| ui::Raag::new(window, instance, cx))
        }) {
            eprintln!("Cassette could not open a window: {error:#}");
            cx.quit();
        }
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if !background {
            cx.activate(true);
        }
    });
}
