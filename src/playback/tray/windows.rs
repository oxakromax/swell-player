//! Windows: an icon in the notification area (`Shell_NotifyIconW`), with
//! a popup menu on right click, through user32/shell32 directly.
//!
//! The icon's messages go to a hidden window of Swell's own, on the main
//! thread. GTK's main loop dispatches them: GDK's event source polls the
//! thread's message queue (`G_WIN32_MSG_HANDLE`) and its dispatch runs
//! `PeekMessageW(NULL, …)` + `DispatchMessageW` for every window of the
//! thread, not only GDK's, so [`window_proc`] runs from the main loop like
//! any other GTK callback. The window is a hidden top-level one rather than
//! message-only (`HWND_MESSAGE`): those miss broadcasts, and the shell
//! broadcasts "TaskbarCreated" when Explorer restarts, the cue to add the
//! icon again.
//!
//! The popup menu runs its own modal loop (`TrackPopupMenuEx`), during
//! which GLib's sources wait: the menu is up for a moment only.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;

use anyhow::{Result, bail};
use tokio::sync::mpsc::UnboundedSender;

use super::TrayState;
use crate::config;
use crate::playback::controls::Command;

type Hwnd = *mut c_void;
type Hicon = *mut c_void;
type Hmenu = *mut c_void;
type Hinstance = *mut c_void;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

const WM_NULL: u32 = 0x0000;
const WM_NCDESTROY: u32 = 0x0082;
const WM_CONTEXTMENU: u32 = 0x007B;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_USER: u32 = 0x0400;
const WM_APP: u32 = 0x8000;
/// The message the icon's events come in.
const WM_TRAY: u32 = WM_APP + 1;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETVERSION: u32 = 4;
const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_SHOWTIP: u32 = 0x80;
const NOTIFYICON_VERSION_4: u32 = 4;
const NIN_SELECT: u32 = WM_USER;
const NIN_KEYSELECT: u32 = WM_USER + 1;
/// The icon's id among this window's (it has one).
const ICON_ID: u32 = 1;

const MF_STRING: u32 = 0x0000;
const MF_GRAYED: u32 = 0x0001;
const MF_SEPARATOR: u32 = 0x0800;
const TPM_LEFTALIGN: u32 = 0x0000;
const TPM_RIGHTALIGN: u32 = 0x0008;
const TPM_RIGHTBUTTON: u32 = 0x0002;
const TPM_NONOTIFY: u32 = 0x0080;
const TPM_RETURNCMD: u32 = 0x0100;

const IMAGE_ICON: u32 = 1;
const LR_DEFAULTCOLOR: u32 = 0;
const IDI_APPLICATION: usize = 32512;
/// The executable's icon, as `build.rs` links it (`1 ICON`).
const EXE_ICON: usize = 1;
const SM_CXSMICON: i32 = 49;
const SM_CYSMICON: i32 = 50;
const SM_MENUDROPALIGNMENT: i32 = 40;

const GWLP_USERDATA: i32 = -21;
const WS_OVERLAPPED: u32 = 0;
const WS_EX_TOOLWINDOW: u32 = 0x80;
const MSGFLT_ALLOW: u32 = 1;
const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

const CLASS_NAME: &str = "SwellTrayIcon";

/// The menu's items.
const ID_SHOW: u32 = 1;
const ID_PLAY_PAUSE: u32 = 2;
const ID_PREVIOUS: u32 = 3;
const ID_NEXT: u32 = 4;
const ID_QUIT: u32 = 5;

/// `WNDCLASSEXW`.
#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    window_proc: Option<WndProc>,
    class_extra: i32,
    window_extra: i32,
    instance: Hinstance,
    icon: Hicon,
    cursor: *mut c_void,
    background: *mut c_void,
    menu_name: *const u16,
    class_name: *const u16,
    small_icon: Hicon,
}

/// `GUID`.
#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// `NOTIFYICONDATAW`, the Vista+ layout. `shellapi.h` packs it to 1 byte
/// on 32-bit Windows only.
#[cfg_attr(target_pointer_width = "64", repr(C))]
#[cfg_attr(target_pointer_width = "32", repr(C, packed(1)))]
struct NotifyIconDataW {
    size: u32,
    hwnd: Hwnd,
    id: u32,
    flags: u32,
    callback_message: u32,
    icon: Hicon,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    /// The union of `uTimeout` and `uVersion`.
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: Guid,
    balloon_icon: Hicon,
}

/// `POINT`.
#[repr(C)]
#[derive(Default)]
struct Point {
    x: i32,
    y: i32,
}

// The layouts of the Windows SDK's headers.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<WndClassExW>() == 80);
    assert!(size_of::<NotifyIconDataW>() == 976);
    assert!(std::mem::offset_of!(NotifyIconDataW, tip) == 40);
    assert!(std::mem::offset_of!(NotifyIconDataW, version) == 816);
    assert!(std::mem::offset_of!(NotifyIconDataW, guid) == 952);
    assert!(std::mem::offset_of!(NotifyIconDataW, balloon_icon) == 968);
};
#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(size_of::<WndClassExW>() == 48);
    assert!(size_of::<NotifyIconDataW>() == 956);
};

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn UnregisterClassW(class_name: *const u16, instance: Hinstance) -> i32;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: Hmenu,
        instance: Hinstance,
        param: *const c_void,
    ) -> Hwnd;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn DefWindowProcW(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
    #[cfg_attr(target_pointer_width = "32", link_name = "SetWindowLongW")]
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    #[cfg_attr(target_pointer_width = "32", link_name = "GetWindowLongW")]
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn ChangeWindowMessageFilterEx(
        hwnd: Hwnd,
        message: u32,
        action: u32,
        filter: *mut c_void,
    ) -> i32;
    fn PostMessageW(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
    fn CreatePopupMenu() -> Hmenu;
    fn AppendMenuW(menu: Hmenu, flags: u32, id: usize, item: *const u16) -> i32;
    fn SetMenuDefaultItem(menu: Hmenu, item: u32, by_position: u32) -> i32;
    fn TrackPopupMenuEx(
        menu: Hmenu,
        flags: u32,
        x: i32,
        y: i32,
        hwnd: Hwnd,
        params: *const c_void,
    ) -> i32;
    fn DestroyMenu(menu: Hmenu) -> i32;
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    fn GetCursorPos(point: *mut Point) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn LoadImageW(
        instance: Hinstance,
        name: *const u16,
        kind: u32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> *mut c_void;
    fn LoadIconW(instance: Hinstance, name: *const u16) -> Hicon;
    fn DestroyIcon(icon: Hicon) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn Shell_NotifyIconW(message: u32, data: *const NotifyIconDataW) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hinstance;
    fn GetLastError() -> u32;
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// `text` in a fixed buffer of `N` UTF-16 units, NUL included: cut after
/// a whole character (never inside a surrogate pair), with an ellipsis.
fn fixed<const N: usize>(text: &str) -> [u16; N] {
    let mut buffer = [0; N];
    let fits = text.encode_utf16().take_while(|&unit| unit != 0).count() < N;
    // Room for the ellipsis and the NUL when cut.
    let limit = if fits { N - 1 } else { N - 2 };
    let mut len = 0;
    for c in text.chars().take_while(|&c| c != '\0') {
        if len + c.len_utf16() > limit {
            break;
        }
        len += c.encode_utf16(&mut buffer[len..]).len();
    }
    if !fits {
        buffer[len] = '…' as u16;
    }
    buffer
}

/// A menu label: `&` would underline the next letter.
fn label(text: &str) -> Vec<u16> {
    wide(&text.replace('&', "&&"))
}

/// The signed x and y coordinates packed in a message parameter
/// (`GET_X_LPARAM`, `GET_Y_LPARAM`).
fn point(packed: usize) -> (i32, i32) {
    let x = (packed & 0xFFFF) as u16 as i16;
    let y = ((packed >> 16) & 0xFFFF) as u16 as i16;
    (x.into(), y.into())
}

/// The icon the notification area shows.
struct Icon {
    handle: Hicon,
    /// Loaded for Swell, rather than a shared system icon.
    owned: bool,
}

impl Icon {
    /// The executable's icon at the small-icon size, else Windows' generic
    /// application icon.
    fn load() -> Self {
        // Safety: these take no pointers but the module handle (null: this
        // executable) and an integer resource id (MAKEINTRESOURCE), and
        // return null on failure.
        let handle = unsafe {
            LoadImageW(
                GetModuleHandleW(ptr::null()),
                EXE_ICON as *const u16,
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_DEFAULTCOLOR,
            )
        };
        if !handle.is_null() {
            return Self {
                handle,
                owned: true,
            };
        }
        log::debug!("tray: the executable has no icon, using the generic one");
        // Safety: a predefined icon (null instance, integer id); shared,
        // never destroyed.
        let handle = unsafe { LoadIconW(ptr::null_mut(), IDI_APPLICATION as *const u16) };
        Self {
            handle,
            owned: false,
        }
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        if self.owned {
            // Safety: loaded by `LoadImageW` without LR_SHARED, so ours to
            // destroy, once; the notification area no longer shows it.
            unsafe { DestroyIcon(self.handle) };
        }
    }
}

/// The icon in the notification area. Dropping it removes the icon.
pub struct Backend {
    hwnd: Hwnd,
    inner: Rc<Inner>,
}

/// What the window procedure needs. The window holds one reference (in
/// `GWLP_USERDATA`) until `WM_NCDESTROY`, the backend another.
struct Inner {
    hwnd: Hwnd,
    commands: UnboundedSender<Command>,
    state: RefCell<TrayState>,
    icon: Icon,
    /// The shell's "TaskbarCreated" message (0 if it has none).
    taskbar_created: u32,
    /// Whether the shell took NOTIFYICON_VERSION_4 (which changes the
    /// events).
    version_4: Cell<bool>,
    menu_open: Cell<bool>,
}

impl Backend {
    pub fn new(commands: UnboundedSender<Command>, state: &TrayState) -> Result<Self> {
        // Safety: null is this executable's module, which is never unloaded.
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        let class_name = wide(CLASS_NAME);
        let class = WndClassExW {
            size: size_of::<WndClassExW>() as u32,
            style: 0,
            window_proc: Some(window_proc),
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: ptr::null_mut(),
            cursor: ptr::null_mut(),
            background: ptr::null_mut(),
            menu_name: ptr::null(),
            class_name: class_name.as_ptr(),
            small_icon: ptr::null_mut(),
        };
        // Safety: `class` is complete and its strings outlive the call (the
        // class keeps a copy). A class left by an earlier icon still
        // registered is the same class.
        if unsafe { RegisterClassExW(&class) } == 0 {
            // Safety: no preconditions.
            let error = unsafe { GetLastError() };
            if error != ERROR_CLASS_ALREADY_EXISTS {
                bail!("cannot register the tray icon's window class (error {error})");
            }
        }
        let title = wide(config::APP_NAME);
        // Safety: the class is registered for this module and the strings
        // live through the call. The window is never shown.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            // Safety: no preconditions.
            let error = unsafe { GetLastError() };
            bail!("cannot create the tray icon's window (error {error})");
        }
        let taskbar_created = wide("TaskbarCreated");
        // Safety: a NUL-terminated string that lives through the call.
        let taskbar_created = unsafe { RegisterWindowMessageW(taskbar_created.as_ptr()) };
        if taskbar_created != 0 {
            // Let the shell's message through even when Swell runs elevated.
            // Safety: our window; the filter information is optional.
            unsafe {
                ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, ptr::null_mut())
            };
        }
        let inner = Rc::new(Inner {
            hwnd,
            commands,
            state: RefCell::new(state.clone()),
            icon: Icon::load(),
            taskbar_created,
            version_4: Cell::new(false),
            menu_open: Cell::new(false),
        });
        // Safety: our window, on this thread. The pointer is one strong
        // reference, which `window_proc` takes back on WM_NCDESTROY; until
        // then the `Inner` stays alive.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::into_raw(inner.clone()) as isize) };
        inner.add();
        Ok(Self { hwnd, inner })
    }

    pub fn update(&self, state: &TrayState) {
        // The menu is made from the state when it opens; only the tooltip
        // is the shell's.
        let old = self.inner.state.replace(state.clone());
        if old.tooltip() != state.tooltip() {
            self.inner.notify(NIM_MODIFY, NIF_TIP | NIF_SHOWTIP);
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.inner.notify(NIM_DELETE, 0);
        // Safety: our window, destroyed once, on the thread that made it.
        // Its WM_NCDESTROY releases its reference to `Inner`.
        if unsafe { DestroyWindow(self.hwnd) } == 0 {
            log::debug!("tray: cannot destroy the icon's window");
        }
        let class_name = wide(CLASS_NAME);
        // Safety: a NUL-terminated string and this module. It fails, without
        // harm, while another window of the class exists.
        unsafe { UnregisterClassW(class_name.as_ptr(), GetModuleHandleW(ptr::null())) };
    }
}

impl Inner {
    /// Adds the icon to the notification area, with the version 4
    /// behaviour if the shell has it.
    fn add(&self) {
        let flags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        // A shell busy at login can time out yet add the icon.
        if !self.notify(NIM_ADD, flags) && !self.notify(NIM_MODIFY, flags) {
            log::info!("tray: the notification area is not there yet");
            return;
        }
        let mut data = self.data(0);
        data.version = NOTIFYICON_VERSION_4;
        // Safety: a complete NOTIFYICONDATAW with its size.
        let version_4 = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) } != 0;
        self.version_4.set(version_4);
        log::debug!("tray: icon added (version 4: {version_4})");
    }

    /// Sends the icon's current data to the shell; false if it refused.
    fn notify(&self, message: u32, flags: u32) -> bool {
        let data = self.data(flags);
        // Safety: a complete NOTIFYICONDATAW with its size; the icon lives
        // as long as `self`. No borrow of `self` is held: the shell's
        // cross-process call may dispatch sent messages to `window_proc`.
        unsafe { Shell_NotifyIconW(message, &data) != 0 }
    }

    fn data(&self, flags: u32) -> NotifyIconDataW {
        let tooltip = self.state.borrow().tooltip();
        NotifyIconDataW {
            size: size_of::<NotifyIconDataW>() as u32,
            hwnd: self.hwnd,
            id: ICON_ID,
            flags,
            callback_message: WM_TRAY,
            icon: self.icon.handle,
            tip: fixed(&tooltip),
            state: 0,
            state_mask: 0,
            info: [0; 256],
            version: 0,
            info_title: [0; 64],
            info_flags: 0,
            guid: Guid {
                data1: 0,
                data2: 0,
                data3: 0,
                data4: [0; 8],
            },
            balloon_icon: ptr::null_mut(),
        }
    }

    fn send(&self, command: Command) {
        // Only fails while the playback object goes away.
        let _ = self.commands.send(command);
    }

    /// Handles the window's messages; `None` for the default handling.
    fn handle(&self, message: u32, wparam: usize, lparam: isize) -> Option<isize> {
        if message == WM_TRAY {
            let event = (lparam as usize & 0xFFFF) as u32;
            if self.version_4.get() {
                match event {
                    NIN_SELECT | NIN_KEYSELECT => self.send(Command::Raise),
                    // The anchor, also for the keyboard (Shift+F10).
                    WM_CONTEXTMENU => {
                        let (x, y) = point(wparam);
                        self.popup(x, y);
                    }
                    _ => {}
                }
            } else {
                match event {
                    WM_LBUTTONUP => self.send(Command::Raise),
                    WM_RBUTTONUP | WM_CONTEXTMENU => {
                        let mut cursor = Point::default();
                        // Safety: a valid POINT to write.
                        unsafe { GetCursorPos(&mut cursor) };
                        self.popup(cursor.x, cursor.y);
                    }
                    _ => {}
                }
            }
            return Some(0);
        }
        if self.taskbar_created != 0 && message == self.taskbar_created {
            // Explorer restarted, with an empty notification area.
            self.add();
            return Some(0);
        }
        None
    }

    /// Shows the menu at (`x`, `y`) and carries out the chosen item.
    fn popup(&self, x: i32, y: i32) {
        if self.menu_open.replace(true) {
            return;
        }
        // A copy: the menu's loop runs `window_proc` again (and could
        // reach `update`), so nothing stays borrowed.
        let state = self.state.borrow().clone();
        let (play_pause_label, play_pause) = state.play_pause();
        let grayed = |enabled: bool| if enabled { MF_STRING } else { MF_GRAYED };
        let items = [
            (MF_STRING, ID_SHOW, Some(&*state.labels.show)),
            (MF_SEPARATOR, 0, None),
            (
                grayed(state.can_play),
                ID_PLAY_PAUSE,
                Some(play_pause_label),
            ),
            (
                grayed(state.can_go_previous),
                ID_PREVIOUS,
                Some(&*state.labels.previous),
            ),
            (
                grayed(state.can_go_next),
                ID_NEXT,
                Some(&*state.labels.next),
            ),
            (MF_SEPARATOR, 0, None),
            (MF_STRING, ID_QUIT, Some(&*state.labels.quit)),
        ];

        // Safety: no preconditions; null on failure.
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            self.menu_open.set(false);
            return log::debug!("tray: cannot create the menu");
        }
        for (flags, id, text) in items {
            let text = text.map(label);
            let text = text.as_ref().map_or(ptr::null(), |text| text.as_ptr());
            // Safety: our live menu; the label is NUL-terminated (the menu
            // copies it) or null for a separator.
            unsafe { AppendMenuW(menu, flags, id as usize, text) };
        }
        // Safety: our live menu and one of its item ids.
        unsafe { SetMenuDefaultItem(menu, ID_SHOW, 0) };

        // Safety: our window, menu and plain integers. Without being in the
        // foreground the menu would not close on a click elsewhere, and the
        // WM_NULL after it makes it close on the next click as well (both
        // documented for notification-area menus).
        let chosen = unsafe {
            let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 {
                TPM_RIGHTALIGN
            } else {
                TPM_LEFTALIGN
            };
            SetForegroundWindow(self.hwnd);
            let flags = align | TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY;
            let chosen = TrackPopupMenuEx(menu, flags, x, y, self.hwnd, ptr::null());
            PostMessageW(self.hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            chosen
        };
        self.menu_open.set(false);

        let command = match u32::try_from(chosen) {
            Ok(ID_SHOW) => Command::Raise,
            Ok(ID_PLAY_PAUSE) => play_pause,
            Ok(ID_PREVIOUS) => Command::Previous,
            Ok(ID_NEXT) => Command::Next,
            Ok(ID_QUIT) => Command::Quit,
            _ => return, // dismissed
        };
        self.send(command);
    }
}

/// The window procedure of the icon's window. Runs on the main thread,
/// from GTK's main loop (see the module's documentation) or from the
/// popup menu's loop.
unsafe extern "system" fn window_proc(
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    // Safety: `hwnd` is the window this is called for.
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Inner;
    if pointer.is_null() {
        // Before `Backend::new` stores it, or after WM_NCDESTROY.
        // Safety: the arguments as Windows gave them.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if message == WM_NCDESTROY {
        // Safety: our window. The pointer came from `Rc::into_raw` in
        // `Backend::new` and is cleared before its reference is dropped,
        // so it is released once.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Rc::from_raw(pointer));
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
    }
    // A reference of its own for this call: the window may be destroyed,
    // and the backend dropped, while it runs (the menu's loop).
    // Safety: the window's reference keeps the `Rc` alive (it is released
    // on WM_NCDESTROY only, never during this call), so its count can be
    // raised.
    let inner = unsafe {
        Rc::increment_strong_count(pointer);
        Rc::from_raw(pointer)
    };
    match inner.handle(message, wparam, lparam) {
        Some(result) => result,
        // Safety: the arguments as Windows gave them.
        None => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(units: &[u16]) -> String {
        let len = units.iter().position(|&unit| unit == 0).unwrap();
        String::from_utf16(&units[..len]).unwrap()
    }

    #[test]
    fn short_text_is_copied_whole() {
        assert_eq!(
            text(&fixed::<128>("Swell\nSong — Artist")),
            "Swell\nSong — Artist"
        );
        assert_eq!(text(&fixed::<8>("1234567")), "1234567");
    }

    #[test]
    fn long_text_is_cut_on_a_character() {
        assert_eq!(text(&fixed::<8>("12345678")), "123456…");
        // A surrogate pair that would straddle the cut is left out whole.
        let cut = fixed::<8>("12345🎵🎵");
        assert_eq!(text(&cut), "12345…");
        assert_eq!(cut[7], 0);
        let cut = fixed::<128>(&"🎵".repeat(100));
        assert_eq!(text(&cut), format!("{}…", "🎵".repeat(63)));
    }

    #[test]
    fn labels_keep_their_ampersands() {
        assert_eq!(label("Rock & Roll"), wide("Rock && Roll"));
    }

    #[test]
    fn coordinates_are_signed() {
        assert_eq!(point(0x0002_0001), (1, 2));
        assert_eq!(point(0xFFFE_FFFF), (-1, -2));
    }

    #[test]
    fn the_window_lets_go_when_dropped() {
        let (commands, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let backend = Backend::new(commands, &TrayState::default()).unwrap();
        let inner = backend.inner.clone();
        assert_eq!(Rc::strong_count(&inner), 3);
        let state = TrayState {
            track: Some("Song — Artist".to_owned()),
            ..TrayState::default()
        };
        backend.update(&state);
        drop(backend);
        assert_eq!(Rc::strong_count(&inner), 1);
        // And again, with the class registered before.
        let (commands, _receiver) = tokio::sync::mpsc::unbounded_channel();
        drop(Backend::new(commands, &state).unwrap());
    }
}
