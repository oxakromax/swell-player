//! The system media controls of Windows (System Media Transport Controls:
//! media keys, the volume flyout, the lock screen) and macOS (Now Playing
//! in Control Centre, media keys), through souvlaki.
//!
//! They live on the main thread. On Windows they belong to a window, so
//! they start once the application window has a native surface
//! ([`OsControls::attach`]); on macOS they start at once. Button presses
//! arrive on other threads and go to the main thread as [`Command`]s.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use souvlaki::{
    MediaControlEvent, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection,
};
use tokio::sync::mpsc::UnboundedSender;

use super::controls::{Command, Snapshot, Status};
use crate::config;

/// How far the seek buttons without an amount (Windows' fast-forward and
/// rewind) move, in seconds.
const SEEK_STEP: f64 = 10.0;

/// The running controls. Dropping them takes Swell off the system's
/// media controls.
pub struct OsControls {
    inner: Rc<Inner>,
}

struct Inner {
    commands: UnboundedSender<Command>,
    /// The current track, for position requests (which carry no id).
    track_id: Arc<AtomicU64>,
    controls: RefCell<Option<souvlaki::MediaControls>>,
    /// What the controls show.
    shown: RefCell<Snapshot>,
}

impl OsControls {
    pub fn start(commands: UnboundedSender<Command>) -> Self {
        let inner = Rc::new(Inner {
            commands,
            track_id: Arc::default(),
            controls: RefCell::default(),
            shown: RefCell::default(),
        });
        // macOS needs no window; Windows waits for `attach`.
        #[cfg(target_os = "macos")]
        inner.create(None);
        Self { inner }
    }

    /// Windows: starts the controls for the application's window, now or
    /// once it is realized.
    pub fn attach(&self, app: &adw::Application) {
        #[cfg(windows)]
        {
            use gtk::prelude::*;
            if self.inner.controls.borrow().is_some() {
                return;
            }
            let Some(window) = app
                .active_window()
                .or_else(|| app.windows().into_iter().next())
            else {
                return;
            };
            if window.is_realized() {
                self.inner.create_for(&window);
            } else {
                let inner = Rc::downgrade(&self.inner);
                window.connect_realize(move |window| {
                    if let Some(inner) = inner.upgrade() {
                        inner.create_for(window);
                    }
                });
            }
        }
        #[cfg(not(windows))]
        let _ = app;
    }

    /// Shows `snapshot`, changing only what differs.
    pub fn update(&self, snapshot: Snapshot) {
        let id = snapshot.track.as_ref().map_or(0, |track| track.track_id);
        self.inner.track_id.store(id, Ordering::Relaxed);
        let old = self.inner.shown.replace(snapshot);
        self.inner.show(Some(&old));
    }

    /// Updates the position without telling the system (it extrapolates).
    pub fn set_position(&self, position: f64) {
        let mut shown = self.inner.shown.borrow_mut();
        shown.position = position;
        shown.position_at = Some(std::time::Instant::now());
    }

    /// Tells the system about a jump in position (seconds).
    pub fn seeked(&self, position: f64) {
        self.set_position(position);
        let shown = self.inner.shown.borrow().clone();
        if let Some(controls) = self.inner.controls.borrow_mut().as_mut()
            && let Err(err) = controls.set_playback(playback(&shown))
        {
            log::debug!("media controls: could not update the position: {err}");
        }
    }
}

impl Inner {
    #[cfg(windows)]
    fn create_for(&self, window: &gtk::Window) {
        use gtk::prelude::*;
        if self.controls.borrow().is_some() {
            return;
        }
        let Some(surface) = window.surface() else {
            return;
        };
        let Some(surface) = surface.downcast_ref::<gdk_win32::Win32Surface>() else {
            return log::info!("media controls: the window is not a Win32 surface");
        };
        self.create(Some(surface.handle().0));
    }

    fn create(&self, hwnd: Option<*mut std::ffi::c_void>) {
        let config = PlatformConfig {
            display_name: config::APP_NAME,
            dbus_name: "swell_player",
            hwnd,
        };
        let mut controls = match souvlaki::MediaControls::new(config) {
            Ok(controls) => controls,
            Err(err) => return log::warn!("media controls unavailable: {err}"),
        };
        let commands = self.commands.clone();
        let track_id = self.track_id.clone();
        let attached = controls.attach(move |event| {
            if let Some(command) = command(event, track_id.load(Ordering::Relaxed)) {
                let _ = commands.send(command);
            }
        });
        if let Err(err) = attached {
            return log::warn!("media controls unavailable: {err}");
        }
        self.controls.replace(Some(controls));
        self.show(None);
    }

    /// Sends what differs from `old` (everything without it).
    fn show(&self, old: Option<&Snapshot>) {
        let mut controls = self.controls.borrow_mut();
        let Some(controls) = controls.as_mut() else {
            return;
        };
        let new = self.shown.borrow();
        let new_track = old.is_none_or(|old| old.track != new.track);
        if new_track {
            let track = new.track.clone().unwrap_or_default();
            let artists = track.artists.join(", ");
            let metadata = MediaMetadata {
                title: Some(track.title.as_str()),
                album: Some(track.album.as_str()),
                artist: Some(artists.as_str()),
                cover_url: track.art_url.as_deref(),
                duration: (track.length > 0.0).then(|| Duration::from_secs_f64(track.length)),
            };
            if let Err(err) = controls.set_metadata(metadata) {
                log::debug!("media controls: could not show the track: {err}");
            }
        }
        // macOS forgets the position with new metadata: send it again.
        if new_track || old.is_some_and(|old| old.status != new.status) {
            if let Err(err) = controls.set_playback(playback(&new)) {
                log::debug!("media controls: could not show the state: {err}");
            }
        }
    }
}

fn playback(snapshot: &Snapshot) -> MediaPlayback {
    let progress = Some(MediaPosition(Duration::from_secs_f64(
        snapshot.position_now(),
    )));
    match snapshot.status {
        Status::Playing => MediaPlayback::Playing { progress },
        Status::Paused => MediaPlayback::Paused { progress },
        Status::Stopped => MediaPlayback::Stopped,
    }
}

/// What a button press asks of `Playback`.
fn command(event: MediaControlEvent, track_id: u64) -> Option<Command> {
    let signed = |direction: SeekDirection, seconds: f64| match direction {
        SeekDirection::Forward => seconds,
        SeekDirection::Backward => -seconds,
    };
    Some(match event {
        MediaControlEvent::Play => Command::Play,
        MediaControlEvent::Pause => Command::Pause,
        MediaControlEvent::Toggle => Command::PlayPause,
        MediaControlEvent::Next => Command::Next,
        MediaControlEvent::Previous => Command::Previous,
        MediaControlEvent::Stop => Command::Stop,
        MediaControlEvent::Seek(direction) => Command::Seek(signed(direction, SEEK_STEP)),
        MediaControlEvent::SeekBy(direction, by) => {
            Command::Seek(signed(direction, by.as_secs_f64()))
        }
        MediaControlEvent::SetPosition(MediaPosition(at)) => {
            if track_id == 0 {
                return None;
            }
            Command::SetPosition {
                track_id,
                position: at.as_secs_f64(),
            }
        }
        MediaControlEvent::SetVolume(volume) => Command::Volume(volume.clamp(0.0, 1.0)),
        MediaControlEvent::OpenUri(uri) => Command::OpenUri(uri),
        MediaControlEvent::Raise => Command::Raise,
        MediaControlEvent::Quit => Command::Quit,
    })
}
