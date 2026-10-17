//! MPRIS 2 (`org.mpris.MediaPlayer2` and `.Player`) on the session bus, so
//! desktop media controls and media keys drive Swell.
//!
//! The D-Bus side runs on the I/O runtime (zbus on tokio). It answers
//! property reads from a [`Snapshot`] the main thread keeps current, and
//! sends method calls back to the main thread as [`Command`]s; the
//! `Playback` facade does the rest. Nothing here touches GTK.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::config;
use crate::runtime;
use crate::settings::Repeat;

const PATH: &str = "/org/mpris/MediaPlayer2";
const BUS_NAME: &str = "org.mpris.MediaPlayer2.SwellPlayer";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
const TRACK_PATH: &str = "/io/github/oxakromax/SwellPlayer/track";

/// A request from a media controller, for the main thread.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Relative, in seconds.
    Seek(f64),
    /// Absolute, in seconds, if `track_id` is still current.
    SetPosition {
        track_id: u64,
        position: f64,
    },
    Volume(f64),
    Shuffle(bool),
    Repeat(Repeat),
    /// A `tidal://` or TIDAL web link to open in the app.
    OpenUri(String),
    Raise,
    Quit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub track_id: u64,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    /// Seconds.
    pub length: f64,
    pub art_url: Option<String>,
    pub url: String,
    pub track_number: u32,
    pub disc_number: u32,
}

/// Everything the bus can read.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub status: Status,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// Slider position 0–1.
    pub volume: f64,
    pub track: Option<Metadata>,
    /// Seconds, as of `position_at` (advancing from then while playing).
    pub position: f64,
    pub position_at: Option<Instant>,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play: bool,
    pub can_seek: bool,
    pub can_raise: bool,
    pub can_quit: bool,
}

impl Snapshot {
    fn position_us(&self) -> i64 {
        let mut position = self.position;
        if self.status == Status::Playing
            && let Some(at) = self.position_at
        {
            position += at.elapsed().as_secs_f64();
        }
        if let Some(track) = &self.track
            && track.length > 0.0
        {
            position = position.min(track.length);
        }
        (position.max(0.0) * 1e6) as i64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prop {
    Status,
    Loop,
    Shuffle,
    Volume,
    Metadata,
    CanGoNext,
    CanGoPrevious,
    CanPlay,
    CanSeek,
    CanRaise,
    CanQuit,
}

fn changes(old: &Snapshot, new: &Snapshot) -> Vec<Prop> {
    let mut props = Vec::new();
    let mut check = |changed: bool, prop| {
        if changed {
            props.push(prop);
        }
    };
    check(old.status != new.status, Prop::Status);
    check(old.repeat != new.repeat, Prop::Loop);
    check(old.shuffle != new.shuffle, Prop::Shuffle);
    check((old.volume - new.volume).abs() > 1e-6, Prop::Volume);
    check(old.track != new.track, Prop::Metadata);
    check(old.can_go_next != new.can_go_next, Prop::CanGoNext);
    check(
        old.can_go_previous != new.can_go_previous,
        Prop::CanGoPrevious,
    );
    check(old.can_play != new.can_play, Prop::CanPlay);
    check(old.can_seek != new.can_seek, Prop::CanSeek);
    check(old.can_raise != new.can_raise, Prop::CanRaise);
    check(old.can_quit != new.can_quit, Prop::CanQuit);
    props
}

type Shared = Arc<Mutex<Snapshot>>;

fn read(shared: &Shared) -> std::sync::MutexGuard<'_, Snapshot> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The running service. Dropping it leaves the bus.
pub struct Mpris {
    shared: Shared,
    connection: Arc<OnceLock<zbus::Connection>>,
}

impl Mpris {
    /// Connects in the background; method calls arrive on `commands`.
    pub fn start(commands: UnboundedSender<Command>) -> Self {
        let shared = Shared::default();
        let connection = Arc::new(OnceLock::new());
        let (state, slot) = (shared.clone(), connection.clone());
        runtime::runtime().spawn(async move {
            match connect(state, commands).await {
                Ok(connection) => {
                    let _ = slot.set(connection);
                }
                Err(err) => log::warn!("MPRIS unavailable: {err}"),
            }
        });
        Self { shared, connection }
    }

    /// Replaces the snapshot and tells the bus what changed.
    pub fn update(&self, snapshot: Snapshot) {
        let props = {
            let mut current = read(&self.shared);
            let props = changes(&current, &snapshot);
            *current = snapshot;
            props
        };
        if props.is_empty() {
            return;
        }
        let Some(connection) = self.connection.get().cloned() else {
            return;
        };
        runtime::runtime().spawn(async move {
            if let Err(err) = emit_changes(&connection, &props).await {
                log::debug!("MPRIS change signal failed: {err}");
            }
        });
    }

    /// Updates the position without signalling (clients extrapolate it).
    pub fn set_position(&self, position: f64) {
        let mut current = read(&self.shared);
        current.position = position;
        current.position_at = Some(Instant::now());
    }

    /// Signals a jump in position (seconds).
    pub fn seeked(&self, position: f64) {
        self.set_position(position);
        let Some(connection) = self.connection.get().cloned() else {
            return;
        };
        let micros = (position.max(0.0) * 1e6) as i64;
        runtime::runtime().spawn(async move {
            let result = async {
                let iface = connection
                    .object_server()
                    .interface::<_, Player>(PATH)
                    .await?;
                Player::seeked(iface.signal_emitter(), micros).await
            };
            if let Err(err) = result.await {
                log::debug!("MPRIS Seeked failed: {err}");
            }
        });
    }
}

async fn connect(
    shared: Shared,
    commands: UnboundedSender<Command>,
) -> zbus::Result<zbus::Connection> {
    let connection = zbus::connection::Builder::session()?
        .serve_at(
            PATH,
            Root {
                shared: shared.clone(),
                commands: commands.clone(),
            },
        )?
        .serve_at(PATH, Player { shared, commands })?
        .build()
        .await?;
    // A second instance (e.g. a screenshot run) takes a unique name, as
    // the specification allows.
    let reply = connection
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await;
    if !matches!(
        reply,
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner)
    ) {
        let name = format!("{BUS_NAME}.instance{}", std::process::id());
        connection.request_name(name).await?;
    }
    Ok(connection)
}

async fn emit_changes(connection: &zbus::Connection, props: &[Prop]) -> zbus::Result<()> {
    let server = connection.object_server();
    let player = server.interface::<_, Player>(PATH).await?;
    let root = server.interface::<_, Root>(PATH).await?;
    let emitter = player.signal_emitter();
    let (player, root) = (player.get().await, root.get().await);
    for prop in props {
        match prop {
            Prop::Status => player.playback_status_changed(emitter).await?,
            Prop::Loop => player.loop_status_changed(emitter).await?,
            Prop::Shuffle => player.shuffle_changed(emitter).await?,
            Prop::Volume => player.volume_changed(emitter).await?,
            Prop::Metadata => player.metadata_changed(emitter).await?,
            Prop::CanGoNext => player.can_go_next_changed(emitter).await?,
            Prop::CanGoPrevious => player.can_go_previous_changed(emitter).await?,
            Prop::CanPlay => {
                player.can_play_changed(emitter).await?;
                player.can_pause_changed(emitter).await?;
            }
            Prop::CanSeek => player.can_seek_changed(emitter).await?,
            Prop::CanRaise => root.can_raise_changed(emitter).await?,
            Prop::CanQuit => root.can_quit_changed(emitter).await?,
        }
    }
    Ok(())
}

fn track_path(id: u64) -> String {
    format!("{TRACK_PATH}/{id}")
}

fn metadata(track: Option<&Metadata>) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    let mut put = |key: &str, value: Value<'_>| {
        if let Ok(value) = OwnedValue::try_from(value) {
            map.insert(key.to_owned(), value);
        }
    };
    let Some(track) = track else {
        if let Ok(path) = ObjectPath::try_from(NO_TRACK) {
            put("mpris:trackid", Value::from(path));
        }
        return map;
    };
    if let Ok(path) = ObjectPath::try_from(track_path(track.track_id)) {
        put("mpris:trackid", Value::from(path));
    }
    put("mpris:length", Value::from((track.length * 1e6) as i64));
    put("xesam:title", Value::from(track.title.as_str()));
    put("xesam:artist", Value::from(track.artists.clone()));
    put("xesam:album", Value::from(track.album.as_str()));
    if let Some(first) = track.artists.first() {
        put("xesam:albumArtist", Value::from(vec![first.clone()]));
    }
    put("xesam:url", Value::from(track.url.as_str()));
    if track.track_number > 0 {
        put("xesam:trackNumber", Value::from(track.track_number as i32));
    }
    if track.disc_number > 0 {
        put("xesam:discNumber", Value::from(track.disc_number as i32));
    }
    if let Some(art) = &track.art_url {
        put("mpris:artUrl", Value::from(art.as_str()));
    }
    map
}

struct Root {
    shared: Shared,
    commands: UnboundedSender<Command>,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2", introspection_docs = false)]
impl Root {
    fn raise(&self) {
        let _ = self.commands.send(Command::Raise);
    }

    fn quit(&self) {
        let _ = self.commands.send(Command::Quit);
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        read(&self.shared).can_quit
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        read(&self.shared).can_raise
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn identity(&self) -> String {
        config::APP_NAME.to_owned()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn desktop_entry(&self) -> String {
        config::APP_ID.to_owned()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["tidal".to_owned(), "https".to_owned()]
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    shared: Shared,
    commands: UnboundedSender<Command>,
}

impl Player {
    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player", introspection_docs = false)]
impl Player {
    fn next(&self) {
        self.send(Command::Next);
    }

    fn previous(&self) {
        self.send(Command::Previous);
    }

    fn pause(&self) {
        self.send(Command::Pause);
    }

    fn play_pause(&self) {
        self.send(Command::PlayPause);
    }

    fn stop(&self) {
        self.send(Command::Stop);
    }

    fn play(&self) {
        self.send(Command::Play);
    }

    /// Microseconds, relative.
    fn seek(&self, offset: i64) {
        self.send(Command::Seek(offset as f64 / 1e6));
    }

    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let id = track_id
            .as_str()
            .strip_prefix(TRACK_PATH)
            .and_then(|rest| rest.strip_prefix('/'))
            .and_then(|id| id.parse().ok());
        if let Some(track_id) = id
            && position >= 0
        {
            self.send(Command::SetPosition {
                track_id,
                position: position as f64 / 1e6,
            });
        }
    }

    fn open_uri(&self, uri: &str) -> zbus::fdo::Result<()> {
        if crate::tidal::pages::Target::from_url(uri).is_none() {
            return Err(zbus::fdo::Error::InvalidArgs("not a TIDAL link".to_owned()));
        }
        self.send(Command::OpenUri(uri.to_owned()));
        Ok(())
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        match read(&self.shared).status {
            Status::Playing => "Playing",
            Status::Paused => "Paused",
            Status::Stopped => "Stopped",
        }
        .to_owned()
    }

    #[zbus(property)]
    fn loop_status(&self) -> String {
        match read(&self.shared).repeat {
            Repeat::Off => "None",
            Repeat::One => "Track",
            Repeat::All => "Playlist",
        }
        .to_owned()
    }

    #[zbus(property)]
    fn set_loop_status(&mut self, value: String) -> zbus::fdo::Result<()> {
        let repeat = match value.as_str() {
            "None" => Repeat::Off,
            "Track" => Repeat::One,
            "Playlist" => Repeat::All,
            _ => {
                return Err(zbus::fdo::Error::InvalidArgs(format!(
                    "unknown loop status {value}"
                )));
            }
        };
        read(&self.shared).repeat = repeat;
        self.send(Command::Repeat(repeat));
        Ok(())
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    /// Only 1.0 is supported; the spec says a rate of 0 pauses.
    #[zbus(property)]
    fn set_rate(&mut self, rate: f64) {
        if rate == 0.0 {
            self.send(Command::Pause);
        }
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        read(&self.shared).shuffle
    }

    #[zbus(property)]
    fn set_shuffle(&mut self, shuffle: bool) {
        read(&self.shared).shuffle = shuffle;
        self.send(Command::Shuffle(shuffle));
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata(read(&self.shared).track.as_ref())
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        read(&self.shared).volume
    }

    #[zbus(property)]
    fn set_volume(&mut self, volume: f64) {
        let volume = volume.clamp(0.0, 1.0);
        read(&self.shared).volume = volume;
        self.send(Command::Volume(volume));
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        read(&self.shared).position_us()
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        read(&self.shared).can_go_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        read(&self.shared).can_go_previous
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        read(&self.shared).can_play
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        read(&self.shared).can_play
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        read(&self.shared).can_seek
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_map() {
        let track = Metadata {
            track_id: 529955457,
            title: "Song".into(),
            artists: vec!["A".into(), "B".into()],
            album: "Album".into(),
            length: 200.5,
            art_url: Some("https://resources.tidal.com/images/a/b/640x640.jpg".into()),
            url: "https://tidal.com/browse/track/529955457".into(),
            track_number: 3,
            disc_number: 1,
        };
        let map = metadata(Some(&track));
        let path: ObjectPath<'_> = map["mpris:trackid"].clone().try_into().unwrap();
        assert_eq!(
            path.as_str(),
            "/io/github/oxakromax/SwellPlayer/track/529955457"
        );
        assert_eq!(
            i64::try_from(map["mpris:length"].clone()).unwrap(),
            200_500_000
        );
        let artists: Vec<String> = map["xesam:artist"].clone().try_into().unwrap();
        assert_eq!(artists, ["A", "B"]);
        let empty = metadata(None);
        let path: ObjectPath<'_> = empty["mpris:trackid"].clone().try_into().unwrap();
        assert_eq!(path.as_str(), NO_TRACK);
    }

    #[test]
    fn position_extrapolates_while_playing() {
        let snapshot = Snapshot {
            status: Status::Paused,
            position: 10.0,
            position_at: Some(Instant::now() - std::time::Duration::from_secs(5)),
            ..Snapshot::default()
        };
        assert_eq!(snapshot.position_us(), 10_000_000);
        let playing = Snapshot {
            status: Status::Playing,
            ..snapshot
        };
        assert!(playing.position_us() >= 15_000_000);
    }

    #[test]
    fn detects_changes() {
        let old = Snapshot::default();
        let new = Snapshot {
            status: Status::Playing,
            volume: 0.5,
            ..Snapshot::default()
        };
        assert_eq!(changes(&old, &new), [Prop::Status, Prop::Volume]);
        assert!(changes(&new, &new.clone()).is_empty());
    }
}
