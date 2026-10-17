//! What every page and widget of a signed-in window shares.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use adw::prelude::*;

use super::nav::{Navigator, Route};
use crate::api::Api;
use crate::covers::CoverLoader;
use crate::library::Library;
use crate::playback::Playback;
use crate::settings;

/// Tabs of the now-playing panel (docs/design.md §3.13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelTab {
    Playing,
    Lyrics,
    Queue,
}

pub struct Ctx {
    pub window: adw::ApplicationWindow,
    pub api: Arc<Api>,
    pub covers: Rc<CoverLoader>,
    pub playback: Rc<Playback>,
    pub library: Rc<Library>,
    pub settings: Rc<settings::Store>,
    pub toasts: adw::ToastOverlay,
    navigator: RefCell<Weak<Navigator>>,
    /// Asks the window to sign out (with an optional reason to show).
    sign_out: RefCell<Option<Box<dyn Fn(Option<String>)>>>,
    /// The open panel tab, `None` when closed. Set by the shell.
    panel: Cell<Option<PanelTab>>,
    panel_handler: RefCell<Option<Box<dyn Fn(Option<PanelTab>)>>>,
    panel_listeners: RefCell<Vec<Rc<dyn Fn(Option<PanelTab>)>>>,
}

impl Ctx {
    pub fn new(
        window: adw::ApplicationWindow,
        api: Arc<Api>,
        covers: Rc<CoverLoader>,
        playback: Rc<Playback>,
        library: Rc<Library>,
        settings: Rc<settings::Store>,
        toasts: adw::ToastOverlay,
    ) -> Rc<Self> {
        Rc::new(Self {
            window,
            api,
            covers,
            playback,
            library,
            settings,
            toasts,
            navigator: RefCell::default(),
            sign_out: RefCell::default(),
            panel: Cell::new(None),
            panel_handler: RefCell::default(),
            panel_listeners: RefCell::default(),
        })
    }

    pub fn set_navigator(&self, navigator: &Rc<Navigator>) {
        self.navigator.replace(Rc::downgrade(navigator));
    }

    pub fn set_sign_out(&self, sign_out: impl Fn(Option<String>) + 'static) {
        self.sign_out.replace(Some(Box::new(sign_out)));
    }

    /// Opens `route` (pushes a page, or switches the sidebar section).
    pub fn navigate(&self, route: Route) {
        if let Some(navigator) = self.navigator.borrow().upgrade() {
            navigator.navigate(route);
        }
    }

    /// The shell shows/hides the panel when asked; it calls
    /// [`Ctx::panel_changed`] once it has.
    pub fn set_panel_handler(&self, handler: impl Fn(Option<PanelTab>) + 'static) {
        self.panel_handler.replace(Some(Box::new(handler)));
    }

    /// Opens the panel on `tab`, or closes it with `None`.
    pub fn show_panel(&self, tab: Option<PanelTab>) {
        if let Some(handler) = self.panel_handler.borrow().as_ref() {
            handler(tab);
        }
    }

    /// Closes the panel if `tab` is the visible tab, opens it on `tab`
    /// otherwise (what the player bar's toggle buttons do).
    pub fn toggle_panel(&self, tab: PanelTab) {
        let next = if self.panel.get() == Some(tab) {
            None
        } else {
            Some(tab)
        };
        self.show_panel(next);
    }

    pub fn panel(&self) -> Option<PanelTab> {
        self.panel.get()
    }

    /// Called by the shell when the panel opened, closed or changed tab.
    pub fn panel_changed(&self, tab: Option<PanelTab>) {
        self.panel.set(tab);
        let listeners: Vec<_> = self.panel_listeners.borrow().clone();
        for listener in listeners {
            listener(tab);
        }
    }

    pub fn connect_panel_changed(&self, listener: impl Fn(Option<PanelTab>) + 'static) {
        self.panel_listeners.borrow_mut().push(Rc::new(listener));
    }

    pub fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    /// A toast with an "Undo" button that runs `undo`.
    pub fn toast_with_undo(&self, message: &str, undo: impl Fn() + 'static) {
        let toast = adw::Toast::builder()
            .title(message)
            .button_label("Undo")
            .build();
        toast.connect_button_clicked(move |_| undo());
        self.toasts.add_toast(toast);
    }

    /// Reports an error: signs out when the session is gone, toasts
    /// `message` otherwise.
    pub fn report(&self, message: &str, err: &anyhow::Error) {
        if crate::tidal::is_auth_error(err) {
            if let Some(sign_out) = self.sign_out.borrow().as_ref() {
                sign_out(Some(
                    "Your TIDAL session has expired. Please sign in again.".into(),
                ));
            }
            return;
        }
        log::warn!("{message}: {err:#}");
        self.toast(message);
    }

    /// Copies `text` to the clipboard and says so.
    pub fn copy(&self, text: &str, what: &str) {
        self.window.clipboard().set_text(text);
        self.toast(&format!("{what} copied"));
    }
}
