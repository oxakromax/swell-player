//! User preferences, kept as JSON in `settings.json` in the user config
//! directory (`~/.config/swell-player/` on Linux)
//! (no GSettings schema to install, so the binary runs from anywhere).
//! Saved shortly after each change; listeners are told what changed.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Normalization {
    /// Bit-perfect output.
    #[default]
    Off,
    Track,
    Album,
}

/// Highest quality to request; capped by what the login allows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreference {
    #[default]
    Best,
    Lossless,
    High,
    Low,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub quality: QualityPreference,
    pub normalization: Normalization,
    pub gapless: bool,
    /// Crossfade between tracks that end by themselves, in seconds (0: off,
    /// up to 12). Never while the output is bit-perfect, nor between
    /// consecutive tracks of an album (see `playback/transition.rs`).
    pub crossfade_secs: u32,
    /// Short fades when pausing, seeking or changing track by hand, so the
    /// sound never stops with a click.
    pub fade_transitions: bool,
    /// Keep playing with the window closed.
    pub background: bool,
    /// Desktop notification when the track changes while the window is not
    /// focused.
    pub notifications: bool,
    /// Linear 0.0–1.0.
    pub volume: f64,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub window_width: i32,
    pub window_height: i32,
    pub window_maximized: bool,
    /// Restore the last queue (paused) at startup.
    pub restore_session: bool,
    /// The now-playing panel was open when last seen (wide layouts).
    pub panel_open: bool,
    /// Its last tab: "playing", "lyrics" or "queue".
    pub panel_tab: String,
    /// The audio output chosen in Preferences, by its GStreamer device
    /// display name (`Audio/Sink` devices of a `gst::DeviceMonitor`);
    /// `None` follows the system default.
    pub output_device: Option<String>,
    /// Interface language: "auto" (the system's) or a code of
    /// `i18n::languages()` ("en", "es", "pt_BR"…). Read at startup only
    /// (see `i18n::init`).
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            quality: QualityPreference::Best,
            normalization: Normalization::Off,
            gapless: true,
            crossfade_secs: 0,
            fade_transitions: true,
            background: false,
            notifications: true,
            volume: 1.0,
            muted: false,
            shuffle: false,
            repeat: Repeat::Off,
            window_width: 1100,
            window_height: 760,
            window_maximized: false,
            restore_session: true,
            panel_open: false,
            panel_tab: "playing".to_owned(),
            output_device: None,
            language: "auto".to_owned(),
        }
    }
}

type Listener = Rc<dyn Fn(&Settings)>;

/// The shared, observable settings of the running app.
pub struct Store {
    current: RefCell<Settings>,
    listeners: RefCell<Vec<(u64, Listener)>>,
    next_id: Cell<u64>,
    save_pending: Cell<bool>,
    /// False for screenshot/debug runs: changes stay in memory.
    persist: bool,
}

fn path() -> PathBuf {
    glib::user_config_dir()
        .join("swell-player")
        .join("settings.json")
}

/// The saved Language preference, read before anything is translated
/// (the store itself is loaded later, with the window).
pub fn saved_language() -> String {
    std::fs::read(path())
        .ok()
        .map(|data| parse(&data).language)
        .unwrap_or_else(|| Settings::default().language)
}

impl Store {
    pub fn load() -> Rc<Self> {
        let current = std::fs::read(path())
            .ok()
            .map(|data| parse(&data))
            .unwrap_or_default();
        Rc::new(Self {
            current: RefCell::new(current),
            listeners: RefCell::default(),
            next_id: Cell::new(1),
            save_pending: Cell::new(false),
            persist: std::env::var_os("SWELL_SCREENSHOT").is_none(),
        })
    }

    pub fn get(&self) -> Settings {
        self.current.borrow().clone()
    }

    /// Applies `change`; if anything differs, notifies listeners and saves
    /// half a second later (coalescing bursts such as volume drags).
    pub fn update(self: &Rc<Self>, change: impl FnOnce(&mut Settings)) {
        let snapshot = {
            let mut current = self.current.borrow_mut();
            let before = current.clone();
            change(&mut current);
            if *current == before {
                return;
            }
            current.clone()
        };
        // A listener may connect or disconnect others while it runs.
        let listeners: Vec<Listener> = self
            .listeners
            .borrow()
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect();
        for listener in listeners {
            listener(&snapshot);
        }
        if !self.save_pending.replace(true) {
            let this = Rc::downgrade(self);
            glib::timeout_add_local_once(Duration::from_millis(500), move || {
                if let Some(this) = this.upgrade() {
                    this.save_pending.set(false);
                    this.save_now();
                }
            });
        }
    }

    pub fn save_now(&self) {
        if !self.persist {
            return;
        }
        let data = match serde_json::to_vec_pretty(&*self.current.borrow()) {
            Ok(data) => data,
            Err(err) => return log::warn!("could not encode settings: {err}"),
        };
        let path = path();
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let partial = path.with_extension("json.part");
                std::fs::write(&partial, data)?;
                std::fs::rename(&partial, &path)
            });
        if let Err(err) = result {
            log::warn!("could not save settings: {err}");
        }
    }

    /// Calls `listener` after every change. Returns an id for
    /// [`Store::disconnect`].
    pub fn connect_changed(&self, listener: impl Fn(&Settings) + 'static) -> u64 {
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
}

/// Reads settings field by field: a field that does not fit (a hand edit,
/// a value from a newer version) falls back to its default and is logged,
/// instead of discarding every other setting.
fn parse(data: &[u8]) -> Settings {
    let Ok(serde_json::Value::Object(saved)) = serde_json::from_slice(data) else {
        log::warn!("settings.json is unreadable; using the defaults");
        return Settings::default();
    };
    let mut merged = match serde_json::to_value(Settings::default()) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => return Settings::default(),
    };
    for (key, value) in saved {
        let mut candidate = merged.clone();
        candidate.insert(key.clone(), value);
        if serde_json::from_value::<Settings>(serde_json::Value::Object(candidate.clone())).is_ok()
        {
            merged = candidate;
        } else {
            log::warn!("ignoring the invalid setting “{key}”");
        }
    }
    serde_json::from_value(serde_json::Value::Object(merged)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bad_field_keeps_the_others() {
        let settings = parse(br#"{"quality":"lossless","repeat":"sideways","volume":0.3}"#);
        assert_eq!(settings.quality, QualityPreference::Lossless);
        assert_eq!(settings.repeat, Repeat::Off);
        assert_eq!(settings.volume, 0.3);
        assert_eq!(parse(b"garbage"), Settings::default());
    }

    #[test]
    fn unknown_and_missing_fields() {
        let settings: Settings =
            serde_json::from_str(r#"{"volume":0.5,"something_new":1}"#).unwrap();
        assert_eq!(settings.volume, 0.5);
        assert!(settings.gapless);
        assert!(!settings.panel_open);
        assert_eq!(settings.panel_tab, "playing");
        assert_eq!(settings.language, "auto");
        assert_eq!(settings.crossfade_secs, 0);
        assert!(settings.fade_transitions);
    }
}
