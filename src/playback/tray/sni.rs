//! The tray icon on Linux and the BSDs: a StatusNotifierItem
//! (`org.kde.StatusNotifierItem`) with a `com.canonical.dbusmenu` menu, on
//! a session bus connection of its own, so closing it removes the icon.
//! KDE Plasma, the GNOME Shell AppIndicator extension, waybar and others
//! show these.
//!
//! The item is announced to the `org.kde.StatusNotifierWatcher` under the
//! connection's unique name, which every watcher accepts (the item then
//! lives at the conventional `/StatusNotifierItem`). A well-known
//! `org.kde.StatusNotifierItem-<pid>-<n>` name would need an
//! `--own-name` wildcard in Flatpak for nothing. The watcher is followed
//! on the bus: when it starts later, or restarts with the panel, the item
//! is announced again.
//!
//! As with MPRIS, the D-Bus side runs on the I/O runtime and answers from
//! a [`TrayState`] the main thread keeps current; clicks go back to the
//! main thread as [`Command`]s. Signals for changes go through the
//! service's own task, in order.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anyhow::Result;
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::AbortHandle;
use zbus::object_server::SignalEmitter;
use zbus::{ObjectPath, OwnedVariant, Type, Variant};

use super::TrayState;
use crate::config;
use crate::playback::controls::Command;
use crate::playback::rundir;
use crate::runtime;

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";

/// The icon, embedded for [`write_icon_theme`].
const ICON: &[u8] =
    include_bytes!("../../../data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg");

/// What the bus reads, kept current by the main thread.
struct Model {
    state: TrayState,
    /// The menu's layout revision, bumped on every change.
    revision: u32,
}

type Shared = Arc<Mutex<Model>>;

fn read(shared: &Shared) -> MutexGuard<'_, Model> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A change to signal.
#[derive(Debug)]
enum Change {
    ToolTip,
    Menu {
        revision: u32,
        items: Vec<(i32, Properties)>,
    },
}

/// The icon while shown. Dropping it closes its connection, which takes
/// the icon off the panel.
pub struct Backend {
    shared: Shared,
    changes: UnboundedSender<Change>,
    task: AbortHandle,
}

impl Backend {
    /// Connects and announces the icon in the background; clicks arrive on
    /// `commands`. Without a tray yet, it waits for one.
    pub fn new(commands: UnboundedSender<Command>, state: &TrayState) -> Result<Self> {
        let shared = Arc::new(Mutex::new(Model {
            state: state.clone(),
            revision: 1,
        }));
        let (changes, receiver) = mpsc::unbounded_channel();
        let service = Service {
            shared: shared.clone(),
            commands,
            text_direction: text_direction(),
        };
        let task = runtime::runtime().spawn(async move {
            if let Err(err) = service.run(receiver).await {
                log::warn!("no tray icon: {err}");
            }
        });
        Ok(Self {
            shared,
            changes,
            task: task.abort_handle(),
        })
    }

    /// Takes a new state and signals what it changes.
    pub fn update(&self, state: &TrayState) {
        let mut model = read(&self.shared);
        if model.state == *state {
            return;
        }
        let tooltip_changed = tool_tip(&model.state) != tool_tip(state);
        let items = changed_items(&model.state, state);
        model.state = state.clone();
        if tooltip_changed {
            let _ = self.changes.send(Change::ToolTip);
        }
        if !items.is_empty() {
            model.revision = model.revision.wrapping_add(1);
            let revision = model.revision;
            let _ = self.changes.send(Change::Menu { revision, items });
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        // The task owns the connection: ending it closes it.
        self.task.abort();
    }
}

/// The menu's text direction, from GTK's (left to right outside the app,
/// as in the tests).
fn text_direction() -> &'static str {
    if gtk::is_initialized_main_thread()
        && gtk::Widget::default_direction() == gtk::TextDirection::Rtl
    {
        "rtl"
    } else {
        "ltr"
    }
}

struct Service {
    shared: Shared,
    commands: UnboundedSender<Command>,
    text_direction: &'static str,
}

impl Service {
    async fn run(self, mut changes: UnboundedReceiver<Change>) -> zbus::Result<()> {
        let icon_theme_path = icon_theme_path().await;
        let connection = zbus::connection::Builder::session()
            .serve_at(
                ITEM_PATH,
                StatusItem {
                    shared: self.shared.clone(),
                    commands: self.commands.clone(),
                    icon_theme_path: icon_theme_path.clone().unwrap_or_default(),
                },
            )
            .serve_at(
                MENU_PATH,
                DbusMenu {
                    shared: self.shared,
                    commands: self.commands,
                    text_direction: self.text_direction,
                    icon_theme_path: icon_theme_path.into_iter().collect(),
                },
            )
            .build()
            .await?;
        // Followed before the first announcement, so a watcher starting in
        // between is not missed.
        let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
        let mut watchers = dbus
            .receive_name_owner_changed_with_args(&[(0, WATCHER)])
            .await?;
        register(&connection).await;
        loop {
            tokio::select! {
                Some(signal) = watchers.next() => {
                    match signal.args() {
                        Ok(args) if args.new_owner().is_some() => register(&connection).await,
                        _ => log::debug!("the StatusNotifierWatcher left the bus"),
                    }
                }
                change = changes.recv() => {
                    let Some(change) = change else {
                        break;
                    };
                    if let Err(err) = emit(&connection, change).await {
                        log::debug!("tray change signal failed: {err}");
                    }
                }
            }
        }
        Ok(())
    }
}

/// Announces the item to the watcher, if there is one.
async fn register(connection: &zbus::Connection) {
    let Some(name) = connection.unique_name() else {
        return;
    };
    let result = connection
        .call_method(
            Some(WATCHER),
            WATCHER_PATH,
            Some(WATCHER),
            "RegisterStatusNotifierItem",
            &(name.as_str(),),
        )
        .await;
    match result {
        Ok(_) => log::debug!("tray icon registered as {name}"),
        Err(zbus::Error::FDO(error))
            if matches!(
                *error,
                zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_)
            ) =>
        {
            log::info!("no tray (StatusNotifierWatcher) yet; the icon appears when one starts");
        }
        Err(err) => log::warn!("could not register the tray icon: {err}"),
    }
}

async fn emit(connection: &zbus::Connection, change: Change) -> zbus::Result<()> {
    let server = connection.object_server();
    match change {
        Change::ToolTip => {
            let item = server.interface::<_, StatusItem>(ITEM_PATH).await?;
            StatusItem::new_tool_tip(item.signal_emitter()).await
        }
        Change::Menu { revision, items } => {
            let menu = server.interface::<_, DbusMenu>(MENU_PATH).await?;
            let emitter = menu.signal_emitter();
            DbusMenu::items_properties_updated(emitter, items, Vec::new()).await?;
            DbusMenu::layout_updated(emitter, revision, ROOT).await
        }
    }
}

/// `IconThemePath`: a directory holding the icon as
/// `hicolor/scalable/apps/<app id>.svg`, written from the embedded copy.
///
/// `IconName` alone needs the icon installed in the host's theme, which it
/// is not when Swell runs from the build tree or an unpacked tarball; the
/// copy covers every case with one code path (installed, it is the same
/// picture). It goes to the shared `icons/` of the runtime directory
/// rather than this process's own one, which is emptied on sign-out while
/// the icon stays. KDE wants the path to end in `<app>/icons`, and takes
/// the `hicolor/` layout, as GTK-based trays do. Flatpak needs none: the
/// sandbox's runtime directory is not the host's, and the installed icon
/// is exported to the host.
async fn icon_theme_path() -> Option<String> {
    if Path::new("/.flatpak-info").exists() {
        return None;
    }
    match tokio::task::spawn_blocking(write_icon_theme).await {
        Ok(Ok(dir)) => Some(dir.to_string_lossy().into_owned()),
        Ok(Err(err)) => {
            log::debug!("no icon theme for the tray icon: {err}");
            None
        }
        Err(err) => {
            log::debug!("no icon theme for the tray icon: {err}");
            None
        }
    }
}

/// Writes the icon (if not there yet) and returns its theme directory.
/// Blocking.
fn write_icon_theme() -> std::io::Result<PathBuf> {
    let own = rundir::create()?;
    let Some(base) = own.parent() else {
        return Err(std::io::Error::other("no runtime directory"));
    };
    let theme = base.join("icons");
    let apps = theme.join("hicolor").join("scalable").join("apps");
    std::fs::create_dir_all(&apps)?;
    let path = apps.join(format!("{}.svg", config::APP_ID));
    if std::fs::read(&path).is_ok_and(|bytes| bytes == ICON) {
        return Ok(theme);
    }
    // Written aside and renamed, as another instance may be reading it.
    let partial = rundir::temp_path(&path);
    let written = std::fs::write(&partial, ICON).and_then(|()| std::fs::rename(&partial, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written.map(|()| theme)
}

/// An icon as pixels: width, height and ARGB32 bytes.
type Pixmap = (i32, i32, Vec<u8>);

/// `ToolTip`: icon name, icon pixmaps, title and description (basic
/// markup, so the text is escaped).
type ToolTip = (String, Vec<Pixmap>, String, String);

fn tool_tip(state: &TrayState) -> ToolTip {
    (
        config::APP_ID.to_owned(),
        Vec::new(),
        escape(&state.title),
        state.track.as_deref().map(escape).unwrap_or_default(),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

struct StatusItem {
    shared: Shared,
    commands: UnboundedSender<Command>,
    icon_theme_path: String,
}

/// Properties are read when asked and announced with the `New…` signals,
/// as the specification has it, not with `PropertiesChanged`.
#[zbus::interface(name = "org.kde.StatusNotifierItem", introspection_docs = false)]
impl StatusItem {
    /// A click: brings the window back.
    fn activate(&self, _x: i32, _y: i32) {
        let _ = self.commands.send(Command::Raise);
    }

    /// A middle click: play or pause.
    fn secondary_activate(&self, _x: i32, _y: i32) {
        let _ = self.commands.send(Command::PlayPause);
    }

    /// The host shows the menu itself (`Menu`).
    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: String) {}

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property(emits_changed_signal = "const"))]
    fn category(&self) -> String {
        "ApplicationStatus".to_owned()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn id(&self) -> String {
        config::APP_ID.to_owned()
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn title(&self) -> String {
        read(&self.shared).state.title.clone()
    }

    /// Always shown: it is the way back to a closed window.
    #[zbus(property(emits_changed_signal = "const"))]
    fn status(&self) -> String {
        "Active".to_owned()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn window_id(&self) -> i32 {
        0
    }

    /// Plasma shows `<IconName>-symbolic` instead when the theme has it,
    /// as an installed Swell does: that icon must take the panel's colours
    /// (a test checks it).
    #[zbus(property(emits_changed_signal = "const"))]
    fn icon_name(&self) -> String {
        config::APP_ID.to_owned()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn icon_theme_path(&self) -> String {
        self.icon_theme_path.clone()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn overlay_icon_name(&self) -> String {
        String::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn overlay_icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn attention_icon_name(&self) -> String {
        String::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn attention_icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn attention_movie_name(&self) -> String {
        String::new()
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn tool_tip(&self) -> ToolTip {
        tool_tip(&read(&self.shared).state)
    }

    /// A click activates; the menu is for the secondary button.
    #[zbus(property(emits_changed_signal = "const"))]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn menu(&self) -> ObjectPath<'static> {
        ObjectPath::from_static_str_unchecked(MENU_PATH)
    }
}

/// A menu item's properties, as dbusmenu has them.
type Properties = HashMap<String, OwnedVariant>;

/// The root item: it holds the menu.
const ROOT: i32 = 0;

/// The menu's items, in order. Item `n` (from 1) is `ENTRIES[n - 1]`.
const ENTRIES: [Entry; 7] = [
    Entry::Show,
    Entry::Separator,
    Entry::PlayPause,
    Entry::Previous,
    Entry::Next,
    Entry::Separator,
    Entry::Quit,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Show,
    Separator,
    PlayPause,
    Previous,
    Next,
    Quit,
}

impl Entry {
    fn from_id(id: i32) -> Option<Self> {
        let index = usize::try_from(id).ok()?.checked_sub(1)?;
        ENTRIES.get(index).copied()
    }

    /// The label, whether it is enabled and what it does; nothing for a
    /// separator.
    fn action(self, state: &TrayState) -> Option<(&str, bool, Command)> {
        let labels = &state.labels;
        match self {
            Self::Show => Some((&labels.show, true, Command::Raise)),
            Self::Separator => None,
            Self::PlayPause => {
                let (label, command) = state.play_pause();
                Some((label, state.can_play, command))
            }
            Self::Previous => Some((&labels.previous, state.can_go_previous, Command::Previous)),
            Self::Next => Some((&labels.next, state.can_go_next, Command::Next)),
            Self::Quit => Some((&labels.quit, true, Command::Quit)),
        }
    }
}

fn item_ids() -> impl Iterator<Item = i32> {
    (1..).take(ENTRIES.len())
}

fn properties<'a>(entries: impl IntoIterator<Item = (&'a str, Variant<'a>)>) -> Properties {
    entries
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), OwnedVariant::try_from(value).ok()?)))
        .collect()
}

/// Item `id`'s properties, all of them (so a change never removes one).
fn item_properties(state: &TrayState, id: i32) -> Option<Properties> {
    if id == ROOT {
        return Some(properties([("children-display", Variant::from("submenu"))]));
    }
    Some(match Entry::from_id(id)?.action(state) {
        None => properties([
            ("type", Variant::from("separator")),
            ("visible", Variant::from(true)),
        ]),
        Some((label, enabled, _)) => properties([
            // An underscore marks a mnemonic; a literal one is doubled.
            ("label", Variant::from(label.replace('_', "__"))),
            ("enabled", Variant::from(enabled)),
            ("visible", Variant::from(true)),
        ]),
    })
}

/// What clicking item `id` does, if it is enabled.
fn item_command(state: &TrayState, id: i32) -> Option<Command> {
    let (_, enabled, command) = Entry::from_id(id)?.action(state)?;
    enabled.then_some(command)
}

/// The items whose properties differ between `old` and `new`, with their
/// new properties.
fn changed_items(old: &TrayState, new: &TrayState) -> Vec<(i32, Properties)> {
    item_ids()
        .filter_map(|id| {
            let properties = item_properties(new, id)?;
            (item_properties(old, id).as_ref() != Some(&properties)).then_some((id, properties))
        })
        .collect()
}

/// Only the properties asked for (all of them for none).
fn filter(mut properties: Properties, names: &[String]) -> Properties {
    if !names.is_empty() {
        properties.retain(|key, _| names.contains(key));
    }
    properties
}

/// A menu item and, as deep as asked, its children.
#[derive(Debug, PartialEq)]
struct Node {
    id: i32,
    properties: Properties,
    children: Vec<Node>,
}

/// The layout under `parent`, `depth` levels down (-1: all of it).
fn layout(state: &TrayState, parent: i32, depth: i32, names: &[String]) -> Option<Node> {
    let properties = filter(item_properties(state, parent)?, names);
    let children = if parent == ROOT && depth != 0 {
        item_ids()
            .filter_map(|id| layout(state, id, depth - 1, names))
            .collect()
    } else {
        Vec::new()
    };
    Some(Node {
        id: parent,
        properties,
        children,
    })
}

/// A [`Node`] on the wire, `(ia{sv}av)`: the children are variants of the
/// same.
#[derive(Debug, Serialize, Type, Variant)]
struct Layout {
    id: i32,
    properties: Properties,
    children: Vec<OwnedVariant>,
}

impl From<Node> for Layout {
    fn from(node: Node) -> Self {
        Self {
            id: node.id,
            properties: node.properties,
            children: node
                .children
                .into_iter()
                .filter_map(|child| OwnedVariant::try_from(Variant::from(Layout::from(child))).ok())
                .collect(),
        }
    }
}

struct DbusMenu {
    shared: Shared,
    commands: UnboundedSender<Command>,
    text_direction: &'static str,
    icon_theme_path: Vec<String>,
}

impl DbusMenu {
    /// Runs the `clicked` event of item `id`; false for an unknown item.
    fn handle(&self, id: i32, event: &str) -> bool {
        let state = &read(&self.shared).state;
        if id != ROOT && Entry::from_id(id).is_none() {
            return false;
        }
        if event == "clicked"
            && let Some(command) = item_command(state, id)
        {
            let _ = self.commands.send(command);
        }
        true
    }
}

fn unknown(id: i32) -> zbus::fdo::Error {
    zbus::fdo::Error::InvalidArgs(format!("no menu item {id}"))
}

#[zbus::interface(name = "com.canonical.dbusmenu", introspection_docs = false)]
impl DbusMenu {
    #[zbus(out_args("revision", "layout"))]
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        let model = read(&self.shared);
        let node = layout(&model.state, parent_id, recursion_depth, &property_names)
            .ok_or_else(|| unknown(parent_id))?;
        Ok((model.revision, node.into()))
    }

    /// All items for no `ids`.
    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, Properties)> {
        let state = &read(&self.shared).state;
        let ids = if ids.is_empty() {
            std::iter::once(ROOT).chain(item_ids()).collect()
        } else {
            ids
        };
        ids.into_iter()
            .filter_map(|id| {
                let properties = item_properties(state, id)?;
                Some((id, filter(properties, &property_names)))
            })
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedVariant> {
        let state = &read(&self.shared).state;
        item_properties(state, id)
            .ok_or_else(|| unknown(id))?
            .remove(&name)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no property {name}")))
    }

    fn event(
        &self,
        id: i32,
        event_id: String,
        _data: OwnedVariant,
        _timestamp: u32,
    ) -> zbus::fdo::Result<()> {
        if self.handle(id, &event_id) {
            Ok(())
        } else {
            Err(unknown(id))
        }
    }

    /// Returns the unknown ids; fails when all of them are.
    #[zbus(out_args("idErrors"))]
    fn event_group(
        &self,
        events: Vec<(i32, String, OwnedVariant, u32)>,
    ) -> zbus::fdo::Result<Vec<i32>> {
        let count = events.len();
        let errors: Vec<i32> = events
            .into_iter()
            .filter(|(id, event, _, _)| !self.handle(*id, event))
            .map(|(id, ..)| id)
            .collect();
        if count > 0 && errors.len() == count {
            return Err(zbus::fdo::Error::InvalidArgs(
                "no such menu items".to_owned(),
            ));
        }
        Ok(errors)
    }

    /// The menu is always current: no update is needed.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    #[zbus(out_args("updatesNeeded", "idErrors"))]
    fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        let errors = ids
            .into_iter()
            .filter(|&id| id != ROOT && Entry::from_id(id).is_none())
            .collect();
        (Vec::new(), errors)
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn items_properties_updated(
        emitter: &SignalEmitter<'_>,
        updated_props: Vec<(i32, Properties)>,
        removed_props: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;

    #[zbus(property(emits_changed_signal = "const"))]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn text_direction(&self) -> String {
        self.text_direction.to_owned()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn status(&self) -> String {
        "normal".to_owned()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn icon_theme_path(&self) -> Vec<String> {
        self.icon_theme_path.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::playback::controls::Status;
    use crate::playback::tray::Labels;

    fn state() -> TrayState {
        TrayState {
            title: "Swell".to_owned(),
            track: None,
            status: Status::Stopped,
            can_play: false,
            can_go_next: false,
            can_go_previous: false,
            labels: Labels {
                show: "Show Window".to_owned(),
                play: "Play".to_owned(),
                pause: "Pause".to_owned(),
                next: "Next Track".to_owned(),
                previous: "Previous Track".to_owned(),
                quit: "Quit".to_owned(),
            },
        }
    }

    fn playing() -> TrayState {
        TrayState {
            track: Some("Song — Artist".to_owned()),
            status: Status::Playing,
            can_play: true,
            can_go_next: true,
            can_go_previous: true,
            ..state()
        }
    }

    fn string(properties: &Properties, key: &str) -> Option<String> {
        properties
            .get(key)
            .map(|value| String::try_from(value.clone()).unwrap())
    }

    fn flag(properties: &Properties, key: &str) -> Option<bool> {
        properties
            .get(key)
            .map(|value| bool::try_from(value.clone()).unwrap())
    }

    /// (label or "-" for a separator, enabled) of each item under the root.
    fn items(node: &Node) -> Vec<(String, bool)> {
        node.children
            .iter()
            .map(|child| match string(&child.properties, "type").as_deref() {
                Some("separator") => ("-".to_owned(), true),
                _ => (
                    string(&child.properties, "label").unwrap(),
                    flag(&child.properties, "enabled").unwrap(),
                ),
            })
            .collect()
    }

    #[test]
    fn menu_layout() {
        let root = layout(&state(), ROOT, -1, &[]).unwrap();
        assert_eq!(root.id, ROOT);
        assert_eq!(
            string(&root.properties, "children-display").as_deref(),
            Some("submenu")
        );
        let ids: Vec<i32> = root.children.iter().map(|child| child.id).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7]);
        let expected = [
            ("Show Window", true),
            ("-", true),
            ("Play", false),
            ("Previous Track", false),
            ("Next Track", false),
            ("-", true),
            ("Quit", true),
        ];
        let expected: Vec<(String, bool)> = expected
            .iter()
            .map(|&(label, enabled)| (label.to_owned(), enabled))
            .collect();
        assert_eq!(items(&root), expected);
        assert!(root.children.iter().all(|child| child.children.is_empty()));

        let root = layout(&playing(), ROOT, 1, &[]).unwrap();
        let items = items(&root);
        assert_eq!(items[2], ("Pause".to_owned(), true));
        assert_eq!(items[3], ("Previous Track".to_owned(), true));
        assert_eq!(items[4], ("Next Track".to_owned(), true));
    }

    #[test]
    fn layout_depth_and_property_names() {
        let root = layout(&state(), ROOT, 0, &[]).unwrap();
        assert!(root.children.is_empty());
        let item = layout(&state(), 7, -1, &[]).unwrap();
        assert_eq!(item.id, 7);
        assert_eq!(string(&item.properties, "label").as_deref(), Some("Quit"));
        assert!(layout(&state(), 8, -1, &[]).is_none());
        assert!(layout(&state(), -1, -1, &[]).is_none());

        let root = layout(&state(), ROOT, -1, &["label".to_owned()]).unwrap();
        assert!(root.properties.is_empty());
        let show = &root.children[0].properties;
        assert_eq!(show.keys().collect::<Vec<_>>(), ["label"]);
        assert!(root.children[1].properties.is_empty());
    }

    #[test]
    fn items_send_commands() {
        let stopped = state();
        assert_eq!(item_command(&stopped, 1), Some(Command::Raise));
        assert_eq!(item_command(&stopped, 2), None);
        // Disabled.
        assert_eq!(item_command(&stopped, 3), None);
        assert_eq!(item_command(&stopped, 4), None);
        assert_eq!(item_command(&stopped, 5), None);
        assert_eq!(item_command(&stopped, 7), Some(Command::Quit));
        assert_eq!(item_command(&stopped, ROOT), None);
        assert_eq!(item_command(&stopped, 8), None);

        let mut state = playing();
        assert_eq!(item_command(&state, 3), Some(Command::Pause));
        assert_eq!(item_command(&state, 4), Some(Command::Previous));
        assert_eq!(item_command(&state, 5), Some(Command::Next));
        state.status = Status::Paused;
        assert_eq!(item_command(&state, 3), Some(Command::Play));
    }

    #[test]
    fn changes_list_only_the_changed_items() {
        assert!(changed_items(&state(), &state()).is_empty());
        let ids: Vec<i32> = changed_items(&state(), &playing())
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(ids, [3, 4, 5]);
        // A new track alone changes only the tooltip.
        let other = TrayState {
            track: Some("Other".to_owned()),
            ..playing()
        };
        assert!(changed_items(&playing(), &other).is_empty());
        assert_ne!(tool_tip(&playing()), tool_tip(&other));
        // Every item always has all its properties, so none is removed.
        let (_, properties) = &changed_items(&state(), &playing())[0];
        assert_eq!(flag(properties, "enabled"), Some(true));
        assert_eq!(flag(properties, "visible"), Some(true));
    }

    #[test]
    fn labels_and_tooltip_are_escaped() {
        let mut state = state();
        state.labels.show = "Show_Window".to_owned();
        let properties = item_properties(&state, 1).unwrap();
        assert_eq!(
            string(&properties, "label").as_deref(),
            Some("Show__Window")
        );

        state.track = Some("Rock & Roll <Live> — A".to_owned());
        let (icon, pixmaps, title, description) = tool_tip(&state);
        assert_eq!(icon, config::APP_ID);
        assert!(pixmaps.is_empty());
        assert_eq!(title, "Swell");
        assert_eq!(description, "Rock &amp; Roll &lt;Live&gt; — A");
        assert_eq!(tool_tip(&TrayState::default()).3, "");
    }

    /// Plasma's tray recolours a symbolic icon only through its
    /// `current-color-scheme` stylesheet (KIconLoader does not override
    /// fills, as GTK does): a fixed dark fill is all but invisible on a
    /// dark panel, and shows only brightened under the pointer.
    #[test]
    fn symbolic_icon_follows_the_panel_colours() {
        let svg = include_str!(
            "../../../data/icons/symbolic/apps/io.github.oxakromax.SwellPlayer-symbolic.svg"
        );
        assert!(svg.contains(r#"<style id="current-color-scheme">.ColorScheme-Text{"#));
        let shapes = ["<path", "<circle", "<rect", "<ellipse", "<polygon"]
            .iter()
            .map(|tag| svg.matches(tag).count())
            .sum::<usize>();
        assert!(shapes > 0);
        let painted = svg
            .matches(r#"class="ColorScheme-Text" fill="currentColor""#)
            .count();
        assert_eq!(painted, shapes, "every shape in the scheme's colour");
        assert!(!svg.contains("fill=\"#"), "no fixed colour");
    }

    #[test]
    fn layout_wire_format() {
        let layout = Layout::from(layout(&state(), ROOT, -1, &[]).unwrap());
        assert_eq!(Layout::SIGNATURE.to_string(), "(ia{sv}av)");
        assert_eq!(layout.children.len(), ENTRIES.len());
        let child = layout.children[0].clone();
        let (id, properties, children): (i32, Properties, Vec<OwnedVariant>) =
            child.try_into().unwrap();
        assert_eq!(id, 1);
        assert_eq!(string(&properties, "label").as_deref(), Some("Show Window"));
        assert!(children.is_empty());
    }

    /// A tray, as the panel has it: it records which items register.
    struct Watcher {
        registered: UnboundedSender<(String, String)>,
    }

    #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
    impl Watcher {
        fn register_status_notifier_item(
            &self,
            service: String,
            #[zbus(header)] header: zbus::message::Header<'_>,
        ) {
            let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
            let _ = self.registered.send((sender, service));
        }
    }

    async fn start_watcher(
        registered: UnboundedSender<(String, String)>,
    ) -> zbus::Result<zbus::Connection> {
        zbus::connection::Builder::session()
            .name(WATCHER)
            .serve_at(WATCHER_PATH, Watcher { registered })
            .build()
            .await
    }

    /// Calls on the item from another connection.
    struct Client {
        connection: zbus::Connection,
        item: String,
    }

    impl Client {
        async fn call<B>(
            &self,
            path: &str,
            interface: &str,
            method: &str,
            body: &B,
        ) -> zbus::Result<zbus::Message>
        where
            B: Serialize + zbus::DynamicType,
        {
            let item = self.item.as_str();
            self.connection
                .call_method(Some(item), path, Some(interface), method, body)
                .await
        }

        async fn menu<B>(&self, method: &str, body: &B) -> zbus::Result<zbus::Message>
        where
            B: Serialize + zbus::DynamicType,
        {
            self.call(MENU_PATH, "com.canonical.dbusmenu", method, body)
                .await
        }

        async fn item<B>(&self, method: &str, body: &B) -> zbus::Result<zbus::Message>
        where
            B: Serialize + zbus::DynamicType,
        {
            self.call(ITEM_PATH, "org.kde.StatusNotifierItem", method, body)
                .await
        }

        async fn click(&self, id: i32) -> zbus::Result<zbus::Message> {
            self.menu("Event", &(id, "clicked", Variant::from(0i32), 0u32))
                .await
        }
    }

    async fn within<T>(future: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), future)
            .await
            .expect("timed out")
    }

    /// The whole item on a real bus: run it in a private session bus,
    /// `dbus-run-session -- cargo test -- --ignored tray`.
    #[test]
    #[ignore = "needs a session bus of its own (dbus-run-session)"]
    fn tray_on_the_bus() {
        runtime::runtime().block_on(on_the_bus());
    }

    async fn on_the_bus() {
        let (commands, mut received) = mpsc::unbounded_channel();
        let backend = Backend::new(commands, &state()).unwrap();
        let client = zbus::Connection::session().await.unwrap();

        // No tray yet: it waits, and registers once one starts.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (registrations, mut registered) = mpsc::unbounded_channel();
        let watcher = start_watcher(registrations.clone()).await.unwrap();
        let (sender, service) = within(registered.recv()).await.unwrap();
        assert_eq!(sender, service, "registered under its unique name");
        assert!(service.starts_with(':'));
        let item = service;

        // The item's properties.
        let properties = zbus::fdo::PropertiesProxy::builder(&client)
            .destination(item.as_str())
            .path(ITEM_PATH)
            .build()
            .await
            .unwrap();
        let interface =
            zbus::names::InterfaceName::from_static_str_unchecked("org.kde.StatusNotifierItem");
        let all = properties.get_all(interface.clone()).await.unwrap();
        let text = |key: &str| String::try_from(all[key].clone()).unwrap();
        assert_eq!(text("Category"), "ApplicationStatus");
        assert_eq!(text("Id"), config::APP_ID);
        assert_eq!(text("IconName"), config::APP_ID);
        assert_eq!(text("Status"), "Active");
        assert_eq!(text("Title"), "Swell");
        assert!(!bool::try_from(all["ItemIsMenu"].clone()).unwrap());
        let menu: ObjectPath<'_> = all["Menu"].clone().try_into().unwrap();
        assert_eq!(menu.as_str(), MENU_PATH);
        let theme = text("IconThemePath");
        let icon = Path::new(&theme)
            .join("hicolor/scalable/apps")
            .join(format!("{}.svg", config::APP_ID));
        assert_eq!(std::fs::read(&icon).unwrap(), ICON, "{}", icon.display());
        let tip: ToolTip = all["ToolTip"].clone().try_into().unwrap();
        assert_eq!(tip, tool_tip(&state()));

        // The menu.
        let call = Client {
            connection: client.clone(),
            item: item.clone(),
        };
        let reply = call
            .menu("GetLayout", &(ROOT, -1, Vec::<String>::new()))
            .await
            .unwrap();
        let (revision, (id, _, children)): (u32, (i32, Properties, Vec<OwnedVariant>)) =
            reply.body().deserialize().unwrap();
        assert_eq!((revision, id, children.len()), (1, ROOT, 7));
        let menu_properties = zbus::fdo::PropertiesProxy::builder(&client)
            .destination(item.as_str())
            .path(MENU_PATH)
            .build()
            .await
            .unwrap();
        let all = menu_properties
            .get_all(zbus::names::InterfaceName::from_static_str_unchecked(
                "com.canonical.dbusmenu",
            ))
            .await
            .unwrap();
        assert_eq!(u32::try_from(all["Version"].clone()).unwrap(), 3);
        assert_eq!(String::try_from(all["Status"].clone()).unwrap(), "normal");
        assert_eq!(
            String::try_from(all["TextDirection"].clone()).unwrap(),
            "ltr"
        );

        // Clicks.
        let click = |id: i32| call.click(id);
        click(1).await.unwrap();
        assert_eq!(received.try_recv().ok(), Some(Command::Raise));
        click(3).await.unwrap(); // Disabled: nothing.
        assert!(received.try_recv().is_err());
        click(7).await.unwrap();
        assert_eq!(received.try_recv().ok(), Some(Command::Quit));
        assert!(click(42).await.is_err());
        call.item("Activate", &(10, 10)).await.unwrap();
        assert_eq!(received.try_recv().ok(), Some(Command::Raise));
        call.item("SecondaryActivate", &(10, 10)).await.unwrap();
        assert_eq!(received.try_recv().ok(), Some(Command::PlayPause));

        // Changes are signalled, and read back.
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(item.as_str())
            .build()
            .unwrap();
        let mut signals = zbus::MessageStream::for_match_rule(rule, &client, None)
            .await
            .unwrap();
        backend.update(&playing());
        let mut names = Vec::new();
        while names.len() < 3 {
            let message = within(signals.next()).await.unwrap().unwrap();
            let header = message.header();
            names.push(header.member().unwrap().to_string());
        }
        names.sort();
        assert_eq!(
            names,
            ["ItemsPropertiesUpdated", "LayoutUpdated", "NewToolTip"]
        );
        let reply = call
            .menu("GetLayout", &(ROOT, -1, Vec::<String>::new()))
            .await
            .unwrap();
        let (revision, (_, _, children)): (u32, (i32, Properties, Vec<OwnedVariant>)) =
            reply.body().deserialize().unwrap();
        assert_eq!(revision, 2);
        let (_, play_pause, _): (i32, Properties, Vec<OwnedVariant>) =
            children[2].clone().try_into().unwrap();
        assert_eq!(string(&play_pause, "label").as_deref(), Some("Pause"));
        click(3).await.unwrap();
        assert_eq!(received.try_recv().ok(), Some(Command::Pause));
        let tip: ToolTip = properties
            .get(interface, "ToolTip")
            .await
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(tip.3, "Song — Artist");

        // A restarted tray gets the item again.
        watcher.close().await.unwrap();
        let _watcher = start_watcher(registrations).await.unwrap();
        let (_, service) = within(registered.recv()).await.unwrap();
        assert_eq!(service, item);

        // Dropped, the item leaves the bus.
        let dbus = zbus::fdo::DBusProxy::new(&client).await.unwrap();
        let name = zbus::names::BusName::try_from(item.as_str()).unwrap();
        assert!(dbus.name_has_owner(name.clone()).await.unwrap());
        drop(backend);
        within(async {
            while dbus.name_has_owner(name.clone()).await.unwrap() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
    }
}
