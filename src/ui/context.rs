//! What every page and widget of a signed-in window shares.
//!
//! Network state: [`Ctx::report`] shows the offline banner when an error
//! is a connection failure; [`Ctx::network_ok`] hides it again and should
//! be called after any request that succeeded.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use adw::prelude::*;

use super::nav::{Navigator, Route};
use crate::api::Api;
use crate::covers::CoverLoader;
use crate::i18n::tr;
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
    sign_out: RefCell<Option<SignOut>>,
    /// The open panel tab, `None` when closed. Set by the shell.
    panel: Cell<Option<PanelTab>>,
    panel_handler: RefCell<Option<PanelHandler>>,
    panel_listeners: RefCell<Vec<(u64, PanelListener)>>,
    next_listener: Cell<u64>,
    /// "You're offline" (docs/design.md §4.8); the shell shows it above
    /// the content.
    pub offline_banner: adw::Banner,
    /// Length of the synced favourite tracks list, once known.
    favorite_track_count: Cell<Option<usize>>,
    count_listeners: RefCell<Vec<CountListener>>,
}

type PanelListener = Rc<dyn Fn(Option<PanelTab>)>;
type CountListener = Rc<dyn Fn(Option<usize>)>;
type PanelHandler = Box<dyn Fn(Option<PanelTab>)>;
type SignOut = Box<dyn Fn(Option<String>)>;

/// Whether `err` (anywhere in its chain) is a failure to reach the server:
/// no connection, DNS, or a timeout.
pub fn is_connection_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.is_connect() || e.is_timeout())
            || cause.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::NotConnected
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::NetworkUnreachable
                        | std::io::ErrorKind::HostUnreachable
                )
            })
    })
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
            next_listener: Cell::new(1),
            offline_banner: adw::Banner::builder()
                .title(tr("You’re offline — showing saved data"))
                .revealed(false)
                .build(),
            favorite_track_count: Cell::new(None),
            count_listeners: RefCell::default(),
        })
    }

    /// The number of favourite tracks the Tracks page lists (TIDAL's own
    /// total also counts tracks it no longer serves, so it is not used).
    /// Whoever holds the synced list reports its length here; the sidebar
    /// shows it.
    pub fn set_favorite_track_count(&self, count: usize) {
        if self.favorite_track_count.replace(Some(count)) == Some(count) {
            return;
        }
        let listeners: Vec<CountListener> = self.count_listeners.borrow().clone();
        for listener in listeners {
            listener(Some(count));
        }
    }

    pub fn favorite_track_count(&self) -> Option<usize> {
        self.favorite_track_count.get()
    }

    /// Calls `listener` whenever [`Ctx::set_favorite_track_count`] changes
    /// the count. For listeners that live as long as the window.
    pub fn connect_favorite_track_count(&self, listener: impl Fn(Option<usize>) + 'static) {
        self.count_listeners.borrow_mut().push(Rc::new(listener));
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
        let listeners: Vec<PanelListener> = self
            .panel_listeners
            .borrow()
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect();
        for listener in listeners {
            listener(tab);
        }
    }

    /// Calls `listener` whenever the panel opens, closes or changes tab.
    /// Returns an id for [`Ctx::disconnect_panel_changed`]; listeners that
    /// live as long as the window may ignore it.
    pub fn connect_panel_changed(&self, listener: impl Fn(Option<PanelTab>) + 'static) -> u64 {
        let id = self.next_listener.get();
        self.next_listener.set(id + 1);
        self.panel_listeners
            .borrow_mut()
            .push((id, Rc::new(listener)));
        id
    }

    pub fn disconnect_panel_changed(&self, id: u64) {
        self.panel_listeners
            .borrow_mut()
            .retain(|(other, _)| *other != id);
    }

    /// Shows or hides the offline banner.
    pub fn set_offline(&self, offline: bool) {
        if self.offline_banner.is_revealed() != offline {
            self.offline_banner.set_revealed(offline);
        }
    }

    pub fn is_offline(&self) -> bool {
        self.offline_banner.is_revealed()
    }

    /// A request reached TIDAL: clears the offline banner. Call it after
    /// any successful network request (pages: after `load_cached`
    /// delivers fresh data).
    pub fn network_ok(&self) {
        self.set_offline(false);
    }

    pub fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    /// A toast with an "Undo" button that runs `undo`.
    pub fn toast_with_undo(&self, message: &str, undo: impl Fn() + 'static) {
        let toast = adw::Toast::builder()
            .title(message)
            .button_label(tr("Undo"))
            .build();
        // Also reachable with Ctrl+Z after the toast is gone.
        let undo = super::window::remember_undo(undo);
        toast.connect_button_clicked(move |_| undo());
        self.toasts.add_toast(toast);
    }

    /// Reports an error: signs out when the session is gone, toasts
    /// `message` otherwise.
    pub fn report(&self, message: &str, err: &anyhow::Error) {
        // A late answer from a session that already ended must not act on
        // the next one.
        if self.api.is_signed_out() {
            log::debug!("ignoring an error from an ended session: {err:#}");
            return;
        }
        if crate::tidal::is_auth_error(err) {
            if let Some(sign_out) = self.sign_out.borrow().as_ref() {
                sign_out(Some(
                    tr("Your TIDAL session has expired. Please sign in again.").into(),
                ));
            }
            return;
        }
        log::warn!("{message}: {err:#}");
        if is_connection_error(err) {
            self.set_offline(true);
        }
        self.toast(message);
    }

    /// Copies `text` to the clipboard and says so with `message` ("Link
    /// copied").
    pub fn copy(&self, text: &str, message: &str) {
        self.window.clipboard().set_text(text);
        self.toast(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_errors() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // Nothing listens on the discard port.
        let err = runtime
            .block_on(async {
                reqwest::Client::new()
                    .get("http://127.0.0.1:9/")
                    .send()
                    .await
            })
            .unwrap_err();
        let err = anyhow::Error::new(err).context("loading the page");
        assert!(is_connection_error(&err));
        assert!(!is_connection_error(&anyhow::anyhow!("TIDAL error 404")));
    }
}
