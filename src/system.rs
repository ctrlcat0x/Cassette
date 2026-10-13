//! Native tray and login-start integration, kept outside rendering code.
use anyhow::{Context as _, Result};
use gpui_kit::{App, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::mpsc::{self, Receiver};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

#[derive(Clone)]
pub enum Command {
    Show,
    TogglePlayback,
    Quit,
    Open(raag::links::Link),
}
pub struct System {
    _icon: TrayIcon,
    events: Receiver<Command>,
    labels: [MenuItem; 3],
}
impl System {
    pub fn new() -> Result<Self> {
        let menu = Menu::new();
        let labels = [
            MenuItem::with_id("cassette-show", raag::i18n::t("Open Cassette"), true, None),
            MenuItem::with_id("cassette-play", raag::i18n::t("Play / pause"), true, None),
            MenuItem::with_id("cassette-quit", raag::i18n::t("Quit Cassette"), true, None),
        ];
        menu.append(&labels[0])?;
        menu.append(&labels[1])?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&labels[2])?;
        let image =
            image::load_from_memory(include_bytes!("../assets/cassette-32.png"))?.to_rgba8();
        let (width, height) = image.dimensions();
        let icon = Icon::from_rgba(image.into_raw(), width, height)?;
        let icon = TrayIconBuilder::new()
            .with_id("cassette")
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip("Cassette")
            .with_menu_on_left_click(false)
            .build()?;
        let (sender, events) = mpsc::sync_channel(8);
        let clicks = sender.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = clicks.try_send(Command::Show);
            }
        }));
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = match event.id.as_ref() {
                "cassette-show" => Command::Show,
                "cassette-play" => Command::TogglePlayback,
                "cassette-quit" => Command::Quit,
                _ => return,
            };
            let _ = sender.try_send(command);
        }));
        Ok(Self {
            _icon: icon,
            events,
            labels,
        })
    }
    pub fn refresh_language(&self) {
        for (item, label) in
            self.labels
                .iter()
                .zip(["Open Cassette", "Play / pause", "Quit Cassette"])
        {
            item.set_text(raag::i18n::t(label));
        }
    }
    pub fn poll(&self) -> Option<Command> {
        self.events.try_recv().ok()
    }
}

pub fn hide_supported(window: &Window) -> bool {
    matches!(
        HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()),
        Ok(RawWindowHandle::Win32(_)
            | RawWindowHandle::AppKit(_)
            | RawWindowHandle::Xcb(_)
            | RawWindowHandle::Xlib(_))
    )
}
pub fn hide(window: &mut Window, _cx: &App) -> bool {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return false;
    };
    match handle.as_raw() {
        #[cfg(target_os = "windows")]
        RawWindowHandle::Win32(handle) => {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                    handle.hwnd.get() as _,
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
                );
            }
            true
        }
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(_) => {
            _cx.hide();
            true
        }
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xcb(handle) => map_x11(handle.window.get(), false).is_ok(),
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xlib(handle) => map_x11(handle.window as u32, false).is_ok(),
        _ => false,
    }
}
pub fn show(window: &mut Window, cx: &App) {
    if let Ok(handle) = HasWindowHandle::window_handle(window) {
        match handle.as_raw() {
            #[cfg(target_os = "windows")]
            RawWindowHandle::Win32(handle) => unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                    handle.hwnd.get() as _,
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW,
                );
            },
            #[cfg(target_os = "linux")]
            RawWindowHandle::Xcb(handle) => {
                let _ = map_x11(handle.window.get(), true);
            }
            #[cfg(target_os = "linux")]
            RawWindowHandle::Xlib(handle) => {
                let _ = map_x11(handle.window as u32, true);
            }
            _ => {}
        }
    }
    cx.activate(true);
    window.activate_window();
}
#[cfg(target_os = "linux")]
fn map_x11(window: u32, show: bool) -> Result<()> {
    use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};
    let (connection, _) = x11rb::connect(None)?;
    if show {
        connection.map_window(window)?.check()?;
    } else {
        connection.unmap_window(window)?.check()?;
    }
    connection.flush()?;
    Ok(())
}
pub fn set_start_at_login(enabled: bool) -> Result<()> {
    let executable = std::env::current_exe().context("Could not locate Cassette")?;
    // AppImage's current executable lives in a temporary mount; register its
    // stable outer file instead so login startup survives the next reboot.
    #[cfg(target_os = "linux")]
    let executable = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .unwrap_or(executable);
    let launch = auto_launch::AutoLaunchBuilder::new()
        .set_app_name("Cassette")
        .set_app_path(&executable.to_string_lossy())
        .set_args(&["--background"])
        .set_macos_launch_mode(auto_launch::MacOSLaunchMode::LaunchAgent)
        .build()?;
    if enabled {
        launch.enable()?;
    } else {
        launch.disable()?;
    }
    Ok(())
}

/// Explicit settings action; installed bundles also declare the app URL scheme.
pub fn register_links() -> Result<()> {
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "windows")]
    {
        let entries = [
            (
                "HKCU\\Software\\Classes\\cassette",
                "/ve",
                "",
                "URL:Cassette Protocol".to_owned(),
            ),
            (
                "HKCU\\Software\\Classes\\cassette",
                "/v",
                "URL Protocol",
                String::new(),
            ),
            (
                "HKCU\\Software\\Classes\\cassette\\shell\\open\\command",
                "/ve",
                "",
                format!("\"{}\" \"%1\"", executable.display()),
            ),
        ];
        for (key, kind, name, value) in entries {
            use std::os::windows::process::CommandExt;
            let mut command = std::process::Command::new("reg.exe");
            command.args(["add", key, kind]);
            if kind == "/v" {
                command.arg(name);
            }
            let status = command
                .args(["/d", &value, "/f"])
                .creation_flags(0x08000000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            anyhow::ensure!(status.success(), "Could not register Cassette links.");
        }
    }
    #[cfg(target_os = "linux")]
    {
        let executable = std::env::var_os("APPIMAGE")
            .map(std::path::PathBuf::from)
            .unwrap_or(executable);
        let path = executable.to_string_lossy();
        anyhow::ensure!(
            !path.contains(['\n', '\r', '%', '`', '$']),
            "Unsupported executable path."
        );
        let applications = directories::BaseDirs::new()
            .context("Home directory unavailable")?
            .data_local_dir()
            .join("applications");
        std::fs::create_dir_all(&applications)?;
        std::fs::write(
            applications.join("cassette.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=Cassette\nExec=\"{}\" %u\nIcon=cassette\nTerminal=false\nMimeType=x-scheme-handler/cassette;\n",
                path.replace('\\', "\\\\").replace('"', "\\\"")
            ),
        )?;
        anyhow::ensure!(
            std::process::Command::new("xdg-mime")
                .args(["default", "cassette.desktop", "x-scheme-handler/cassette"])
                .status()?
                .success(),
            "Could not register Cassette links."
        );
    }
    #[cfg(target_os = "macos")]
    {
        let _ = executable;
    }
    Ok(())
}

/// One process owns the player, tray and login-start entry. Reopening the app
/// asks that process to show its existing window instead of allocating another.
pub struct Instance {
    sender: std::sync::mpsc::SyncSender<Command>,
    _lock: std::fs::File,
    events: Receiver<Command>,
    closing: std::sync::Arc<std::sync::atomic::AtomicBool>,
    address: std::net::SocketAddr,
    worker: Option<std::thread::JoinHandle<()>>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct InstanceAddress {
    port: u16,
    token: String,
}
impl Instance {
    pub fn link_sender(&self) -> std::sync::mpsc::SyncSender<Command> {
        self.sender.clone()
    }
    pub fn acquire(background: bool, quit: bool) -> Result<Option<Self>> {
        Self::acquire_in(&raag::library::data_dir(), background, quit)
    }
    fn acquire_in(
        directory: &std::path::Path,
        background: bool,
        quit: bool,
    ) -> Result<Option<Self>> {
        use std::{
            fs::OpenOptions,
            io::{Read, Write},
            net::{TcpListener, TcpStream},
        };
        std::fs::create_dir_all(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("cassette.lock"))?;
        let address_path = directory.join("cassette-instance.json");
        let link = std::env::args().find_map(|arg| raag::links::Link::parse(&arg).ok());
        if lock.try_lock().is_err() {
            if background && !quit {
                return Ok(None);
            }
            // The owner may still be publishing its endpoint during startup.
            for _ in 0..20 {
                if let Ok(file) = std::fs::File::open(&address_path)
                    && let Ok(address) = raag::resource::json::<InstanceAddress>(file, 4096)
                    && let Ok(mut stream) = TcpStream::connect_timeout(
                        &std::net::SocketAddr::from(([127, 0, 0, 1], address.port)),
                        std::time::Duration::from_millis(200),
                    )
                {
                    stream.set_write_timeout(Some(std::time::Duration::from_millis(200)))?;
                    if let Some(link) = &link {
                        writeln!(stream, "open {} {}", address.token, link.url())?;
                    } else {
                        writeln!(
                            stream,
                            "{} {}",
                            if quit { "quit" } else { "show" },
                            address.token
                        )?;
                    }
                    return Ok(None);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            anyhow::bail!("Cassette is already starting. Try opening it again in a moment.");
        }
        if quit {
            return Ok(None);
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let token = format!("{:032x}", rand::random::<u128>());
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        serde_json::to_writer(
            options.open(address_path)?,
            &InstanceAddress {
                port: address.port(),
                token: token.clone(),
            },
        )?;
        let (sender, events) = mpsc::sync_channel(8);
        if let Some(link) = link {
            let _ = sender.try_send(Command::Open(link));
        }
        let closing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = closing.clone();
        let incoming = sender.clone();
        let worker = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let Ok(mut stream) = stream else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
                let mut buffer = [0u8; 2304];
                // A local launch sends one bounded line; fragmented writes are valid.
                let mut length = 0;
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
                while length < buffer.len() && std::time::Instant::now() < deadline {
                    match stream.read(&mut buffer[length..]) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            length += count;
                            if buffer[..length].contains(&b'\n') {
                                break;
                            }
                        }
                    }
                }
                let request = String::from_utf8_lossy(&buffer[..length]);
                let Some((command, supplied)) = request.trim().split_once(' ') else {
                    continue;
                };
                let (supplied, argument) = supplied.split_once(' ').unwrap_or((supplied, ""));
                if supplied != token {
                    continue;
                }
                let command = match command {
                    "show" => Command::Show,
                    "quit" => Command::Quit,
                    "open" => match raag::links::Link::parse(argument) {
                        Ok(link) => Command::Open(link),
                        Err(_) => continue,
                    },
                    _ => continue,
                };
                let _ = incoming.try_send(command);
            }
        });
        Ok(Some(Self {
            sender,
            _lock: lock,
            events,
            closing,
            address,
            worker: Some(worker),
        }))
    }
    pub fn poll(&self) -> Option<Command> {
        self.events.try_recv().ok()
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        self.closing
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = std::net::TcpStream::connect_timeout(
            &self.address,
            std::time::Duration::from_millis(200),
        );
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
