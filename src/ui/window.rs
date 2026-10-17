//! The main window: a stack that switches between loading, sign-in and the
//! library once a session exists.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use adw::prelude::*;
use gtk::{gio, glib};

use super::library::TrackList;
use super::player_bar::PlayerBar;
use crate::api::{self, Api, CachedHead, FavoritesUpdate};
use crate::cache::{Cache, CachedCollection};
use crate::covers::CoverLoader;
use crate::model::TrackInfo;
use crate::player::{Event, Player, State};
use crate::{config, runtime, session};

pub fn build(app: &adw::Application) {
    // Launching Swell again only raises the running window.
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

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
    let playback = Playback::new();
    if let Some(playback) = &playback {
        playback.bar.widget.set_visible(false);
        toolbar.add_bottom_bar(&playback.bar.widget);
    }
    let ui = Rc::new(Ui {
        window,
        stack,
        title,
        menu_button,
        covers: CoverLoader::new(http.clone()),
        http,
        playback,
        play_request: Cell::new(0),
        library_task: RefCell::default(),
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
    /// `None` if GStreamer's playbin3 is not installed.
    playback: Option<Playback>,
    /// Bumped on every track activation, so a slow stream lookup that was
    /// overtaken by a newer one is ignored.
    play_request: Cell<u64>,
    /// Loads and syncs the library; aborting it also aborts the sync on the
    /// I/O runtime.
    library_task: RefCell<Option<glib::JoinHandle<()>>>,
}

struct Playback {
    player: Rc<Player>,
    bar: Rc<PlayerBar>,
    requested_at: Rc<Cell<Option<Instant>>>,
}

impl Playback {
    fn new() -> Option<Self> {
        let bar_slot: Rc<OnceCell<Rc<PlayerBar>>> = Rc::default();
        let requested_at: Rc<Cell<Option<Instant>>> = Rc::default();
        let player = Player::new({
            let bar_slot = bar_slot.clone();
            let requested_at = requested_at.clone();
            move |event| {
                let Some(bar) = bar_slot.get() else { return };
                match event {
                    Event::State(state) => {
                        if *state == State::Playing
                            && let Some(at) = requested_at.take()
                        {
                            log::info!(
                                "playback started {} ms after the track was chosen",
                                at.elapsed().as_millis()
                            );
                            crate::timing::mark("playing");
                        }
                        bar.set_state(*state);
                    }
                    Event::Finished => bar.set_state(State::Stopped),
                    Event::Error(message) => bar.show_error(message),
                }
            }
        });
        let player = match player {
            Ok(player) => Rc::new(player),
            Err(err) => {
                log::error!("playback unavailable: {err:#}");
                return None;
            }
        };
        let bar = PlayerBar::new(player.clone());
        let _ = bar_slot.set(bar.clone());
        Some(Self {
            player,
            bar,
            requested_at,
        })
    }
}

impl Ui {
    fn set_page(&self, name: &str, page: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.stack.child_by_name(name) {
            self.stack.remove(&old);
        }
        self.stack.add_named(page, Some(name));
        self.stack.set_visible_child_name(name);
    }

    /// Stops everything tied to the signed-in session: library sync, pending
    /// stream lookups and playback.
    fn stop_session_work(&self) {
        if let Some(task) = self.library_task.take() {
            task.abort();
        }
        self.play_request.set(self.play_request.get() + 1);
        if let Some(playback) = &self.playback {
            playback.player.stop();
            playback.bar.widget.set_visible(false);
        }
    }

    fn show_login(self: &Rc<Self>, error: Option<String>) {
        self.stop_session_work();
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

    /// Stops session work, deletes the tokens and cached library, and goes
    /// back to the sign-in page with `reason`.
    fn sign_out(self: &Rc<Self>, api: Arc<Api>, reason: Option<String>) {
        self.stop_session_work();
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

    fn show_library(self: &Rc<Self>, api: Arc<Api>) {
        let sign_out = gio::ActionEntry::builder("sign-out")
            .activate({
                let ui = Rc::downgrade(self);
                let api = api.clone();
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(ui) = ui.upgrade() {
                        ui.sign_out(api.clone(), None);
                    }
                }
            })
            .build();
        self.window.add_action_entries([sign_out]);
        self.menu_button.set_visible(true);
        self.title.set_title("Favourite Tracks");
        self.title.set_subtitle("Loading…");

        let list = Rc::new(TrackList::new(self.covers.clone()));
        self.set_page("library", &list.widget);
        list.connect_track_activated({
            let ui = Rc::downgrade(self);
            let api = api.clone();
            move |track| {
                if let Some(ui) = ui.upgrade() {
                    ui.play(api.clone(), track);
                }
            }
        });

        let task = glib::spawn_future_local(self.clone().load_library(api, list));
        if let Some(old) = self.library_task.replace(Some(task)) {
            old.abort();
        }
    }

    async fn load_library(self: Rc<Self>, api: Arc<Api>, list: Rc<TrackList>) {
        let user_id = api.client.get_user_id();
        let (cache, cached) = runtime::spawn(async move { Ok(open_cache(user_id).await) })
            .await
            .unwrap_or((None, None));
        let head = cached.as_ref().map(CachedHead::of);
        if let Some(cached) = cached {
            list.append(cached.tracks);
            self.title.set_subtitle(&format!("{} tracks", list.len()));
            crate::timing::mark_after_next_frame(&list.widget, "cached favourites on screen");
        }

        let (updates_tx, mut updates) = tokio::sync::mpsc::unbounded_channel();
        let sync =
            runtime::runtime().spawn(api.clone().sync_favorite_tracks(cache, head, updates_tx));
        let _sync = runtime::AbortOnDrop(sync.abort_handle());

        let mut first = true;
        let mut finished = false;
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
                Ok(FavoritesUpdate::Finished) => finished = true,
                Err(err) if api::is_auth_error(&err) => {
                    log::warn!("the session is no longer valid: {err:#}");
                    self.sign_out(
                        api,
                        Some("Your TIDAL session has expired. Please sign in again.".into()),
                    );
                    return;
                }
                Err(err) => {
                    log::warn!("loading favourites failed: {err:#}");
                    break;
                }
            }
            first = false;
            self.title.set_subtitle(&format!("{} tracks", list.len()));
        }
        if !finished {
            self.title.set_subtitle("Could not load all favourites");
            return;
        }
        crate::timing::mark("favourites up to date");

        if std::env::var_os("SWELL_BENCH_PLAY").is_some()
            && let Some(track) = list.first_where(|track| track.hires)
        {
            self.play(api, track);
        }
    }
}

impl Ui {
    fn play(self: &Rc<Self>, api: Arc<Api>, track: TrackInfo) {
        let Some(playback) = &self.playback else {
            self.title
                .set_subtitle("Playback needs GStreamer's playbin3");
            return;
        };
        let request = self.play_request.get() + 1;
        self.play_request.set(request);
        playback.requested_at.set(Some(Instant::now()));
        playback.player.stop();
        playback.bar.widget.set_visible(true);
        playback.bar.show_loading(&track);
        playback.bar.set_state(State::Buffering);

        let ui = self.clone();
        glib::spawn_future_local(async move {
            let track_id = track.id;
            let stream = runtime::spawn(async move { api.stream(track_id).await }).await;
            let Some(playback) = &ui.playback else { return };
            if ui.play_request.get() != request {
                return;
            }
            match stream {
                Ok(stream) => {
                    log::info!("playing track {track_id} as {}", stream.format);
                    playback.bar.show_format(&track, &stream.format);
                    playback.player.play(&stream.uri);
                }
                Err(err) => {
                    log::warn!("could not get a stream for track {track_id}: {err:#}");
                    playback
                        .bar
                        .show_error(&format!("Cannot play this track: {err}"));
                }
            }
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
