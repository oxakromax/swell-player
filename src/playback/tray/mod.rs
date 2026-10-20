//! The tray icon (notification area, status bar), shown while "Run in
//! Background" is on, so a window closed to the background can be brought
//! back and playback driven without it.
//!
//! [`Tray`] is the same on every platform: the `Playback` facade turns it
//! on and off with the setting and keeps it current with [`TrayState`];
//! clicks come back as [`Command`]s on the media controls' channel. The
//! icon itself is a platform backend: a StatusNotifierItem on D-Bus on
//! Linux and the BSDs (`sni.rs`), a notification-area icon on Windows
//! (`windows.rs`) and a status item in the menu bar on macOS (`macos.rs`).
//! A backend that cannot start (no tray host) only logs it.

use std::cell::RefCell;

use tokio::sync::mpsc::UnboundedSender;

use super::controls::{Command, Status};
use crate::config;
use crate::i18n::tr;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(unix, not(target_os = "macos")))]
mod sni;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
use macos::Backend;
#[cfg(all(unix, not(target_os = "macos")))]
use sni::Backend;
#[cfg(windows)]
use windows::Backend;

/// Everything the icon and its menu show, with their labels translated.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayState {
    /// The tooltip's title: the application's name.
    pub title: String,
    /// The tooltip's second line: the current track ("Title — Artist"),
    /// if any.
    pub track: Option<String>,
    pub status: Status,
    pub can_play: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub labels: Labels,
}

/// The menu's labels.
#[derive(Clone, Debug, PartialEq)]
pub struct Labels {
    pub show: String,
    pub play: String,
    pub pause: String,
    pub next: String,
    pub previous: String,
    pub quit: String,
}

impl Labels {
    fn current() -> Self {
        Self {
            show: tr("Show Window").to_owned(),
            play: tr("Play").to_owned(),
            pause: tr("Pause").to_owned(),
            next: tr("Next Track").to_owned(),
            previous: tr("Previous Track").to_owned(),
            quit: tr("Quit").to_owned(),
        }
    }
}

impl TrayState {
    pub fn new(
        track: Option<String>,
        status: Status,
        can_play: bool,
        can_go_next: bool,
        can_go_previous: bool,
    ) -> Self {
        Self {
            title: config::APP_NAME.to_owned(),
            track,
            status,
            can_play,
            can_go_next,
            can_go_previous,
            labels: Labels::current(),
        }
    }

    /// The tooltip in one string, for platforms with a single line (the
    /// StatusNotifierItem has a title and a description).
    #[cfg_attr(all(unix, not(target_os = "macos")), allow(dead_code))]
    pub fn tooltip(&self) -> String {
        match &self.track {
            Some(track) => format!("{}\n{track}", self.title),
            None => self.title.clone(),
        }
    }

    /// What the play/pause item says and does.
    pub fn play_pause(&self) -> (&str, Command) {
        match self.status {
            Status::Playing => (&self.labels.pause, Command::Pause),
            Status::Paused | Status::Stopped => (&self.labels.play, Command::Play),
        }
    }
}

impl Default for TrayState {
    fn default() -> Self {
        Self::new(None, Status::Stopped, false, false, false)
    }
}

/// The tray icon, while shown. Lives on the main thread.
pub struct Tray {
    commands: UnboundedSender<Command>,
    backend: RefCell<Option<Backend>>,
    /// The last state given, for an icon shown later.
    state: RefCell<TrayState>,
}

impl Tray {
    /// A tray icon, hidden until [`Tray::set_visible`].
    pub fn new(commands: UnboundedSender<Command>) -> Self {
        Self {
            commands,
            backend: RefCell::default(),
            state: RefCell::default(),
        }
    }

    /// Shows or removes the icon.
    pub fn set_visible(&self, visible: bool) {
        if !visible {
            // Dropping the backend removes the icon.
            self.backend.take();
            return;
        }
        if self.backend.borrow().is_some() {
            return;
        }
        match Backend::new(self.commands.clone(), &self.state.borrow()) {
            Ok(backend) => {
                self.backend.replace(Some(backend));
            }
            Err(err) => log::warn!("no tray icon: {err:#}"),
        }
    }

    /// Updates the icon's tooltip and menu, if they changed.
    pub fn update(&self, state: TrayState) {
        if *self.state.borrow() == state {
            return;
        }
        if let Some(backend) = &*self.backend.borrow() {
            backend.update(&state);
        }
        self.state.replace(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_pause_follows_the_status() {
        let mut state = TrayState::default();
        assert_eq!(state.play_pause().1, Command::Play);
        state.status = Status::Playing;
        assert_eq!(state.play_pause().1, Command::Pause);
    }

    #[test]
    fn tooltip_has_the_track_on_a_second_line() {
        let mut state = TrayState::default();
        assert_eq!(state.tooltip(), config::APP_NAME);
        state.track = Some("Song — Artist".to_owned());
        assert_eq!(
            state.tooltip(),
            format!("{}\nSong — Artist", config::APP_NAME)
        );
    }
}
