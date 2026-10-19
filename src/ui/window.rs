//! The main window (docs/design.md §2): a stack that switches between
//! loading, sign-in and the signed-in shell, plus the window's actions,
//! keyboard shortcuts (§5.2), background mode and `tidal://` links.
//!
//! The shell is an `AdwMultiLayoutView` with a "wide" layout (sidebar,
//! content, docked/overlaid now-playing panel, player bar) and a "narrow"
//! one (content, mini player, bottom sheet, view switcher bar), switched by
//! three breakpoints (§2.2). The content, panel and player bar are single
//! widgets moved between the layouts' slots.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::context::{Ctx, PanelTab};
use super::nav::{Navigator, Route};
use super::sidebar::Sidebar;
use super::widgets::mini_player::MiniPlayer;
use super::widgets::now_playing::NowPlayingPanel;
use super::widgets::player_bar::PlayerBar;
use crate::api::{self, Api};
use crate::covers::CoverLoader;
use crate::i18n::tr;
use crate::library::{Item, Library};
use crate::model::TrackInfo;
use crate::playback::{Event, Playback, Source};
use crate::settings::Repeat;
use crate::tidal::pages::Target;
use crate::{config, runtime, session, settings};

/// Seek step of Shift+←/→, in seconds.
const SEEK_STEP: f64 = 10.0;
/// Volume step of Ctrl+↑/↓.
const VOLUME_STEP: f64 = 0.05;
/// Windows this short get the `short` class (smaller page headers).
const SHORT: &str = "max-height: 640px";

/// Accelerators of the app and window actions (§5.2) that act wherever the
/// focus is. Shortcuts that mean something else while editing text (Space,
/// the arrows, Ctrl+Z/S/R/M/D) are handled by a capture-phase key
/// controller instead ([`KEYS`]), so they never fire while typing or in
/// dialogs.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("app.quit", &["<Control>q"]),
    ("win.close", &["<Control>w"]),
    ("win.preferences", &["<Control>comma"]),
    ("win.shortcuts", &["<Control>question"]),
    ("win.search", &["<Control>f"]),
    ("win.section('home')", &["<Alt>1"]),
    ("win.section('explore')", &["<Alt>2"]),
    ("win.section('search')", &["<Alt>3"]),
    ("win.section('collection')", &["<Alt>4"]),
    ("win.show-sidebar", &["F9"]),
    ("win.back", &["<Alt>Left", "Back"]),
    ("win.panel('playing')", &["<Control><Shift>n"]),
    ("win.panel('lyrics')", &["<Control><Shift>l"]),
    ("win.panel('queue')", &["<Control>l"]),
    ("win.play-pause", &["<Control>space"]),
];

/// Shortcuts of the window's key controller: (key, modifiers, action).
/// They act only when [`shortcut_allowed`] says so; Space also leaves
/// buttons, switches and scales alone.
const KEYS: &[(gdk::Key, Modifiers, &str)] = &[
    (gdk::Key::space, Modifiers::None, "play-pause"),
    (gdk::Key::Right, Modifiers::Control, "next"),
    (gdk::Key::Left, Modifiers::Control, "previous"),
    (gdk::Key::Right, Modifiers::Shift, "seek-forward"),
    (gdk::Key::Left, Modifiers::Shift, "seek-backward"),
    (gdk::Key::Up, Modifiers::Control, "volume-up"),
    (gdk::Key::Down, Modifiers::Control, "volume-down"),
    (gdk::Key::z, Modifiers::Control, "undo"),
    (gdk::Key::m, Modifiers::Control, "mute"),
    (gdk::Key::s, Modifiers::Control, "shuffle"),
    (gdk::Key::r, Modifiers::Control, "repeat"),
    (gdk::Key::d, Modifiers::Control, "favorite"),
];

/// The modifier combinations [`KEYS`] use.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Modifiers {
    None,
    Control,
    Shift,
}

impl Modifiers {
    fn of(state: gdk::ModifierType) -> Option<Self> {
        let state = state
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK);
        if state.is_empty() {
            Some(Self::None)
        } else if state == gdk::ModifierType::CONTROL_MASK {
            Some(Self::Control)
        } else if state == gdk::ModifierType::SHIFT_MASK {
            Some(Self::Shift)
        } else {
            None
        }
    }
}

/// The text of [`KEYS`]'s accelerator for `action`, for the Keyboard
/// Shortcuts dialog (`GtkShortcutTrigger` syntax).
pub fn key_accel(action: &str) -> Option<String> {
    let (key, modifiers, _) = KEYS.iter().find(|(_, _, a)| *a == action)?;
    let prefix = match modifiers {
        Modifiers::None => "",
        Modifiers::Control => "<Control>",
        Modifiers::Shift => "<Shift>",
    };
    Some(format!("{prefix}{}", key.name()?))
}

thread_local! {
    /// The window's state, while it exists (held strongly by the window).
    static UI: RefCell<Weak<Ui>> = RefCell::default();
    /// The last reversible action, for Ctrl+Z.
    static UNDO: RefCell<Option<Rc<dyn Fn()>>> = RefCell::default();
}

fn current() -> Option<Rc<Ui>> {
    UI.with(|ui| ui.borrow().upgrade())
}

/// Shows the window, creating it the first time. A second launch (or a
/// launch while running in the background) only presents it.
pub fn activate(app: &adw::Application) {
    match current() {
        Some(ui) => ui.window.present(),
        None => build(app),
    }
}

/// Opens TIDAL links (`tidal://…`, `https://tidal.com/…`,
/// `https://listen.tidal.com/…`): pages are pushed onto the visible
/// section, tracks are played. Links that arrive before sign-in wait for it.
pub fn open_links(app: &adw::Application, uris: &[String]) {
    activate(app);
    let Some(ui) = current() else {
        return;
    };
    for uri in uris {
        match Target::from_url(uri) {
            Some(target) => ui.open_target(target),
            None => log::warn!("not a TIDAL link: {uri}"),
        }
    }
}

/// Saves the window state and quits (the Quit action: closing the window
/// may only hide it).
pub fn quit(app: &adw::Application) {
    if let Some(ui) = current() {
        ui.save_geometry();
        ui.settings.save_now();
    }
    app.quit();
}

static RESTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Quits, and has `main` start Swell again once this instance is done.
pub fn restart(app: &adw::Application) {
    RESTART.store(true, std::sync::atomic::Ordering::SeqCst);
    quit(app);
}

/// Whether [`restart`] was asked for.
pub fn restart_requested() -> bool {
    RESTART.load(std::sync::atomic::Ordering::SeqCst)
}

/// Remembers `undo` as the last reversible action, so Ctrl+Z runs it even
/// after its toast is gone. Returns the action to run from the toast's
/// "Undo" button; either way it runs at most once.
pub fn remember_undo(undo: impl Fn() + 'static) -> Rc<dyn Fn()> {
    let done = Cell::new(false);
    let once: Rc<dyn Fn()> = Rc::new(move || {
        if !done.replace(true) {
            undo();
        }
    });
    UNDO.with(|slot| slot.replace(Some(once.clone())));
    once
}

fn build(app: &adw::Application) {
    let settings = settings::Store::load();
    let mut current = settings.get();
    if let Some((width, height)) = super::debug::initial_size() {
        (current.window_width, current.window_height) = (width, height);
        current.window_maximized = false;
    }
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(config::APP_NAME)
        .icon_name(config::APP_ID)
        .default_width(current.window_width)
        .default_height(current.window_height)
        .maximized(current.window_maximized)
        .hide_on_close(current.background)
        .width_request(360)
        .height_request(294)
        .build();

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(200)
        .build();
    // The spinner appears only if loading takes a noticeable time.
    let spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    glib::timeout_add_local_once(std::time::Duration::from_millis(300), {
        let spinner = spinner.downgrade();
        move || {
            if let Some(spinner) = spinner.upgrade() {
                spinner.set_visible(true);
            }
        }
    });
    stack.add_named(&spinner, Some("loading"));
    window.set_content(Some(&stack));
    window.present();
    super::debug::schedule_screenshot(window.upcast_ref());

    let http = api::http_client();
    api::preconnect(&http);
    let ui = Rc::new(Ui {
        window,
        stack,
        covers: CoverLoader::new(http.clone()),
        http,
        settings,
        session: RefCell::default(),
        session_actions: RefCell::default(),
        pending: RefCell::default(),
    });
    UI.with(|slot| slot.replace(Rc::downgrade(&ui)));
    ui.install_actions();
    ui.install_key_controller();

    // Run in background: closing hides the window; Quit quits.
    ui.settings.connect_changed({
        let window = ui.window.downgrade();
        move |settings| {
            if let Some(window) = window.upgrade() {
                window.set_hide_on_close(settings.background);
            }
        }
    });
    ui.window.connect_close_request({
        let ui = Rc::downgrade(&ui);
        move |_| {
            if let Some(ui) = ui.upgrade() {
                ui.save_geometry();
            }
            glib::Propagation::Proceed
        }
    });
    // Everything else holds the UI weakly; the window keeps it alive.
    ui.window.connect_destroy({
        let ui = RefCell::new(Some(ui.clone()));
        move |_| {
            if let Some(ui) = ui.take() {
                ui.end_session();
            }
        }
    });
    if super::debug::force_login() {
        return ui.show_login(None);
    }
    glib::spawn_future_local(async move {
        let stored = runtime::spawn(session::load()).await;
        crate::timing::mark("session read from the keyring");
        match stored {
            Ok(Some(stored)) => ui.show_shell(Api::restore(stored, ui.http.clone())),
            Ok(None) => ui.show_login(None),
            Err(err) => {
                log::warn!("could not open the keyring: {err:#}");
                ui.show_login(Some(session::open_error().to_owned()))
            }
        }
    });
}

struct Ui {
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    covers: Rc<CoverLoader>,
    http: reqwest::Client,
    settings: Rc<settings::Store>,
    /// The signed-in session, while signed in.
    session: RefCell<Option<Session>>,
    /// Actions that need a session (disabled while signed out).
    session_actions: RefCell<Vec<gio::SimpleAction>>,
    /// Links opened before sign-in.
    pending: RefCell<Vec<Target>>,
}

struct Session {
    ctx: Rc<Ctx>,
    shell: Rc<Shell>,
}

impl Ui {
    fn ctx(&self) -> Option<Rc<Ctx>> {
        self.session.borrow().as_ref().map(|s| s.ctx.clone())
    }

    fn shell(&self) -> Option<Rc<Shell>> {
        self.session.borrow().as_ref().map(|s| s.shell.clone())
    }

    fn save_geometry(&self) {
        if !self.window.is_visible() {
            return;
        }
        let (width, height) = self.window.default_size();
        let maximized = self.window.is_maximized();
        self.settings.update(|s| {
            if !maximized {
                s.window_width = width;
                s.window_height = height;
            }
            s.window_maximized = maximized;
        });
        self.settings.save_now();
    }

    fn set_page(&self, name: &str, page: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.stack.child_by_name(name) {
            self.stack.remove(&old);
        }
        self.stack.add_named(page, Some(name));
        self.stack.set_visible_child_name(name);
    }

    /// Stops everything tied to the signed-in session.
    fn end_session(&self) {
        if let Some(session) = self.session.take() {
            session.ctx.playback.stop();
        }
        // Ctrl+Z must not reach back into the old session.
        UNDO.with(|slot| slot.take());
        self.window.remove_action("show-sidebar");
        self.window.remove_css_class("narrow");
        self.window.remove_css_class("short");
        for action in self.session_actions.borrow().iter() {
            action.set_enabled(false);
        }
    }

    fn show_login(self: &Rc<Self>, error: Option<String>) {
        self.end_session();
        if let Some(main) = self.stack.child_by_name("main") {
            self.stack.remove(&main);
        }
        let ui = Rc::downgrade(self);
        let page = super::login::page(&self.window, self.http.clone(), error, move |api| {
            if let Some(ui) = ui.upgrade() {
                ui.show_shell(api);
            }
        });
        self.set_page("login", &page);
    }

    /// Ends the session, deletes the tokens and cached library, and goes
    /// back to the sign-in page with `reason`.
    fn sign_out(self: &Rc<Self>, reason: Option<String>) {
        let Some(ctx) = self.ctx() else {
            return;
        };
        ctx.playback.forget_session();
        self.end_session();
        // Nothing of this account may show in the next one.
        super::pages::search_data::forget_all();
        self.covers.clear_memory();
        self.pending.borrow_mut().clear();
        let api = ctx.api.clone();
        let ui = self.clone();
        glib::spawn_future_local(async move {
            let error = match runtime::spawn(async move { api.sign_out().await }).await {
                Ok(()) => reason,
                Err(err) => {
                    log::warn!("could not remove the session from the keyring: {err:#}");
                    Some(tr("Couldn't remove your session from the keyring.").to_owned())
                }
            };
            ui.show_login(error);
        });
    }

    /// Asks before signing out (§4.13: irreversible, no undo).
    fn confirm_sign_out(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Sign Out?"))
            .body(tr("You will need to sign in again to listen. Your library cached on this computer is deleted."))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", tr("_Cancel")), ("sign-out", tr("_Sign Out"))]);
        dialog.set_response_appearance("sign-out", adw::ResponseAppearance::Destructive);
        let ui = Rc::downgrade(self);
        dialog.connect_response(Some("sign-out"), move |_, _| {
            if let Some(ui) = ui.upgrade() {
                ui.sign_out(None);
            }
        });
        dialog.present(Some(&self.window));
    }

    fn show_shell(self: &Rc<Self>, api: Arc<Api>) {
        let playback = Playback::new(api.clone(), self.settings.clone());
        if let Some(app) = self.window.application().and_downcast::<adw::Application>() {
            playback.attach(&app);
        }
        let library = Library::new(api.clone());
        let toasts = adw::ToastOverlay::new();
        let ctx = Ctx::new(
            self.window.clone(),
            api,
            self.covers.clone(),
            playback,
            library.clone(),
            self.settings.clone(),
            toasts,
        );
        ctx.set_sign_out({
            let ui = Rc::downgrade(self);
            move |reason| {
                if let Some(ui) = ui.upgrade() {
                    ui.sign_out(reason);
                }
            }
        });

        let shell = Shell::new(&ctx);
        self.window.add_action(&gio::PropertyAction::new(
            "show-sidebar",
            &shell.sidebar_split,
            "show-sidebar",
        ));
        for action in self.session_actions.borrow().iter() {
            action.set_enabled(true);
        }
        self.session.replace(Some(Session {
            ctx: ctx.clone(),
            shell: shell.clone(),
        }));
        // Start in the layout breakpoint C will pick, so nothing is built
        // for the wrong one before the first allocation.
        let (width, _) = self.window.default_size();
        if !self.window.is_maximized() && width > 0 && width <= 720 {
            shell.multi.set_layout_name("narrow");
        }
        self.set_page("main", &shell.root);
        // Say what to install rather than fail at the first track.
        if let Some(message) = crate::playback::plugins::missing_message() {
            ctx.toasts
                .add_toast(adw::Toast::builder().title(message).timeout(0).build());
        }
        shell.navigator.show_section("home");
        if let Some(section) = super::debug::section() {
            shell.navigator.show_section(&section);
        }
        if let Some(route) = super::debug::route() {
            ctx.navigate(route);
        }
        if let Some(tab) = super::debug::panel() {
            ctx.show_panel(Some(tab));
        } else if !super::debug::screenshot_mode() {
            shell.restore_panel_state(&self.settings.get());
        }
        super::debug::open_dialog(self.window.upcast_ref());
        super::debug::run_actions(self.window.upcast_ref());
        if let Some(login) = self.stack.child_by_name("login") {
            self.stack.remove(&login);
        }
        // After the player bar subscribed: it shows the restored track.
        ctx.playback.restore_session();
        let pending: Vec<Target> = self.pending.take();
        for target in pending {
            self.open_target(target);
        }

        // A stream resolved: TIDAL is reachable.
        ctx.playback.subscribe({
            let ctx = Rc::downgrade(&ctx);
            move |event| {
                if let (Event::Format(Some(_)), Some(ctx)) = (event, ctx.upgrade()) {
                    ctx.network_ok();
                }
            }
        });
        // Weakly: a load that ends after signing out must not report to
        // (or sign out) the next session.
        glib::spawn_future_local({
            let ctx = Rc::downgrade(&ctx);
            async move {
                let result = library.load().await;
                let Some(ctx) = ctx.upgrade() else {
                    return;
                };
                match result {
                    Ok(()) => ctx.network_ok(),
                    Err(err) => ctx.report(tr("Couldn't load your library"), &err),
                }
            }
        });
    }

    fn open_target(&self, target: Target) {
        let Some(ctx) = self.ctx() else {
            self.pending.borrow_mut().push(target);
            return;
        };
        self.window.present();
        match target {
            Target::Track(id) => play_track(&ctx, id),
            target => match Route::from_target(&target) {
                Some(route) => ctx.navigate(route),
                None => log::warn!("cannot open {target:?}"),
            },
        }
    }

    /// Window actions. Those that need a session are disabled while
    /// signed out.
    fn install_actions(self: &Rc<Self>) {
        let string = Some(glib::VariantTy::STRING);
        self.action("shortcuts", None, false, |ui, _| {
            super::shortcuts::present(&ui.window)
        });
        self.action("about", None, false, |ui, _| {
            super::dialogs::about::present(&ui.window)
        });
        self.action("close", None, false, |ui, _| ui.window.close());
        self.action("preferences", None, true, |ui, _| {
            if let Some(ctx) = ui.ctx() {
                super::dialogs::preferences::present(&ctx);
            }
        });
        self.action("sign-out", None, true, |ui, _| ui.confirm_sign_out());
        self.action("section", string, true, |ui, param| {
            if let (Some(shell), Some(section)) = (ui.shell(), param.and_then(|p| p.str())) {
                shell.navigator.activate_section(section);
            }
        });
        self.action("search", None, true, |ui, _| {
            if let Some(shell) = ui.shell() {
                shell.focus_search();
            }
        });
        self.action("back", None, true, |ui, _| {
            if let Some(shell) = ui.shell() {
                shell.navigator.back();
            }
        });
        self.action("undo", None, true, |_, _| {
            if let Some(undo) = UNDO.with(|slot| slot.take()) {
                undo();
            }
        });
        self.action("panel", string, true, |ui, param| {
            let Some(tab) = param.and_then(|p| p.str()).and_then(tab_from_name) else {
                return;
            };
            if let Some(ctx) = ui.ctx() {
                ctx.toggle_panel(tab);
            }
        });
        self.playback_action("play-pause", |ctx| ctx.playback.toggle());
        self.playback_action("next", |ctx| ctx.playback.next());
        self.playback_action("previous", |ctx| ctx.playback.previous());
        self.playback_action("seek-forward", |ctx| seek_by(ctx, SEEK_STEP));
        self.playback_action("seek-backward", |ctx| seek_by(ctx, -SEEK_STEP));
        self.playback_action("volume-up", |ctx| {
            let playback = &ctx.playback;
            let value = (playback.volume() + VOLUME_STEP).min(1.0);
            if playback.is_muted() && value > 0.0 {
                playback.set_muted(false);
            }
            playback.set_volume(value);
        });
        self.playback_action("volume-down", |ctx| {
            let playback = &ctx.playback;
            let value = (playback.volume() - VOLUME_STEP).max(0.0);
            if playback.is_muted() && value > 0.0 {
                playback.set_muted(false);
            }
            playback.set_volume(value);
        });
        self.playback_action("mute", |ctx| {
            ctx.playback.set_muted(!ctx.playback.is_muted())
        });
        self.playback_action("shuffle", |ctx| {
            ctx.playback.set_shuffle(!ctx.playback.shuffle())
        });
        self.playback_action("repeat", |ctx| {
            ctx.playback.set_repeat(match ctx.playback.repeat() {
                Repeat::Off => Repeat::All,
                Repeat::All => Repeat::One,
                Repeat::One => Repeat::Off,
            });
        });
        self.playback_action("favorite", toggle_favorite_current);
        for action in self.session_actions.borrow().iter() {
            action.set_enabled(false);
        }

        // The mouse's back button.
        let back = gtk::GestureClick::builder().button(8).build();
        back.set_propagation_phase(gtk::PropagationPhase::Capture);
        back.connect_pressed({
            let window = self.window.downgrade();
            move |_, _, _, _| {
                if let Some(window) = window.upgrade() {
                    let _ = WidgetExt::activate_action(&window, "win.back", None);
                }
            }
        });
        self.window.add_controller(back);
    }

    fn action(
        self: &Rc<Self>,
        name: &str,
        parameter: Option<&glib::VariantTy>,
        needs_session: bool,
        activate: impl Fn(&Rc<Self>, Option<&glib::Variant>) + 'static,
    ) {
        let action = gio::SimpleAction::new(name, parameter);
        let ui = Rc::downgrade(self);
        action.connect_activate(move |_, parameter| {
            if let Some(ui) = ui.upgrade() {
                activate(&ui, parameter);
            }
        });
        self.window.add_action(&action);
        if needs_session {
            self.session_actions.borrow_mut().push(action);
        }
    }

    fn playback_action(self: &Rc<Self>, name: &str, activate: impl Fn(&Rc<Ctx>) + 'static) {
        self.action(name, None, true, move |ui, _| {
            if let Some(ctx) = ui.ctx() {
                activate(&ctx);
            }
        });
    }

    /// [`KEYS`] (§5.2), in the capture phase so lists do not swallow them,
    /// but never while typing or in dialogs (and Space never on buttons,
    /// switches or scales, where it has its own meaning); and F6 /
    /// Shift+F6, which move between the window's regions.
    fn install_key_controller(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let ui = Rc::downgrade(self);
        keys.connect_key_pressed(move |controller, key, keycode, state| {
            let Some(ui) = ui.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let Some(modifiers) = Modifiers::of(state) else {
                return glib::Propagation::Proceed;
            };
            if key == gdk::Key::F6 && modifiers != Modifiers::Control {
                return match ui.shell() {
                    Some(shell) if !in_dialog(&ui.window) => {
                        shell.cycle_region(&ui.window, modifiers == Modifiers::Shift);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                };
            }
            let key = key.to_lower();
            let find = |k: gdk::Key| KEYS.iter().find(|(kk, m, _)| *kk == k && *m == modifiers);
            let found = find(key).or_else(|| {
                // Non-Latin layouts: the same physical key in the other
                // layout groups (as GTK does for accelerators).
                if key.to_unicode().is_some_and(|c| c.is_ascii()) {
                    return None;
                }
                controller
                    .widget()?
                    .display()
                    .map_keycode(keycode)?
                    .into_iter()
                    .filter(|(position, _)| position.level() == 0)
                    .find_map(|(_, k)| find(k.to_lower()))
            });
            let Some((matched, _, action)) = found else {
                return glib::Propagation::Proceed;
            };
            if ui.ctx().is_none() || !shortcut_allowed(&ui.window, *matched == gdk::Key::space) {
                return glib::Propagation::Proceed;
            }
            let _ = WidgetExt::activate_action(&ui.window, &format!("win.{action}"), None);
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
    }
}

/// Whether the focus is in a dialog or popover over the window.
fn in_dialog(window: &adw::ApplicationWindow) -> bool {
    gtk::prelude::RootExt::focus(window).is_some_and(|focus| {
        focus.ancestor(adw::Dialog::static_type()).is_some()
            || focus.ancestor(gtk::Popover::static_type()).is_some()
    })
}

/// Whether a [`KEYS`] shortcut may act with the current focus: not while
/// editing text, and not in dialogs or popovers.
fn shortcut_allowed(window: &adw::ApplicationWindow, space: bool) -> bool {
    let Some(focus) = gtk::prelude::RootExt::focus(window) else {
        return true;
    };
    if focus.is::<gtk::Editable>()
        || focus.is::<gtk::Text>()
        || focus.is::<gtk::TextView>()
        || focus.ancestor(gtk::Editable::static_type()).is_some()
        || in_dialog(window)
    {
        return false;
    }
    !(space
        && (focus.is::<gtk::Button>()
            // The mini player is a focusable box that acts as a button.
            || focus.accessible_role() == gtk::AccessibleRole::Button
            || focus.is::<gtk::Switch>()
            || focus.is::<gtk::Range>()
            || focus.is::<gtk::CheckButton>()
            || focus.is::<gtk::DropDown>()))
}

fn seek_by(ctx: &Rc<Ctx>, delta: f64) {
    let playback = &ctx.playback;
    let Some(position) = playback.position() else {
        return;
    };
    let end = playback.duration().unwrap_or(f64::MAX);
    playback.seek((position + delta).clamp(0.0, end));
}

fn toggle_favorite_current(ctx: &Rc<Ctx>) {
    let Some(track) = ctx.playback.current() else {
        return;
    };
    let item = Item::Track(track.id);
    let favorite = !ctx.library.is_favorite(&item);
    let weak = Rc::downgrade(ctx);
    ctx.library.set_favorite(item, favorite, move |result| {
        if let (Err(err), Some(ctx)) = (result, weak.upgrade()) {
            ctx.report(tr("Couldn't update favourites"), &err);
        }
    });
}

/// Plays one track from a link.
fn play_track(ctx: &Rc<Ctx>, id: u64) {
    let api = ctx.api.clone();
    let weak = Rc::downgrade(ctx);
    glib::spawn_future_local(async move {
        let result = runtime::spawn(async move { api.track(id).await }).await;
        let Some(ctx) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(track) => {
                let title = track.title.clone();
                ctx.playback.play_tracks(
                    vec![TrackInfo::from(track)],
                    0,
                    Source {
                        title,
                        target: Some(Target::Track(id)),
                    },
                );
            }
            Err(err) => ctx.report(tr("Couldn't open the track"), &err),
        }
    });
}

/// The signed-in UI (§2.1).
struct Shell {
    this: Weak<Self>,
    ctx: Weak<Ctx>,
    /// Holds the breakpoints; the "main" page of the root stack.
    root: adw::BreakpointBin,
    navigator: Rc<Navigator>,
    multi: adw::MultiLayoutView,
    sidebar_split: adw::OverlaySplitView,
    panel_split: adw::OverlaySplitView,
    sheet: adw::BottomSheet,
    panel: Rc<NowPlayingPanel>,
    player_bar: Rc<PlayerBar>,
    narrow: Cell<bool>,
    /// Last seen `panel_split.collapsed`.
    panel_collapsed: Cell<bool>,
    /// Set while the shell itself opens or closes the panel.
    updating_panel: Cell<bool>,
    sidebar: Rc<Sidebar>,
    mini_player: Rc<MiniPlayer>,
}

impl Shell {
    fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let navigator = Navigator::new(ctx);
        let sidebar = Sidebar::new(ctx, &navigator);
        navigator.set_index_page({
            let sidebar = Rc::downgrade(&sidebar);
            move || match sidebar.upgrade() {
                Some(sidebar) => sidebar.index_page(),
                None => adw::NavigationPage::builder()
                    .title(tr("Collection"))
                    .build(),
            }
        });
        let panel = NowPlayingPanel::new(ctx);
        let player_bar = PlayerBar::new(ctx);
        let mini_player = MiniPlayer::new(ctx);

        // [content]: the toast overlay wraps the content only, so toasts
        // never cover the player.
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&ctx.offline_banner);
        ctx.toasts.set_child(Some(navigator.sections()));
        ctx.toasts.set_vexpand(true);
        content.append(&ctx.toasts);

        // Wide: sidebar | content | panel, player bar below.
        let panel_split = adw::OverlaySplitView::builder()
            .sidebar_position(gtk::PackType::End)
            .min_sidebar_width(320.0)
            .max_sidebar_width(420.0)
            .sidebar_width_fraction(0.3)
            .show_sidebar(false)
            .content(&adw::LayoutSlot::new("content"))
            .sidebar(&adw::LayoutSlot::new("panel"))
            .build();
        let sidebar_split = adw::OverlaySplitView::builder()
            .sidebar_position(gtk::PackType::Start)
            .min_sidebar_width(sidebar_min_width(200.0))
            .max_sidebar_width(280.0)
            .sidebar_width_fraction(0.2)
            .sidebar(&sidebar.widget)
            .content(&panel_split)
            .build();
        let wide = adw::ToolbarView::builder()
            .bottom_bar_style(adw::ToolbarStyle::RaisedBorder)
            .content(&sidebar_split)
            .build();
        wide.add_bottom_bar(&adw::LayoutSlot::new("player"));

        // Narrow: content with the mini player and the now-playing sheet,
        // view switcher below.
        let sheet = adw::BottomSheet::builder()
            .show_drag_handle(true)
            .full_width(true)
            .modal(true)
            .content(&adw::LayoutSlot::new("content"))
            .bottom_bar(&mini_player.widget)
            .sheet(&adw::LayoutSlot::new("panel"))
            .build();
        let switcher = adw::ViewSwitcherBar::builder()
            .stack(navigator.sections())
            .reveal(true)
            .build();
        sheet
            .bind_property("open", &switcher, "reveal")
            .invert_boolean()
            .sync_create()
            .build();
        let narrow = adw::ToolbarView::builder().content(&sheet).build();
        narrow.add_bottom_bar(&switcher);

        let multi = adw::MultiLayoutView::new();
        let wide_layout = adw::Layout::new(&wide);
        wide_layout.set_name(Some("wide"));
        multi.add_layout(wide_layout);
        let narrow_layout = adw::Layout::new(&narrow);
        narrow_layout.set_name(Some("narrow"));
        multi.add_layout(narrow_layout);
        multi.set_child("content", &content);
        multi.set_child("panel", &panel.widget);
        multi.set_child("player", &player_bar.widget);
        multi.set_layout_name("wide");

        let root = adw::BreakpointBin::builder()
            .width_request(360)
            .height_request(294)
            .child(&multi)
            .build();
        // Only one breakpoint applies at a time (the last that matches), so
        // each repeats the setters of the wider ones. Every width has a
        // twin for short windows (≤ 640 px high), which also gives the
        // window the `short` class (smaller page header artwork).
        let collapsed = (
            panel_split.upcast_ref::<glib::Object>(),
            "collapsed",
            true.to_value(),
        );
        let slim_sidebar = (
            sidebar_split.upcast_ref::<glib::Object>(),
            "min-sidebar-width",
            sidebar_min_width(180.0).to_value(),
        );
        let narrow_layout = (
            multi.upcast_ref::<glib::Object>(),
            "layout-name",
            "narrow".to_value(),
        );
        let levels: [(Option<&str>, Vec<_>); 4] = [
            (None, vec![]),
            (Some("max-width: 1180sp"), vec![collapsed.clone()]),
            (
                Some("max-width: 900sp"),
                vec![collapsed.clone(), slim_sidebar.clone()],
            ),
            (
                Some("max-width: 720sp"),
                vec![collapsed, slim_sidebar, narrow_layout],
            ),
        ];
        // (breakpoint, compact player bar, short window)
        let mut breakpoints = Vec::new();
        for (level, (width, setters)) in levels.iter().enumerate() {
            for short in [false, true] {
                let condition = match (width, short) {
                    (None, false) => continue,
                    (None, true) => SHORT.to_owned(),
                    (Some(width), false) => (*width).to_owned(),
                    (Some(width), true) => format!("{width} and {SHORT}"),
                };
                let breakpoint = adw::Breakpoint::new(
                    adw::BreakpointCondition::parse(&condition)
                        .expect("valid breakpoint condition"),
                );
                let setters: Vec<(&glib::Object, &str, &glib::Value)> = setters
                    .iter()
                    .map(|(object, property, value)| (*object, *property, value))
                    .collect();
                breakpoint.add_setters(&setters);
                root.add_breakpoint(breakpoint.clone());
                breakpoints.push((breakpoint, level >= 2, short));
            }
        }

        let shell = Rc::new_cyclic(|this| Self {
            this: this.clone(),
            ctx: Rc::downgrade(ctx),
            root,
            navigator,
            multi: multi.clone(),
            sidebar_split,
            panel_split,
            sheet,
            panel,
            player_bar,
            narrow: Cell::new(false),
            panel_collapsed: Cell::new(false),
            updating_panel: Cell::new(false),
            sidebar,
            mini_player,
        });

        // Breakpoint B and narrower: compact player bar. Short windows: the
        // `short` class.
        shell.root.connect_current_breakpoint_notify({
            let shell = Rc::downgrade(&shell);
            move |root| {
                let Some(shell) = shell.upgrade() else {
                    return;
                };
                let current = root.current_breakpoint();
                let (compact, short) = breakpoints
                    .iter()
                    .find(|(breakpoint, _, _)| current.as_ref() == Some(breakpoint))
                    .map_or((false, false), |(_, compact, short)| (*compact, *short));
                shell.player_bar.set_compact(compact);
                if let Some(ctx) = shell.ctx.upgrade() {
                    if short {
                        ctx.window.add_css_class("short");
                    } else {
                        ctx.window.remove_css_class("short");
                    }
                }
            }
        });
        multi.connect_layout_name_notify({
            let shell = Rc::downgrade(&shell);
            move |multi| {
                if let Some(shell) = shell.upgrade() {
                    shell.set_narrow(multi.layout_name().as_deref() == Some("narrow"));
                }
            }
        });

        ctx.set_panel_handler({
            let shell = Rc::downgrade(&shell);
            move |tab| {
                if let Some(shell) = shell.upgrade() {
                    shell.show_panel(tab);
                }
            }
        });
        // The panel closed by other means: Esc or a click outside (overlaid),
        // dragging the sheet down; or the sheet opened by tapping the mini
        // player (opens on Playing).
        // The split view also shows/hides its sidebar by itself when it
        // (un)collapses: that is not the user's doing, so it is undone.
        shell.panel_split.connect_show_sidebar_notify({
            let shell = Rc::downgrade(&shell);
            move |split| {
                let Some(shell) = shell.upgrade() else {
                    return;
                };
                if shell.updating_panel.get() || shell.narrow.get() {
                    return;
                }
                if split.is_collapsed() != shell.panel_collapsed.replace(split.is_collapsed()) {
                    shell.restore_panel_split();
                } else if !split.shows_sidebar() {
                    shell.panel_changed(None);
                }
            }
        });
        shell.panel_split.connect_collapsed_notify({
            let shell = Rc::downgrade(&shell);
            move |split| {
                if let Some(shell) = shell.upgrade() {
                    shell.panel_collapsed.set(split.is_collapsed());
                    if !shell.updating_panel.get() && !shell.narrow.get() {
                        shell.restore_panel_split();
                    }
                }
            }
        });
        // Remember the panel for the next start (not while debugging, so
        // screenshots do not change the user's settings).
        if !super::debug::screenshot_mode() {
            let settings = Rc::downgrade(&ctx.settings);
            ctx.connect_panel_changed(move |tab| {
                let Some(settings) = settings.upgrade() else {
                    return;
                };
                settings.update(|s| {
                    s.panel_open = tab.is_some();
                    if let Some(tab) = tab {
                        s.panel_tab = tab_name(tab).to_owned();
                    }
                });
            });
        }
        shell.sheet.connect_open_notify({
            let shell = Rc::downgrade(&shell);
            move |sheet| {
                let Some(shell) = shell.upgrade() else {
                    return;
                };
                if shell.updating_panel.get() || !shell.narrow.get() {
                    return;
                }
                if !sheet.is_open() {
                    shell.panel_changed(None);
                } else if shell.ctx.upgrade().is_some_and(|ctx| ctx.panel().is_none()) {
                    shell.panel.show_tab(PanelTab::Playing);
                    shell.panel_changed(Some(PanelTab::Playing));
                }
            }
        });
        shell
    }

    /// Shows the docked/overlaid panel if (and only if) it is open.
    fn restore_panel_split(&self) {
        let open = self.ctx.upgrade().and_then(|ctx| ctx.panel()).is_some();
        if self.panel_split.shows_sidebar() != open {
            self.updating_panel.set(true);
            self.panel_split.set_show_sidebar(open);
            self.updating_panel.set(false);
        }
    }

    /// Reopens the panel as it was last time, in the wide layouts (a
    /// bottom sheet that opens by itself would cover the window).
    fn restore_panel_state(&self, settings: &settings::Settings) {
        if !settings.panel_open || self.multi.layout_name().as_deref() == Some("narrow") {
            return;
        }
        if let (Some(ctx), Some(tab)) = (self.ctx.upgrade(), tab_from_name(&settings.panel_tab)) {
            ctx.show_panel(Some(tab));
        }
    }

    fn show_panel(&self, tab: Option<PanelTab>) {
        // libadwaita animates the reveal by the sidebar's last width, which
        // is 0 before the split view was first allocated: the animation
        // then runs on NaN ("attempt to allocate AdwBin widget with width
        // -2147482624"). Wait for a first allocation.
        if tab.is_some() && !self.narrow.get() && self.panel_split.width() == 0 {
            let this = self.this.clone();
            self.panel_split.add_tick_callback(move |split, _| {
                if split.width() == 0 {
                    return glib::ControlFlow::Continue;
                }
                if let Some(shell) = this.upgrade() {
                    shell.show_panel(tab);
                }
                glib::ControlFlow::Break
            });
            return;
        }
        if let Some(tab) = tab {
            self.panel.show_tab(tab);
        }
        self.updating_panel.set(true);
        if self.narrow.get() {
            self.sheet.set_open(tab.is_some());
        } else {
            self.panel_split.set_show_sidebar(tab.is_some());
        }
        self.updating_panel.set(false);
        self.panel_changed(tab);
    }

    fn panel_changed(&self, tab: Option<PanelTab>) {
        if let Some(ctx) = self.ctx.upgrade()
            && ctx.panel() != tab
        {
            ctx.panel_changed(tab);
        }
    }

    /// Moves to the wide or narrow layout: the window's `narrow` class,
    /// Collection's root and where the open panel is shown.
    fn set_narrow(&self, narrow: bool) {
        if self.narrow.replace(narrow) == narrow {
            return;
        }
        if let Some(ctx) = self.ctx.upgrade() {
            if narrow {
                ctx.window.add_css_class("narrow");
            } else {
                ctx.window.remove_css_class("narrow");
            }
        }
        self.navigator.set_narrow(narrow);
        self.panel.set_narrow(narrow);
        let tab = self.ctx.upgrade().and_then(|ctx| ctx.panel());
        self.updating_panel.set(true);
        self.sheet.set_open(tab.is_some() && narrow);
        if narrow || self.panel_split.width() > 0 {
            self.panel_split.set_show_sidebar(tab.is_some() && !narrow);
        }
        self.updating_panel.set(false);
        if !narrow && tab.is_some() && self.panel_split.width() == 0 {
            // First time in the wide layout: reveal once allocated.
            self.show_panel(tab);
        }
    }

    /// F6 / Shift+F6 (§5.1): moves the focus to the next (previous) region
    /// that is showing: sidebar → content → now-playing panel → player.
    fn cycle_region(&self, window: &adw::ApplicationWindow, backward: bool) {
        let narrow = self.narrow.get();
        let panel_open = self.ctx.upgrade().is_some_and(|ctx| ctx.panel().is_some());
        let content = self
            .navigator
            .visible_view()
            .visible_page()
            .map(|page| page.upcast::<gtk::Widget>());
        let player = if narrow {
            self.mini_player.widget.clone()
        } else {
            self.player_bar.widget.clone().upcast()
        };
        let regions: Vec<gtk::Widget> = [
            (!narrow && self.sidebar_split.shows_sidebar())
                .then(|| self.sidebar.widget.clone().upcast()),
            content,
            panel_open.then(|| self.panel.widget.clone().upcast()),
            Some(player),
        ]
        .into_iter()
        .flatten()
        .filter(|region| region.is_mapped())
        .collect();
        if regions.is_empty() {
            return;
        }
        let focus = gtk::prelude::RootExt::focus(window);
        let current = focus.as_ref().and_then(|focus| {
            regions
                .iter()
                .position(|region| focus == region || focus.is_ancestor(region))
        });
        let n = regions.len();
        let start = match (current, backward) {
            (Some(i), false) => i + 1,
            (Some(i), true) => i + n - 1,
            (None, false) => 0,
            (None, true) => n - 1,
        };
        for step in 0..n {
            let index = if backward {
                (start + n - step) % n
            } else {
                (start + step) % n
            };
            if focus_first(window, &regions[index]) {
                return;
            }
        }
    }

    /// Ctrl+F: the Search section at its root, with the focus in its
    /// search entry.
    fn focus_search(&self) {
        self.navigator.show_section("search");
        self.navigator.pop_to_root("search");
        let view = self.navigator.view("search").clone();
        glib::idle_add_local_once(move || {
            if let Some(entry) = view
                .visible_page()
                .and_then(|page| find_descendant::<gtk::SearchEntry>(page.upcast_ref()))
            {
                entry.grab_focus();
            }
        });
    }
}

/// `base`, or more when the longest sidebar row ("Recently Played") needs
/// it in the UI language: its label plus the icon, spacing and padding
/// around it.
fn sidebar_min_width(base: f64) -> f64 {
    let label = gtk::Label::new(Some(tr("Recently Played")));
    let (_, natural, _, _) = label.measure(gtk::Orientation::Horizontal, -1);
    base.max(f64::from(natural + 64))
}

fn tab_name(tab: PanelTab) -> &'static str {
    match tab {
        PanelTab::Playing => "playing",
        PanelTab::Lyrics => "lyrics",
        PanelTab::Queue => "queue",
    }
}

fn tab_from_name(name: &str) -> Option<PanelTab> {
    match name {
        "playing" => Some(PanelTab::Playing),
        "lyrics" => Some(PanelTab::Lyrics),
        "queue" => Some(PanelTab::Queue),
        _ => None,
    }
}

/// Moves the focus to the first focusable widget in `region`, the way Tab
/// would enter it.
fn focus_first(window: &adw::ApplicationWindow, region: &gtk::Widget) -> bool {
    let previous = gtk::prelude::RootExt::focus(window);
    // Forget the region's last focus child, so traversal starts at its top.
    gtk::prelude::RootExt::set_focus(window, None::<&gtk::Widget>);
    if region.child_focus(gtk::DirectionType::TabForward) {
        return true;
    }
    if let Some(previous) = previous {
        previous.grab_focus();
    }
    false
}

/// The first descendant of `widget` of type `T`, depth first.
fn find_descendant<T: IsA<gtk::Widget>>(widget: &gtk::Widget) -> Option<T> {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Ok(found) = current.clone().downcast::<T>() {
            return Some(found);
        }
        if let Some(found) = find_descendant::<T>(&current) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}
