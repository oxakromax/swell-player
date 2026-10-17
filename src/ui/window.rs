//! The main window: a stack that switches between loading, sign-in and the
//! signed-in shell.
//!
//! PROVISIONAL shell: content navigation and the player bar only. The full
//! shell of docs/design.md §2 replaces `build_shell`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};

use super::context::Ctx;
use super::nav::{Navigator, Route};
use super::widgets::player_bar::PlayerBar;
use crate::api::{self, Api};
use crate::covers::CoverLoader;
use crate::library::Library;
use crate::playback::Playback;
use crate::{config, runtime, session, settings};

pub fn build(app: &adw::Application) {
    // Launching Swell again only raises the running window.
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    let settings = settings::Store::load();
    let current = settings.get();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(config::APP_NAME)
        .default_width(current.window_width)
        .default_height(current.window_height)
        .maximized(current.window_maximized)
        .width_request(360)
        .height_request(294)
        .build();

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    let spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    stack.add_named(&spinner, Some("loading"));
    window.set_content(Some(&stack));
    window.present();

    let http = api::http_client();
    api::preconnect(&http);
    let ui = Rc::new(Ui {
        window,
        stack,
        covers: CoverLoader::new(http.clone()),
        http,
        settings,
        session: RefCell::default(),
    });
    ui.window.connect_close_request({
        let ui = Rc::downgrade(&ui);
        move |window| {
            if let Some(ui) = ui.upgrade() {
                let (width, height) = window.default_size();
                let maximized = window.is_maximized();
                ui.settings.update(|s| {
                    if !maximized {
                        s.window_width = width;
                        s.window_height = height;
                    }
                    s.window_maximized = maximized;
                });
                ui.settings.save_now();
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
    glib::spawn_future_local(async move {
        let stored = runtime::spawn(session::load()).await;
        crate::timing::mark("session read from the keyring");
        match stored {
            Ok(Some(stored)) => ui.show_shell(Api::restore(stored, ui.http.clone())),
            Ok(None) => ui.show_login(None),
            Err(err) => ui.show_login(Some(format!("Could not open the keyring: {err:#}"))),
        }
    });
}

struct Ui {
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    covers: Rc<CoverLoader>,
    http: reqwest::Client,
    settings: Rc<settings::Store>,
    /// The signed-in session's shared state, while signed in.
    session: RefCell<Option<Rc<Ctx>>>,
}

impl Ui {
    fn set_page(&self, name: &str, page: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.stack.child_by_name(name) {
            self.stack.remove(&old);
        }
        self.stack.add_named(page, Some(name));
        self.stack.set_visible_child_name(name);
    }

    /// Stops everything tied to the signed-in session.
    fn end_session(&self) {
        if let Some(ctx) = self.session.take() {
            ctx.playback.stop();
        }
        self.window.remove_action("sign-out");
    }

    fn show_login(self: &Rc<Self>, error: Option<String>) {
        self.end_session();
        if let Some(main) = self.stack.child_by_name("main") {
            self.stack.remove(&main);
        }
        let ui = self.clone();
        let page = super::login::page(&self.window, self.http.clone(), error, move |api| {
            ui.show_shell(api);
        });
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&page));
        self.set_page("login", &toolbar);
    }

    /// Ends the session, deletes the tokens and cached library, and goes
    /// back to the sign-in page with `reason`.
    fn sign_out(self: &Rc<Self>, reason: Option<String>) {
        let Some(ctx) = self.session.borrow().clone() else {
            return;
        };
        self.end_session();
        let api = ctx.api.clone();
        let ui = self.clone();
        glib::spawn_future_local(async move {
            let error = match runtime::spawn(async move { api.sign_out().await }).await {
                Ok(()) => reason,
                Err(err) => Some(format!(
                    "Could not remove the session from the keyring: {err:#}"
                )),
            };
            ui.show_login(error);
        });
    }

    fn show_shell(self: &Rc<Self>, api: Arc<Api>) {
        let playback = Playback::new(api.clone(), self.settings.clone());
        let library = Library::new(api.clone());
        let toasts = adw::ToastOverlay::new();
        let ctx = Ctx::new(
            self.window.clone(),
            api,
            self.covers.clone(),
            playback,
            library.clone(),
            self.settings.clone(),
            toasts.clone(),
        );
        ctx.set_sign_out({
            let ui = Rc::downgrade(self);
            move |reason| {
                if let Some(ui) = ui.upgrade() {
                    ui.sign_out(reason);
                }
            }
        });
        let sign_out = gio::ActionEntry::builder("sign-out")
            .activate({
                let ui = Rc::downgrade(self);
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(ui) = ui.upgrade() {
                        ui.sign_out(None);
                    }
                }
            })
            .build();
        self.window.add_action_entries([sign_out]);

        let shell = build_shell(&ctx, &toasts);
        self.session.replace(Some(ctx.clone()));
        self.set_page("main", &shell);
        if let Some(login) = self.stack.child_by_name("login") {
            self.stack.remove(&login);
        }

        glib::spawn_future_local({
            let ctx = ctx.clone();
            async move {
                if let Err(err) = library.load().await {
                    ctx.report("Could not load your library", &err);
                }
            }
        });
    }
}

fn build_shell(ctx: &Rc<Ctx>, toasts: &adw::ToastOverlay) -> gtk::Widget {
    let view = adw::NavigationView::new();
    let navigator = Navigator::new(ctx, view.clone());
    toasts.set_child(Some(&view));

    let menu = gio::Menu::new();
    menu.append(Some("Sign Out"), Some("win.sign-out"));

    let sections = gtk::Box::builder().spacing(6).build();
    for (label, route) in [
        ("Home", Route::Home),
        ("Explore", Route::Explore),
        ("Search", Route::Search(None)),
        ("Tracks", Route::Collection(super::nav::CollectionView::Tracks)),
    ] {
        let button = gtk::Button::builder().label(label).css_classes(["flat"]).build();
        let ctx = Rc::downgrade(ctx);
        button.connect_clicked(move |_| {
            if let Some(ctx) = ctx.upgrade() {
                ctx.navigate(route.clone());
            }
        });
        sections.append(&button);
    }
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
        .menu_model(&menu)
        .build();
    sections.append(&menu_button);

    let player_bar = PlayerBar::new(ctx.playback.clone());
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&sections);
    toasts.set_vexpand(true);
    root.append(toasts);
    root.append(&player_bar.widget);
    // The bar lives as long as its widget.
    root.connect_destroy(move |_| {
        let _ = &player_bar;
    });

    navigator.navigate(Route::Home);
    // The navigator lives as long as the shell.
    let keep = RefCell::new(Some(navigator));
    root.connect_destroy(move |_| drop(keep.take()));
    root.upcast()
}
