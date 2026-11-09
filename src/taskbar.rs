//! Native thumbnail controls, owned by the UI thread and its window.
use crate::system::Command;
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::Cell,
    sync::mpsc::{self, Receiver, SyncSender},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::{
            Shell::{
                DefSubclassProc, ITaskbarList3, RemoveWindowSubclass, SetWindowSubclass, THB_FLAGS,
                THB_ICON, THB_TOOLTIP, THBF_DISABLED, THBF_ENABLED, THUMBBUTTON, TaskbarList,
            },
            WindowsAndMessaging::{
                CreateIconFromResourceEx, DestroyIcon, HICON, IMAGE_FLAGS, RegisterWindowMessageW,
                WM_COMMAND, WM_NCDESTROY,
            },
        },
    },
    core::w,
};

const SUBCLASS: usize = 0xca55;
const PREVIOUS: u32 = 4001;
const TOGGLE: u32 = 4002;
const NEXT: u32 = 4003;

struct Icon(HICON);
impl Icon {
    fn new(bytes: &[u8]) -> Option<Self> {
        let offset = u32::from_le_bytes(bytes.get(18..22)?.try_into().ok()?) as usize;
        // Bundled ICO files contain one 32-bit image. Windows owns a copy of
        // the resource bits; this wrapper releases the resulting icon handle.
        unsafe {
            CreateIconFromResourceEx(bytes.get(offset..)?, true, 0x30000, 20, 20, IMAGE_FLAGS(0))
        }
        .ok()
        .map(Self)
    }
}
impl Drop for Icon {
    fn drop(&mut self) {
        let _ = unsafe { DestroyIcon(self.0) };
    }
}
struct State {
    hwnd: HWND,
    taskbar: ITaskbarList3,
    created: u32,
    added: Cell<bool>,
    destroyed: Cell<bool>,
    playing: Cell<bool>,
    enabled: Cell<bool>,
    icons: [Icon; 4],
    sender: SyncSender<Command>,
}
impl State {
    fn buttons(&self) -> [THUMBBUTTON; 3] {
        let playing = self.playing.get();
        [
            (PREVIOUS, 0, "Previous"),
            (
                TOGGLE,
                if playing { 2 } else { 1 },
                if playing { "Pause" } else { "Play" },
            ),
            (NEXT, 3, "Next"),
        ]
        .map(|(id, icon, label)| {
            let mut button = THUMBBUTTON {
                dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
                iId: id,
                hIcon: self.icons[icon].0,
                dwFlags: if self.enabled.get() {
                    THBF_ENABLED
                } else {
                    THBF_DISABLED
                },
                ..Default::default()
            };
            for (to, from) in button
                .szTip
                .iter_mut()
                .take(259)
                .zip(raag::i18n::t(label).encode_utf16())
            {
                *to = from;
            }
            button
        })
    }
    fn refresh(&self) {
        if self.destroyed.get() {
            return;
        }
        let buttons = self.buttons();
        unsafe {
            if self.added.get() {
                let _ = self.taskbar.ThumbBarUpdateButtons(self.hwnd, &buttons);
            } else if self.taskbar.ThumbBarAddButtons(self.hwnd, &buttons).is_ok() {
                self.added.set(true);
            }
        }
    }
}
pub struct Taskbar {
    state: Box<State>,
    pub events: Receiver<Command>,
    previous: Option<(bool, bool, String)>,
}
impl Taskbar {
    pub fn new(window: &Window) -> Option<Self> {
        let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
        else {
            return None;
        };
        let taskbar: ITaskbarList3 =
            unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }.ok()?;
        unsafe { taskbar.HrInit() }.ok()?;
        let (sender, events) = mpsc::sync_channel(16);
        let mut state = Box::new(State {
            hwnd: HWND(handle.hwnd.get() as _),
            taskbar,
            created: unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) },
            added: Cell::new(false),
            destroyed: Cell::new(false),
            playing: Cell::new(false),
            enabled: Cell::new(false),
            icons: [
                Icon::new(include_bytes!("../assets/icons/taskbar-previous.ico"))?,
                Icon::new(include_bytes!("../assets/icons/taskbar-play.ico"))?,
                Icon::new(include_bytes!("../assets/icons/taskbar-pause.ico"))?,
                Icon::new(include_bytes!("../assets/icons/taskbar-next.ico"))?,
            ],
            sender,
        });
        if state.created == 0
            || !unsafe {
                SetWindowSubclass(
                    state.hwnd,
                    Some(subclass),
                    SUBCLASS,
                    (&mut *state as *mut State) as usize,
                )
            }
            .as_bool()
        {
            return None;
        }
        state.refresh();
        Some(Self {
            state,
            events,
            previous: None,
        })
    }
    pub fn update(&mut self, enabled: bool, playing: bool) {
        if self
            .previous
            .as_ref()
            .is_some_and(|(old_enabled, old_playing, language)| {
                *old_enabled == enabled
                    && *old_playing == playing
                    && language == raag::i18n::t("Play")
            })
        {
            return;
        }
        let key = (enabled, playing, raag::i18n::t("Play").to_owned());
        if self.previous.as_ref() != Some(&key) {
            self.state.enabled.set(enabled);
            self.state.playing.set(playing);
            self.state.refresh();
            self.previous = Some(key);
        }
    }
}
impl Drop for Taskbar {
    fn drop(&mut self) {
        if !self.state.destroyed.get() {
            unsafe {
                let _ = RemoveWindowSubclass(self.state.hwnd, Some(subclass), SUBCLASS);
            }
        }
    }
}
unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    // The boxed state stays at this address until the subclass is removed.
    let state = unsafe { &*(data as *const State) };
    if message == state.created {
        state.added.set(false);
        state.refresh();
    } else if message == WM_COMMAND && (wparam.0 >> 16) & 0xffff == 0x1800 {
        let command = match wparam.0 as u32 & 0xffff {
            PREVIOUS => Some(Command::Previous),
            TOGGLE => Some(Command::TogglePlayback),
            NEXT => Some(Command::Next),
            _ => None,
        };
        if let Some(command) = command {
            let _ = state.sender.try_send(command);
            return LRESULT(0);
        }
    } else if message == WM_NCDESTROY {
        state.destroyed.set(true);
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS);
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
