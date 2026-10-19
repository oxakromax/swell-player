//! Update checks while Swell runs (src/update): the first shortly after it
//! starts, then once a day, and Preferences' Check Now.
//!
//! Where this install replaces itself, a newer release is downloaded and
//! installed when Swell quits; a toast says it is ready, with Restart to
//! install it at once. Elsewhere a toast says that it is out, with Details
//! for its page. Each version is announced once per run. Nothing shows
//! while Swell is up to date, and failures only reach the log unless the
//! check was asked for.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;
use crate::update::install::{self, Prepared};
use crate::update::{self, Release, Version, channel};
use crate::{runtime, settings};

/// The first check leaves the start (the first pages loading) alone.
const FIRST_CHECK: Duration = Duration::from_secs(20);
/// Then one a day.
const INTERVAL_SECS: u32 = 24 * 60 * 60;

/// Where the checks stand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing checked yet in this run.
    Idle,
    Checking,
    Downloading(Version),
    UpToDate,
    /// A newer release, which this install does not install by itself.
    Available(Release),
    /// Downloaded and checked: installed when Swell quits.
    Ready(Version),
    /// The release's disk image, downloaded for the user to open (a macOS
    /// app in a folder Swell cannot write to).
    DiskImage(Version, PathBuf),
    /// The check failed (shown only when it was asked for).
    Failed,
    /// A newer release whose package could not be downloaded.
    DownloadFailed(Release),
}

impl Status {
    /// Whether a check is under way.
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Downloading(_))
    }

    /// What Preferences says about it.
    pub fn describe(&self) -> String {
        let with = |text: &str, version: &dyn std::fmt::Display| {
            text.replace("{version}", &version.to_string())
        };
        match self {
            Self::Idle => with(tr("Version {version}"), &update::CURRENT),
            Self::Checking => tr("Checking…").to_owned(),
            Self::Downloading(version) => with(tr("Downloading Swell {version}…"), version),
            Self::UpToDate => with(tr("Swell {version} is up to date"), &update::CURRENT),
            Self::Available(Release { version, .. }) | Self::DiskImage(version, _) => {
                with(tr("Swell {version} is available"), version)
            }
            Self::Ready(version) => with(tr("Swell {version} is ready to install"), version),
            Self::Failed => tr("Couldn’t check for updates").to_owned(),
            Self::DownloadFailed(release) => {
                with(tr("Couldn’t download Swell {version}"), &release.version)
            }
        }
    }
}

/// What a new release does on this install, for Preferences.
pub fn policy() -> &'static str {
    if channel::current().installs_updates() {
        tr("Downloads new versions and installs them when you quit Swell")
    } else {
        tr("Tells you when a new version is available")
    }
}

thread_local! {
    /// The running checks, while the window holds them.
    static UPDATER: RefCell<Weak<Updater>> = RefCell::default();
}

/// The running checks, if this build checks for updates.
pub fn current() -> Option<Rc<Updater>> {
    UPDATER.with(|updater| updater.borrow().upgrade())
}

type Listener = Rc<dyn Fn(&Status)>;

pub struct Updater {
    window: glib::WeakRef<adw::ApplicationWindow>,
    settings: Rc<settings::Store>,
    status: RefCell<Status>,
    /// The status before the check under way, back after a failure that
    /// was not asked about.
    before: RefCell<Status>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    /// Check Now was pressed during the check under way.
    manual: Cell<bool>,
    /// The signed-in window's toasts; announcements wait while there are
    /// none.
    toasts: RefCell<glib::WeakRef<adw::ToastOverlay>>,
    waiting: RefCell<Option<adw::Toast>>,
    /// The version last announced, and whether as ready to install.
    announced: Cell<Option<(Version, bool)>>,
    listeners: RefCell<Vec<(u64, Listener)>>,
    next_id: Cell<u64>,
}

/// Starts the checks, unless this build does not check for updates (or
/// takes screenshots). The window keeps the result.
pub fn start(
    window: &adw::ApplicationWindow,
    settings: &Rc<settings::Store>,
) -> Option<Rc<Updater>> {
    if !update::enabled() || super::debug::screenshot_mode() {
        return None;
    }
    let updater = Rc::new(Updater {
        window: window.downgrade(),
        settings: settings.clone(),
        status: RefCell::new(Status::Idle),
        before: RefCell::new(Status::Idle),
        task: RefCell::default(),
        manual: Cell::new(false),
        toasts: RefCell::default(),
        waiting: RefCell::default(),
        announced: Cell::new(None),
        listeners: RefCell::default(),
        next_id: Cell::new(1),
    });
    UPDATER.with(|slot| slot.replace(Rc::downgrade(&updater)));
    // Found off the main thread, since it looks at files.
    runtime::runtime().spawn_blocking(|| {
        channel::current();
    });
    glib::timeout_add_local_once(FIRST_CHECK, {
        let updater = Rc::downgrade(&updater);
        move || {
            if let Some(updater) = updater.upgrade() {
                updater.scheduled_check();
            }
        }
    });
    glib::timeout_add_seconds_local(INTERVAL_SECS, {
        let updater = Rc::downgrade(&updater);
        move || match updater.upgrade() {
            Some(updater) => {
                updater.scheduled_check();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        }
    });
    let enabled = Cell::new(settings.get().check_updates);
    settings.connect_changed({
        let updater = Rc::downgrade(&updater);
        move |settings| {
            if enabled.replace(settings.check_updates) == settings.check_updates {
                return;
            }
            if let Some(updater) = updater.upgrade() {
                updater.enabled_changed(settings.check_updates);
            }
        }
    });
    Some(updater)
}

impl Updater {
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Calls `listener` whenever the status changes. Returns an id for
    /// [`Updater::disconnect`].
    pub fn connect_status(&self, listener: impl Fn(&Status) + 'static) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.listeners.borrow_mut().push((id, Rc::new(listener)));
        id
    }

    pub fn disconnect(&self, id: u64) {
        self.listeners
            .borrow_mut()
            .retain(|(other, _)| *other != id);
    }

    fn set_status(&self, status: Status) {
        self.status.replace(status.clone());
        let listeners: Vec<Listener> = self
            .listeners
            .borrow()
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect();
        for listener in listeners {
            listener(&status);
        }
    }

    /// The signed-in window's toasts: announcements show there, and one
    /// that waited for sign-in shows now.
    pub fn set_toasts(&self, overlay: &adw::ToastOverlay) {
        self.toasts.replace(overlay.downgrade());
        if let Some(toast) = self.waiting.take() {
            overlay.add_toast(toast);
        }
    }

    fn scheduled_check(self: &Rc<Self>) {
        if self.settings.get().check_updates {
            self.check(false);
        }
    }

    fn enabled_changed(self: &Rc<Self>, enabled: bool) {
        if enabled {
            if *self.status.borrow() == Status::Idle {
                self.check(false);
            }
            return;
        }
        // Off: nothing more is downloaded, announced or installed.
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.manual.set(false);
        install::unstage();
        if let Some(toast) = self.waiting.take() {
            toast.dismiss();
        }
        self.set_status(Status::Idle);
    }

    /// Checks for a newer release, unless a check is under way already.
    /// `manual` (Check Now): a failure is shown too.
    pub fn check(self: &Rc<Self>, manual: bool) {
        if manual {
            self.manual.set(true);
        }
        if self.task.borrow().is_some() {
            return;
        }
        self.before.replace(self.status());
        self.set_status(Status::Checking);
        let task = glib::spawn_future_local(run_check(Rc::downgrade(self)));
        self.task.replace(Some(task));
    }

    fn finish(&self, status: Status) {
        self.task.take();
        self.manual.set(false);
        self.set_status(status.clone());
        self.announce(&status);
    }

    fn failed(&self, err: &anyhow::Error) {
        log::info!("could not check for updates: {err:#}");
        self.task.take();
        let status = if self.manual.replace(false) {
            Status::Failed
        } else {
            self.before.borrow().clone()
        };
        self.set_status(status);
    }

    /// Shows `status` in the window if it is news: a version not
    /// announced yet in this run, or now ready to install.
    fn announce(&self, status: &Status) {
        let (version, ready) = match status {
            Status::Ready(version) => (*version, true),
            Status::Available(release) | Status::DownloadFailed(release) => {
                (release.version, false)
            }
            Status::DiskImage(version, _) => (*version, false),
            _ => return,
        };
        if self
            .announced
            .get()
            .is_some_and(|(last, was_ready)| last == version && (was_ready || !ready))
        {
            return;
        }
        let Some(toast) = self.toast(status) else {
            return;
        };
        self.announced.set(Some((version, ready)));
        match self.toasts.borrow().upgrade() {
            Some(overlay) => overlay.add_toast(toast),
            None => {
                self.waiting.replace(Some(toast));
            }
        }
    }

    /// A toast saying `status`, with what can be done about it: Restart
    /// (install now), Details (the release's page) or Open (the disk
    /// image). `None` while a check is under way.
    pub fn toast(&self, status: &Status) -> Option<adw::Toast> {
        let title = match status {
            Status::Idle | Status::Checking | Status::Downloading(_) => return None,
            // The release can still be downloaded from its page.
            Status::DownloadFailed(release) => tr("Swell {version} is available")
                .replace("{version}", &release.version.to_string()),
            status => status.describe(),
        };
        let toast = adw::Toast::new(&title);
        let window = self.window.clone();
        match status {
            Status::Ready(_) => with_button(&toast, tr("Restart"), move || {
                let app = window
                    .upgrade()
                    .and_then(|window| window.application())
                    .and_downcast::<adw::Application>();
                if let Some(app) = app {
                    super::window::restart(&app);
                }
            }),
            Status::Available(release) | Status::DownloadFailed(release) => {
                let page = release.page.clone();
                with_button(&toast, tr("Details"), move || {
                    gtk::UriLauncher::new(&page).launch(
                        window.upgrade().as_ref(),
                        gio::Cancellable::NONE,
                        |result| {
                            if let Err(err) = result {
                                log::warn!("could not open the release page: {err}");
                            }
                        },
                    );
                });
            }
            Status::DiskImage(_, path) => {
                let file = gio::File::for_path(path);
                with_button(&toast, tr("Open"), move || {
                    gtk::FileLauncher::new(Some(&file)).launch(
                        window.upgrade().as_ref(),
                        gio::Cancellable::NONE,
                        |result| {
                            if let Err(err) = result {
                                log::warn!("could not open the disk image: {err}");
                            }
                        },
                    );
                });
            }
            _ => {}
        }
        Some(toast)
    }
}

/// Gives `toast` a button that runs `activate`; it then stays until
/// dismissed.
fn with_button(toast: &adw::Toast, label: &str, activate: impl Fn() + 'static) {
    toast.set_button_label(Some(label));
    toast.set_timeout(0);
    toast.connect_button_clicked(move |_| activate());
}

/// One check: the latest release, and what it means for this install.
async fn run_check(updater: Weak<Updater>) {
    let result = runtime::spawn(update::latest()).await;
    let Some(this) = updater.upgrade() else {
        return;
    };
    let release = match result {
        Ok(release) => release,
        Err(err) => return this.failed(&err),
    };
    if !release.is_newer() {
        log::debug!(
            "Swell is up to date (the latest release is {})",
            release.tag
        );
        runtime::runtime().spawn(install::clean(None));
        return this.finish(Status::UpToDate);
    }
    let channel = channel::current().clone();
    log::info!(
        "Swell {} is out ({} install)",
        release.version,
        channel.name()
    );
    if !channel.downloads_updates() {
        return this.finish(Status::Available(release));
    }
    this.set_status(Status::Downloading(release.version));
    drop(this);
    let prepared = runtime::spawn(install::prepare(release.clone(), channel)).await;
    let Some(this) = updater.upgrade() else {
        return;
    };
    let status = match prepared {
        Ok(Prepared::Staged(staged)) => {
            let version = staged.version;
            install::stage(staged);
            Status::Ready(version)
        }
        Ok(Prepared::DiskImage(path)) => Status::DiskImage(release.version, path),
        Ok(Prepared::Announce) => Status::Available(release),
        Err(err) => {
            log::info!("could not download Swell {}: {err:#}", release.version);
            Status::DownloadFailed(release)
        }
    };
    this.finish(status);
}
