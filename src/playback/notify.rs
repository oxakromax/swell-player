//! Desktop integration that needs the `GtkApplication`: inhibiting suspend
//! while playing, the "now playing" notification, and the Raise/Quit hooks
//! MPRIS calls.
//!
//! On Windows the notification does not go through `GApplication`: GLib's
//! backend there (since GLib 2.88) shows notifications from a
//! notification-area icon of its own, which it adds on the first
//! notification sent or withdrawn and never removes, beside Swell's tray
//! icon, and leaves behind dead when Swell exits. Swell shows it from its
//! own icon instead (`Desktop::set_notifier`), so only while that is
//! there ("Run in Background").

use std::cell::{Cell, RefCell};
use std::rc::Rc;
#[cfg(not(windows))]
use std::sync::OnceLock;
#[cfg(not(windows))]
use std::time::Duration;

use adw::prelude::*;
#[cfg(not(windows))]
use anyhow::Result;
#[cfg(not(windows))]
use gtk::gio;
use gtk::glib;

use crate::i18n::tr;
use crate::model::TrackInfo;
#[cfg(not(windows))]
use crate::runtime;

type Hook = Rc<dyn Fn()>;
/// Windows: shows a notification (title, body).
#[cfg(windows)]
type Notifier = Rc<dyn Fn(&str, &str)>;

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
    #[cfg(windows)]
    notifier: RefCell<Option<Notifier>>,
}

/// Why Swell keeps the system awake (shown by the desktop).
fn reason() -> &'static str {
    tr("Playing music")
}

/// Windows: GTK cannot inhibit suspend there, so Swell asks the system
/// itself. The request belongs to the calling thread: always the main one.
#[cfg(windows)]
#[derive(Default)]
struct SleepLock;

#[cfg(windows)]
impl SleepLock {
    fn set(self: &Rc<Self>, wanted: bool) {
        const ES_CONTINUOUS: u32 = 0x8000_0000;
        const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetThreadExecutionState(flags: u32) -> u32;
        }
        let flags = if wanted {
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED
        } else {
            ES_CONTINUOUS
        };
        // Safety: it only takes flags, and changes this thread's request.
        if unsafe { SetThreadExecutionState(flags) } == 0 {
            log::info!("cannot inhibit suspend");
        }
    }
}

/// macOS and the BSDs: GTK's inhibition (an IOKit power assertion on
/// macOS, the session manager or portal elsewhere) is all there is.
#[cfg(not(any(target_os = "linux", windows)))]
#[derive(Default)]
struct SleepLock;

#[cfg(not(any(target_os = "linux", windows)))]
impl SleepLock {
    fn set(self: &Rc<Self>, _wanted: bool) {}
}

/// A logind "sleep" inhibitor, held as a file descriptor while wanted.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct SleepLock {
    wanted: Cell<bool>,
    held: RefCell<Option<zbus::OwnedFd>>,
    requesting: Cell<bool>,
    unavailable: Cell<bool>,
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
async fn logind_inhibit() -> Result<zbus::OwnedFd> {
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
        // The application's Quit action saves the window state first.
        if let Some(app) = self.app() {
            if app.lookup_action("quit").is_some() {
                app.activate_action("quit", None);
            } else {
                app.quit();
            }
        }
    }

    /// Keeps the system awake while music plays: a logind sleep lock on
    /// Linux (any systemd desktop) or an execution-state request on
    /// Windows, plus GTK's inhibition (session manager, portal or macOS
    /// power assertion, which also tells the desktop why).
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

    /// Windows: shows the notifications with `notifier` (title, body).
    #[cfg(windows)]
    pub fn set_notifier(&self, notifier: impl Fn(&str, &str) + 'static) {
        self.notifier.replace(Some(Rc::new(notifier)));
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
            // The earlier notification would show the wrong track.
            self.withdraw();
            return;
        }
        log::debug!("notifying track {}", track.id);
        let mut body = track.artist_names();
        if !track.album.is_empty() {
            body = tr("{artists} — {album}")
                .replace("{artists}", &body)
                .replace("{album}", &track.album);
        }
        self.show(app, track, &body);
    }

    #[cfg(windows)]
    fn show(&self, _app: adw::Application, track: &TrackInfo, body: &str) {
        let notifier = self.notifier.borrow().clone();
        if let Some(notifier) = notifier {
            notifier(&track.title, body);
        }
    }

    #[cfg(not(windows))]
    fn show(&self, app: adw::Application, track: &TrackInfo, body: &str) {
        let notification = gio::Notification::new(&track.title);
        notification.set_body(Some(body));
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

    /// Removes the notification shown or about to be (Windows: one shown
    /// stays until it times out).
    fn withdraw(&self) {
        if let Some(task) = self.notification.take() {
            task.abort();
        }
        #[cfg(not(windows))]
        if let Some(app) = self.app() {
            app.withdraw_notification("now-playing");
        }
    }

    /// Removes the notification, the inhibition and the shutdown handler
    /// (the playback object is going away).
    pub fn clear(&self) {
        self.set_playing(false);
        self.withdraw();
        self.detach();
        self.notified.set(None);
    }
}

/// Downloads a cover for the notification into this process's runtime
/// directory.
#[cfg(not(windows))]
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
