//! The main window: a stack that switches between loading, sign-in and the
//! library once a session exists.

use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};

use super::library::TrackList;
use crate::api::{self, Api, CachedHead, FavoritesUpdate};
use crate::cache::{Cache, CachedCollection};
use crate::covers::CoverLoader;
use crate::{config, runtime, session};

pub fn build(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(config::APP_NAME)
        .default_width(960)
        .default_height(720)
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

    let title = adw::WindowTitle::new(config::APP_NAME, "");
    let menu = gio::Menu::new();
    menu.append(Some("Sign Out"), Some("win.sign-out"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
        .menu_model(&menu)
        .visible(false)
        .build();
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    header.pack_end(&menu_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    window.set_content(Some(&toolbar));
    window.present();

    let http = api::http_client();
    api::preconnect(&http);
    let ui = Rc::new(Ui {
        window,
        stack,
        title,
        menu_button,
        covers: CoverLoader::new(http.clone()),
        http,
    });
    glib::spawn_future_local(async move {
        let stored = runtime::spawn(session::load()).await;
        crate::timing::mark("session read from the keyring");
        match stored {
            Ok(Some(stored)) => ui.show_library(Api::restore(stored, ui.http.clone())),
            Ok(None) => ui.show_login(None),
            Err(err) => ui.show_login(Some(format!("Could not open the keyring: {err:#}"))),
        }
    });
}

struct Ui {
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    title: adw::WindowTitle,
    menu_button: gtk::MenuButton,
    covers: Rc<CoverLoader>,
    http: reqwest::Client,
}

impl Ui {
    fn set_page(&self, name: &str, page: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.stack.child_by_name(name) {
            self.stack.remove(&old);
        }
        self.stack.add_named(page, Some(name));
        self.stack.set_visible_child_name(name);
    }

    fn show_login(self: &Rc<Self>, error: Option<String>) {
        self.title.set_subtitle("");
        self.menu_button.set_visible(false);
        self.window.remove_action("sign-out");
        if let Some(library) = self.stack.child_by_name("library") {
            self.stack.remove(&library);
        }

        let ui = self.clone();
        let page = super::login::page(&self.window, self.http.clone(), error, move |api| {
            ui.show_library(api);
        });
        self.set_page("login", &page);
    }

    fn show_library(self: &Rc<Self>, api: Arc<Api>) {
        let sign_out = gio::ActionEntry::builder("sign-out")
            .activate({
                let ui = self.clone();
                let api = api.clone();
                move |_: &adw::ApplicationWindow, _, _| {
                    let ui = ui.clone();
                    let api = api.clone();
                    glib::spawn_future_local(async move {
                        if let Err(err) = runtime::spawn(async move { api.sign_out().await }).await
                        {
                            log::warn!("could not remove the stored session: {err:#}");
                        }
                        ui.show_login(None);
                    });
                }
            })
            .build();
        self.window.add_action_entries([sign_out]);
        self.menu_button.set_visible(true);
        self.title.set_title("Favourite Tracks");
        self.title.set_subtitle("Loading…");

        let list = Rc::new(TrackList::new(self.covers.clone()));
        self.set_page("library", &list.widget);

        let ui = self.clone();
        glib::spawn_future_local(async move {
            let user_id = api.client.get_user_id();
            let (cache, cached) = runtime::spawn(open_cache(user_id)).await;
            let head = cached.as_ref().map(CachedHead::of);
            if let Some(cached) = cached {
                list.append(cached.tracks);
                ui.title.set_subtitle(&format!("{} tracks", list.len()));
                crate::timing::mark_after_next_frame(&list.widget, "cached favourites on screen");
            }

            let (updates_tx, mut updates) = tokio::sync::mpsc::unbounded_channel();
            runtime::runtime().spawn(api.sync_favorite_tracks(cache, head, updates_tx));

            let mut first = true;
            while let Some(update) = updates.recv().await {
                match update {
                    Ok(FavoritesUpdate::Append(tracks)) => {
                        list.append(tracks);
                        if first {
                            crate::timing::mark_after_next_frame(
                                &list.widget,
                                "first favourites on screen",
                            );
                        }
                    }
                    Ok(FavoritesUpdate::Replace(tracks)) => list.replace(tracks),
                    Ok(FavoritesUpdate::UpToDate) => {}
                    Err(err) => {
                        log::warn!("loading favourites failed: {err:#}");
                        ui.title.set_subtitle("Could not load all favourites");
                        return;
                    }
                }
                first = false;
                ui.title.set_subtitle(&format!("{} tracks", list.len()));
            }
            crate::timing::mark("favourites up to date");
        });
    }
}

/// Opens the library cache and reads the user's cached favourites. A broken
/// cache is not fatal: the list then just loads from TIDAL.
async fn open_cache(user_id: Option<u64>) -> (Option<Cache>, Option<CachedCollection>) {
    let cache = match Cache::open().await {
        Ok(cache) => cache,
        Err(err) => {
            log::warn!("library cache unavailable: {err:#}");
            return (None, None);
        }
    };
    let cached = match user_id {
        Some(user_id) => cache.favorite_tracks(user_id).await.unwrap_or_else(|err| {
            log::warn!("could not read cached favourites: {err:#}");
            None
        }),
        None => None,
    };
    (Some(cache), cached)
}
