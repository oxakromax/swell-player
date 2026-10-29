//! The tray icon on macOS: a status item in the menu bar, with the app's
//! symbolic icon as a template image and a menu of its own.
//!
//! AppKit through the Objective-C runtime (objc 0.2, manual reference
//! counting). Everything runs on the main thread, which GTK's macOS backend
//! shares with the NSApplication event loop, so the menu's actions arrive
//! there too. The items' target is an object of a small class declared
//! here (`SwellTrayTarget`), which maps the chosen item's tag to a
//! [`Command`] and sends it on the channel it points to.

// objc 0.2's `sel!` expands to `cfg_attr(feature = "cargo-clippy", …)`,
// a feature this crate does not have.
#![allow(unexpected_cfgs)]

use std::ffi::c_void;
use std::ptr;

use anyhow::{Context, Result};
use objc::declare::ClassDecl;
use objc::rc::autoreleasepool;
use objc::runtime::{BOOL, Class, NO, Object, Sel, YES};
use objc::{class, msg_send, sel, sel_impl};
use tokio::sync::mpsc::UnboundedSender;

use super::TrayState;
use crate::playback::controls::Command;

/// The icon, drawn in the menu bar's colour as a template image. NSImage
/// reads SVG since macOS 14; older systems get the app's own icon.
const SYMBOLIC_ICON: &[u8] = include_bytes!(
    "../../../data/icons/symbolic/apps/io.github.oxakromax.SwellPlayer-symbolic.svg"
);
/// The icon's size in the menu bar, in points.
const ICON_SIZE: f64 = 18.0;
/// `NSVariableStatusItemLength`: as wide as the icon.
const VARIABLE_LENGTH: f64 = -1.0;

const TARGET_CLASS: &str = "SwellTrayTarget";
/// The target's ivar: a `*mut UnboundedSender<Command>`, or null.
const COMMANDS_IVAR: &str = "swellCommands";
/// `NSUTF8StringEncoding`.
const UTF8_ENCODING: usize = 4;

/// An object, as Objective-C names its pointers.
#[allow(non_camel_case_types)]
type id = *mut Object;
#[allow(non_upper_case_globals)]
const nil: id = ptr::null_mut();

/// `NSSize` (`CGSize` on 64-bit), in points.
#[repr(C)]
struct NSSize {
    width: f64,
    height: f64,
}

/// What the menu's items send, by tag: an item's tag is its command's
/// index here plus one, since 0 is every item's default tag.
const COMMANDS: [Command; 6] = [
    Command::Raise,
    Command::Play,
    Command::Pause,
    Command::Previous,
    Command::Next,
    Command::Quit,
];

/// The status item, while shown. Every `id` here is owned (+1) and
/// released on drop.
pub struct Backend {
    status_item: id,
    menu: id,
    show: id,
    play_pause: id,
    previous: id,
    next: id,
    quit: id,
    target: id,
    /// The target's channel, freed on drop after the target lets go of it.
    commands: *mut UnboundedSender<Command>,
}

impl Backend {
    pub fn new(commands: UnboundedSender<Command>, state: &TrayState) -> Result<Self> {
        let class = target_class().context("could not declare the status item's target")?;
        autoreleasepool(|| {
            let commands = Box::into_raw(Box::new(commands));
            // Safety: main thread (see the module's documentation). Each
            // message matches its AppKit signature; objects from `alloc`/
            // `new` and the retained status item are owned and released in
            // `Drop`, the rest are autoreleased into this pool. The target
            // holds `commands` until `Drop` clears it, before freeing it.
            unsafe {
                let target: id = msg_send![class, new];
                (*target).set_ivar(COMMANDS_IVAR, commands.cast::<c_void>());

                let menu: id = msg_send![class!(NSMenu), new];
                let () = msg_send![menu, setAutoenablesItems: NO];
                let show = menu_item(menu, target);
                add_separator(menu);
                let play_pause = menu_item(menu, target);
                let previous = menu_item(menu, target);
                let next = menu_item(menu, target);
                add_separator(menu);
                let quit = menu_item(menu, target);

                let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
                let status_item: id = msg_send![status_bar, statusItemWithLength: VARIABLE_LENGTH];
                let status_item: id = msg_send![status_item, retain];
                let () = msg_send![status_item, setMenu: menu];
                let button: id = msg_send![status_item, button];
                let () = msg_send![button, setAccessibilityLabel: ns_string(&state.title)];
                if !set_icon(button) {
                    let () = msg_send![button, setTitle: ns_string(&state.title)];
                }

                let backend = Self {
                    status_item,
                    menu,
                    show,
                    play_pause,
                    previous,
                    next,
                    quit,
                    target,
                    commands,
                };
                backend.update(state);
                Ok(backend)
            }
        })
    }

    pub fn update(&self, state: &TrayState) {
        let (play_pause, play_pause_command) = state.play_pause();
        let labels = &state.labels;
        autoreleasepool(|| {
            // Safety: main thread; all of these are live objects owned by
            // `self`, and the strings are autoreleased into this pool.
            unsafe {
                let button: id = msg_send![self.status_item, button];
                let () = msg_send![button, setToolTip: ns_string(&state.tooltip())];
                set_item(self.show, &labels.show, Command::Raise, true);
                set_item(
                    self.play_pause,
                    play_pause,
                    play_pause_command,
                    state.can_play,
                );
                set_item(
                    self.previous,
                    &labels.previous,
                    Command::Previous,
                    state.can_go_previous,
                );
                set_item(self.next, &labels.next, Command::Next, state.can_go_next);
                set_item(self.quit, &labels.quit, Command::Quit, true);
            }
        });
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        autoreleasepool(|| {
            // Safety: main thread; every object here is owned by `self` and
            // released once. The items lose their target and the target its
            // channel before either goes, so a late action does nothing.
            unsafe {
                let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
                let () = msg_send![status_bar, removeStatusItem: self.status_item];
                let () = msg_send![self.status_item, setMenu: nil];
                for item in [
                    self.show,
                    self.play_pause,
                    self.previous,
                    self.next,
                    self.quit,
                ] {
                    let () = msg_send![item, setTarget: nil];
                    let () = msg_send![item, release];
                }
                (*self.target).set_ivar(COMMANDS_IVAR, ptr::null_mut::<c_void>());
                drop(Box::from_raw(self.commands));
                let () = msg_send![self.target, release];
                let () = msg_send![self.menu, release];
                let () = msg_send![self.status_item, release];
            }
        });
    }
}

/// The class of the menu items' target, declared on first use.
fn target_class() -> Option<&'static Class> {
    if let Some(class) = Class::get(TARGET_CLASS) {
        return Some(class);
    }
    let mut decl = ClassDecl::new(TARGET_CLASS, class!(NSObject))?;
    decl.add_ivar::<*mut c_void>(COMMANDS_IVAR);
    // Safety: `menu_action` has the signature of an action method,
    // `- (void)menuAction:(id)sender`.
    unsafe {
        decl.add_method(
            sel!(menuAction:),
            menu_action as extern "C" fn(&Object, Sel, id),
        );
    }
    Some(decl.register())
}

/// `-[SwellTrayTarget menuAction:]`: sends the command of the item chosen.
extern "C" fn menu_action(this: &Object, _: Sel, sender: id) {
    // Safety: AppKit calls this on the main thread with the menu item as
    // the sender. The ivar is null or points to the `Backend`'s channel,
    // which outlives it (`Drop` clears it first).
    unsafe {
        let commands = *this.get_ivar::<*mut c_void>(COMMANDS_IVAR);
        let commands = commands.cast::<UnboundedSender<Command>>().as_ref();
        let tag: isize = msg_send![sender, tag];
        let (Some(commands), Some(command)) = (commands, command(tag)) else {
            return;
        };
        // A menu item does not activate the app: bring it forward before
        // the window is presented.
        if command == Command::Raise {
            activate();
        }
        let _ = commands.send(command);
    }
}

/// Makes Swell the active app.
///
/// # Safety
/// Main thread.
unsafe fn activate() {
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let modern: BOOL = msg_send![app, respondsToSelector: sel!(activate)];
        if modern == YES {
            let () = msg_send![app, activate];
        } else {
            let () = msg_send![app, activateIgnoringOtherApps: YES];
        }
    }
}

fn tag(command: &Command) -> isize {
    COMMANDS
        .iter()
        .position(|known| known == command)
        .map_or(0, |index| index as isize + 1)
}

fn command(tag: isize) -> Option<Command> {
    let index = usize::try_from(tag).ok()?.checked_sub(1)?;
    COMMANDS.get(index).cloned()
}

/// Adds an item that sends `menuAction:` to `target`; its label and tag
/// come with the first [`set_item`]. Returns it owned.
///
/// # Safety
/// Main thread, inside an autorelease pool; `menu` is an NSMenu.
unsafe fn menu_item(menu: id, target: id) -> id {
    unsafe {
        let item: id = msg_send![class!(NSMenuItem), alloc];
        let item: id = msg_send![item,
            initWithTitle: ns_string("")
            action: sel!(menuAction:)
            keyEquivalent: ns_string("")];
        let () = msg_send![item, setTarget: target];
        let () = msg_send![menu, addItem: item];
        item
    }
}

/// # Safety
/// Main thread, inside an autorelease pool; `menu` is an NSMenu.
unsafe fn add_separator(menu: id) {
    unsafe {
        let separator: id = msg_send![class!(NSMenuItem), separatorItem];
        let () = msg_send![menu, addItem: separator];
    }
}

/// # Safety
/// Main thread, inside an autorelease pool; `item` is an NSMenuItem.
unsafe fn set_item(item: id, label: &str, command: Command, enabled: bool) {
    unsafe {
        let () = msg_send![item, setTitle: ns_string(label)];
        let () = msg_send![item, setTag: tag(&command)];
        let () = msg_send![item, setEnabled: if enabled { YES } else { NO }];
    }
}

/// Gives the status item's button its icon: the symbolic icon as a
/// template image, else the app's icon. False if there is neither.
///
/// # Safety
/// Main thread, inside an autorelease pool; `button` is an NSButton.
unsafe fn set_icon(button: id) -> bool {
    unsafe {
        let data: id = msg_send![class!(NSData),
            dataWithBytes: SYMBOLIC_ICON.as_ptr().cast::<c_void>()
            length: SYMBOLIC_ICON.len()];
        let image: id = msg_send![class!(NSImage), alloc];
        let mut image: id = msg_send![image, initWithData: data];
        if image != nil {
            let valid: BOOL = msg_send![image, isValid];
            if valid == YES {
                let () = msg_send![image, setTemplate: YES];
            } else {
                let () = msg_send![image, release];
                image = nil;
            }
        }
        if image == nil {
            // A copy, so resizing it leaves the Dock's icon alone.
            let app: id = msg_send![class!(NSApplication), sharedApplication];
            let icon: id = msg_send![app, applicationIconImage];
            if icon == nil {
                return false;
            }
            image = msg_send![icon, copy];
        }
        let size = NSSize {
            width: ICON_SIZE,
            height: ICON_SIZE,
        };
        let () = msg_send![image, setSize: size];
        let () = msg_send![button, setImage: image];
        let () = msg_send![image, release];
        true
    }
}

/// An autoreleased NSString.
///
/// # Safety
/// Inside an autorelease pool.
unsafe fn ns_string(text: &str) -> id {
    unsafe {
        let string: id = msg_send![class!(NSString), alloc];
        let string: id = msg_send![string,
            initWithBytes: text.as_ptr().cast::<c_void>()
            length: text.len()
            encoding: UTF8_ENCODING];
        msg_send![string, autorelease]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip() {
        for known in &COMMANDS {
            assert_eq!(command(tag(known)).as_ref(), Some(known));
        }
        assert_eq!(tag(&Command::Stop), 0);
        assert_eq!(command(0), None);
        assert_eq!(command(-1), None);
        assert_eq!(command(COMMANDS.len() as isize + 1), None);
    }
}
