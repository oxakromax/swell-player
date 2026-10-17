//! Desktop integration that needs the `GtkApplication`: inhibiting suspend
//! while playing, the "now playing" notification, and the Raise/Quit hooks
//! MPRIS calls.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

use adw::prelude::*;
use anyhow::Result;
use gtk::{gio, glib};

use crate::i18n::tr;
use crate::model::TrackInfo;
use crate::runtime;

type Hook = Rc<dyn Fn()>;

#[derive(Default)]
pub struct Desktop {
    app: RefCell<Option<glib::WeakRef<adw::Application>>>,
    inhibit_cookie: Cell<Option<u32>>,
    raise: RefCell<Option<Hook>>,
    quit: RefCell<Option<Hook>>,
    /// The track the last notification was for.
    notified: Cell<Option<u64>>,
    notification: RefCell<Option<glib::JoinHandle<()>>>,
    shutdown_handler: RefCell<Option<glib::SignalHandlerId>>,
    sleep_lock: Rc<SleepLock>,
}

/// Why Swell keeps the system awake (shown by the desktop).
fn reason() -> &'static str {
    tr("Playing music")
}

/// A logind "sleep" inhibitor, held as a file descriptor while wanted.
#[derive(Default)]
struct SleepLock {
    wanted: Cell<bool>,
    held: RefCell<Option<zbus::zvariant::OwnedFd>>,
    requesting: Cell<bool>,
    unavailable: Cell<bool>,
}

impl SleepLock {
    fn set(self: &Rc<Self>, wanted: bool) {
        self.wanted.set(wanted);
        if !wanted {
            // Closing the descriptor releases the lock.
            self.held.replace(None);
            return;
        }
        if self.held.borrow().is_some() || self.requesting.get() || self.unavailable.get() {
            return;
        }
        self.requesting.set(true);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = runtime::spawn(logind_inhibit()).await;
            this.requesting.set(false);
            match result {
                Ok(fd) if this.wanted.get() => {
                    log::debug!("holding a logind sleep lock");
                    this.held.replace(Some(fd));
                }
                Ok(_) => {}
                Err(err) => {
                    log::info!("cannot inhibit suspend through logind: {err:#}");
                    this.unavailable.set(true);
                }
            }
        });
    }
}

async fn logind_inhibit() -> Result<zbus::zvariant::OwnedFd> {
    static SYSTEM: tokio::sync::OnceCell<zbus::Connection> = tokio::sync::OnceCell::const_new();
    let connection = SYSTEM.get_or_try_init(zbus::Connection::system).await?;
    let reply = connection
        .call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "Inhibit",
            &("sleep", crate::config::APP_NAME, reason(), "block"),
        )
        .await?;
    Ok(reply.body().deserialize()?)
}

/// TIDAL's image URL for an album cover id, as `covers.rs` builds it.
pub fn cover_url(cover: &str, size: u32) -> String {
    format!(
        "https://resources.tidal.com/images/{}/{size}x{size}.jpg",
        cover.replace('-', "/")
    )
}

impl Desktop {
    /// `on_shutdown` runs when the application shuts down.
    pub fn attach(&self, app: &adw::Application, on_shutdown: impl Fn() + 'static) {
        self.detach();
        let handler = app.connect_shutdown(move |_| on_shutdown());
        self.shutdown_handler.replace(Some(handler));
        self.app.replace(Some(app.downgrade()));
    }

    fn detach(&self) {
        if let (Some(app), Some(handler)) = (self.app(), self.shutdown_handler.take()) {
            app.disconnect(handler);
        }
    }

    fn app(&self) -> Option<adw::Application> {
        self.app.borrow().as_ref().and_then(|app| app.upgrade())
    }

    pub fn set_hooks(&self, raise: Hook, quit: Hook) {
        self.raise.replace(Some(raise));
        self.quit.replace(Some(quit));
    }

    pub fn can_raise(&self) -> bool {
        self.raise.borrow().is_some() || self.app().is_some()
    }

    pub fn can_quit(&self) -> bool {
        self.quit.borrow().is_some() || self.app().is_some()
    }

    /// Shows the window: the shell's hook, or the application's window.
    pub fn raise(&self) {
        let hook = self.raise.borrow().clone();
        if let Some(hook) = hook {
            return hook();
        }
        if let Some(app) = self.app() {
            match app.active_window() {
                Some(window) => window.present(),
                None => app.activate(),
            }
        }
    }

    pub fn quit(&self) {
        let hook = self.quit.borrow().clone();
        if let Some(hook) = hook {
            return hook();
        }
        if let Some(app) = self.app() {
            app.quit();
        }
    }

    /// Keeps the system awake while music plays: a logind sleep lock
    /// (works on any systemd desktop) plus GTK's inhibition (session
    /// manager or portal, which also tells the desktop why).
    pub fn set_playing(&self, playing: bool) {
        self.sleep_lock.set(playing);
        let Some(app) = self.app() else {
            return;
        };
        match (playing, self.inhibit_cookie.get()) {
            (true, None) => {
                let window = app.active_window();
                let cookie = app.inhibit(
                    window.as_ref(),
                    gtk::ApplicationInhibitFlags::SUSPEND,
                    Some(reason()),
                );
                if cookie != 0 {
                    self.inhibit_cookie.set(Some(cookie));
                }
            }
            (false, Some(cookie)) => {
                app.uninhibit(cookie);
                self.inhibit_cookie.set(None);
            }
            _ => {}
        }
    }

    /// Tells the desktop about a new track, unless the window is in front.
    pub fn track_changed(&self, track: &TrackInfo) {
        let Some(app) = self.app() else {
            return;
        };
        if self.notified.replace(Some(track.id)) == Some(track.id) {
            return;
        }
        if app.active_window().is_some_and(|window| window.is_active()) {
            return;
        }
        log::debug!("notifying track {}", track.id);
        let notification = gio::Notification::new(&track.title);
        let mut body = track.artist_names();
        if !track.album.is_empty() {
            body = tr("{artists} — {album}")
                .replace("{artists}", &body)
                .replace("{album}", &track.album);
        }
        notification.set_body(Some(&body));
        notification.set_priority(gio::NotificationPriority::Low);

        let cover = track.cover.clone();
        let app = app.downgrade();
        let task = glib::spawn_future_local(async move {
            if let Some(cover) = cover {
                // A cover makes it nicer, but never late.
                let fetch = runtime::spawn(save_cover(cover));
                let timeout = glib::timeout_future(Duration::from_secs(2));
                futures_util::pin_mut!(fetch, timeout);
                if let futures_util::future::Either::Left((Ok(path), _)) =
                    futures_util::future::select(fetch, timeout).await
                {
                    notification.set_icon(&gio::FileIcon::new(&gio::File::for_path(path)));
                }
            }
            if let Some(app) = app.upgrade() {
                app.send_notification(Some("now-playing"), &notification);
            }
        });
        if let Some(old) = self.notification.replace(Some(task)) {
            old.abort();
        }
    }

    /// Removes the notification, the inhibition and the shutdown handler
    /// (the playback object is going away).
    pub fn clear(&self) {
        self.set_playing(false);
        if let Some(app) = self.app() {
            app.withdraw_notification("now-playing");
        }
        if let Some(task) = self.notification.take() {
            task.abort();
        }
        self.detach();
        self.notified.set(None);
    }
}

/// Downloads a cover for the notification into this process's runtime
/// directory.
async fn save_cover(cover: String) -> Result<std::path::PathBuf> {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    let http = HTTP.get_or_init(crate::api::http_client);
    let bytes = http
        .get(cover_url(&cover, 320))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let dir = tokio::task::spawn_blocking(super::rundir::create).await??;
    // A new name per cover: the notification server may read it late.
    let path = dir.join(format!("cover-{}.jpg", cover.replace('/', "-")));
    tokio::fs::write(&path, &bytes).await?;
    let mut entries = tokio::fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let old = entry.path();
        let is_cover = old
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("cover-"));
        if is_cover && old != path {
            let _ = tokio::fs::remove_file(old).await;
        }
    }
    Ok(path)
}
