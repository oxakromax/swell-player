//! The sidebar (docs/design.md §2.3) and the Collection index (§3.6).
//!
//! Both are a `gtk::ListView.navigation-sidebar` over the same stores and
//! row factory: navigation (Home, Explore, Search; sidebar only),
//! Collection sub-views and the user's playlists, flattened into one
//! virtualised model with a header per store. The sidebar's selection
//! follows the current location; the row of whatever is playing shows a
//! speaker.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gio, glib};

use super::context::Ctx;
use super::nav::{CollectionView, Location, Navigator, Route};
use crate::covers::{Image, Ticket};
use crate::library::Change;
use crate::playback::Event;
use crate::tidal::models::Playlist;

/// What a sidebar row stands for.
#[derive(Clone, Debug)]
enum Entry {
    Nav {
        title: &'static str,
        icon: &'static str,
        route: Route,
    },
    Playlist {
        route: Route,
        title: String,
        image: Option<String>,
        /// The creator, for playlists the user does not own.
        owner: Option<String>,
    },
    /// Shown instead of the playlists while there are none.
    NewPlaylist,
}

impl Entry {
    /// The tag of the page it opens (see [`Route::tag`]).
    fn key(&self) -> Option<String> {
        match self {
            Self::Nav { route, .. } | Self::Playlist { route, .. } => Some(route.tag()),
            Self::NewPlaylist => None,
        }
    }

    fn of(object: &glib::Object) -> Option<Self> {
        object
            .downcast_ref::<glib::BoxedAnyObject>()
            .map(|boxed| boxed.borrow::<Entry>().clone())
    }

    fn object(self) -> glib::BoxedAnyObject {
        glib::BoxedAnyObject::new(self)
    }
}

/// The widgets of one row, rebound as the list recycles it.
struct Row {
    icon: gtk::Image,
    cover: gtk::Image,
    label: gtk::Label,
    speaker: gtk::Image,
    root: gtk::Box,
    key: RefCell<Option<String>>,
    ticket: RefCell<Option<Ticket>>,
}

pub struct Sidebar {
    ctx: Weak<Ctx>,
    navigator: Weak<Navigator>,
    collection_store: gio::ListStore,
    playlist_store: gio::ListStore,
    selection: gtk::SingleSelection,
    /// The sidebar pane (header bar + list).
    pub widget: adw::ToolbarView,
    rows: RefCell<Vec<Weak<Row>>>,
    /// Key of the row whose source is playing.
    playing: RefCell<Option<String>>,
    /// Set while the selection is changed to follow the location.
    syncing: Cell<bool>,
    library_listener: Cell<u64>,
    playback_subscriber: Cell<u64>,
}

impl Sidebar {
    pub fn new(ctx: &Rc<Ctx>, navigator: &Rc<Navigator>) -> Rc<Self> {
        let nav_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        for (title, icon, route) in [
            ("Home", "user-home-symbolic", Route::Home),
            ("Explore", "swell-explore-symbolic", Route::Explore),
            ("Search", "system-search-symbolic", Route::Search(None)),
        ] {
            nav_store.append(&Entry::Nav { title, icon, route }.object());
        }
        let collection_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        for (title, icon, route) in [
            ("Tracks", "swell-heart-symbolic", Route::Collection(CollectionView::Tracks)),
            ("Albums", "media-optical-symbolic", Route::Collection(CollectionView::Albums)),
            ("Artists", "avatar-default-symbolic", Route::Collection(CollectionView::Artists)),
            ("Playlists", "view-list-symbolic", Route::Collection(CollectionView::Playlists)),
            ("Mixes & Radios", "swell-mix-symbolic", Route::Collection(CollectionView::Mixes)),
            ("Recently Played", "document-open-recent-symbolic", Route::History),
        ] {
            collection_store.append(&Entry::Nav { title, icon, route }.object());
        }
        let playlist_store = gio::ListStore::new::<glib::BoxedAnyObject>();

        let sections = gio::ListStore::new::<gio::ListModel>();
        sections.append(&nav_store);
        sections.append(&collection_store);
        sections.append(&playlist_store);
        let selection = gtk::SingleSelection::builder()
            .model(&gtk::FlattenListModel::new(Some(sections)))
            .autoselect(false)
            .can_unselect(true)
            .build();
        selection.set_selected(gtk::INVALID_LIST_POSITION);

        let widget = adw::ToolbarView::new();
        let header = adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new(crate::config::APP_NAME, ""))
            .build();
        header.pack_end(&main_menu_button());
        widget.add_top_bar(&header);

        let sidebar = Rc::new(Self {
            ctx: Rc::downgrade(ctx),
            navigator: Rc::downgrade(navigator),
            collection_store,
            playlist_store,
            selection,
            widget,
            rows: RefCell::default(),
            playing: RefCell::default(),
            syncing: Cell::new(false),
            library_listener: Cell::new(0),
            playback_subscriber: Cell::new(0),
        });

        let list = gtk::ListView::builder()
            .model(&sidebar.selection)
            .factory(&sidebar.row_factory(false))
            .header_factory(&sidebar.header_factory(true))
            .css_classes(["navigation-sidebar"])
            .build();
        list.update_property(&[gtk::accessible::Property::Label("Sidebar")]);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .build();
        sidebar.widget.set_content(Some(&scrolled));

        sidebar.selection.connect_selection_changed({
            let sidebar = Rc::downgrade(&sidebar);
            move |selection, _, _| {
                let Some(sidebar) = sidebar.upgrade() else {
                    return;
                };
                if sidebar.syncing.get() {
                    return;
                }
                if let Some(entry) = selection.selected_item().as_ref().and_then(Entry::of) {
                    sidebar.open(&entry);
                }
            }
        });
        navigator.connect_location_changed({
            let sidebar = Rc::downgrade(&sidebar);
            move |location| {
                if let Some(sidebar) = sidebar.upgrade() {
                    sidebar.select(location);
                }
            }
        });

        let id = ctx.library.connect_changed({
            let sidebar = Rc::downgrade(&sidebar);
            move |change| {
                if let (Some(sidebar), Change::Playlists) = (sidebar.upgrade(), change) {
                    sidebar.reload_playlists();
                }
            }
        });
        sidebar.library_listener.set(id);
        let id = ctx.playback.subscribe({
            let sidebar = Rc::downgrade(&sidebar);
            move |event| {
                if let (Some(sidebar), Event::Track(_)) = (sidebar.upgrade(), event) {
                    sidebar.update_playing();
                }
            }
        });
        sidebar.playback_subscriber.set(id);
        sidebar.reload_playlists_if_loaded();
        sidebar.update_playing();
        sidebar
    }

    /// The Collection index page: the narrow layout's Collection root.
    pub fn index_page(self: &Rc<Self>) -> adw::NavigationPage {
        let sections = gio::ListStore::new::<gio::ListModel>();
        sections.append(&self.collection_store);
        sections.append(&self.playlist_store);
        let model = gtk::NoSelection::new(Some(gtk::FlattenListModel::new(Some(sections))));
        let list = gtk::ListView::builder()
            .model(&model)
            .factory(&self.row_factory(true))
            .header_factory(&self.header_factory(false))
            .single_click_activate(true)
            .css_classes(["navigation-sidebar", "collection-index"])
            .build();
        list.update_property(&[gtk::accessible::Property::Label("Collection")]);
        list.connect_activate({
            let sidebar = Rc::downgrade(self);
            move |list, position| {
                let entry = list
                    .model()
                    .and_then(|model| model.item(position))
                    .as_ref()
                    .and_then(Entry::of);
                if let (Some(sidebar), Some(entry)) = (sidebar.upgrade(), entry) {
                    sidebar.open(&entry);
                }
            }
        });
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .build();
        let header = adw::HeaderBar::new();
        header.pack_end(&main_menu_button());
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scrolled));
        adw::NavigationPage::builder()
            .title("Collection")
            .child(&toolbar)
            .build()
    }

    fn open(&self, entry: &Entry) {
        let Some(navigator) = self.navigator.upgrade() else {
            return;
        };
        match entry {
            Entry::Nav { route, .. } | Entry::Playlist { route, .. } => {
                if route.is_collection_root() {
                    navigator.open_collection_root(route.clone());
                } else {
                    navigator.navigate(route.clone());
                }
            }
            Entry::NewPlaylist => {
                if let Some(ctx) = self.ctx.upgrade() {
                    new_playlist(&ctx);
                }
                self.select(&navigator.location());
            }
        }
    }

    /// Selects the row of `location`: the open playlist if it has a row,
    /// else the section's root.
    fn select(&self, location: &Location) {
        let mut position = gtk::INVALID_LIST_POSITION;
        let mut root_position = gtk::INVALID_LIST_POSITION;
        let section_key = match location.section.as_str() {
            "collection" => None,
            section => Some(section.to_owned()),
        };
        for i in 0..self.selection.n_items() {
            let Some(key) = self.selection.item(i).as_ref().and_then(Entry::of).and_then(|e| e.key()) else {
                continue;
            };
            if location.visible.as_ref() == Some(&key) && key.starts_with("playlist-") {
                position = i;
                break;
            }
            if root_position == gtk::INVALID_LIST_POSITION
                && (location.root.as_ref() == Some(&key) || section_key.as_ref() == Some(&key))
            {
                root_position = i;
            }
        }
        if position == gtk::INVALID_LIST_POSITION {
            position = root_position;
        }
        if self.selection.selected() != position {
            self.syncing.set(true);
            self.selection.set_selected(position);
            self.syncing.set(false);
        }
    }

    fn reload_playlists_if_loaded(&self) {
        if let Some(ctx) = self.ctx.upgrade()
            && !ctx.library.playlists().is_empty()
        {
            self.reload_playlists();
        }
    }

    fn reload_playlists(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let playlists = ctx.library.playlists();
        let entries: Vec<glib::BoxedAnyObject> = if playlists.is_empty() {
            vec![Entry::NewPlaylist.object()]
        } else {
            playlists
                .iter()
                .map(|playlist| playlist_entry(&ctx, playlist).object())
                .collect()
        };
        self.syncing.set(true);
        self.playlist_store
            .splice(0, self.playlist_store.n_items(), &entries);
        self.syncing.set(false);
        if let Some(navigator) = self.navigator.upgrade() {
            self.select(&navigator.location());
        }
    }

    fn update_playing(&self) {
        let key = self.ctx.upgrade().and_then(|ctx| {
            let target = ctx.playback.source()?.target?;
            Some(Route::from_target(&target)?.tag())
        });
        if *self.playing.borrow() == key {
            return;
        }
        self.playing.replace(key);
        self.rows.borrow_mut().retain(|row| row.strong_count() > 0);
        for row in self.rows.borrow().iter().filter_map(Weak::upgrade) {
            self.show_speaker(&row);
        }
    }

    fn show_speaker(&self, row: &Row) {
        let playing = row.key.borrow().is_some() && *row.key.borrow() == *self.playing.borrow();
        row.speaker.set_visible(playing);
    }

    fn row_factory(self: &Rc<Self>, index: bool) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let sidebar = Rc::downgrade(self);
        factory.connect_setup(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(this) = sidebar.upgrade() else {
                return;
            };
            let row = Rc::new(Row::new());
            this.rows.borrow_mut().push(Rc::downgrade(&row));
            item.set_child(Some(&row.root));
            item.connect_item_notify({
                let row = row.clone();
                let sidebar = sidebar.clone();
                move |item| {
                    if let Some(sidebar) = sidebar.upgrade() {
                        sidebar.bind(&row, item.item().as_ref().and_then(Entry::of));
                    }
                }
            });
            if !index {
                // Clicking the selected row again goes back to its root.
                let was_selected = Rc::new(Cell::new(false));
                let click = gtk::GestureClick::new();
                click.connect_pressed({
                    let was_selected = was_selected.clone();
                    let item = item.downgrade();
                    move |_, _, _, _| {
                        was_selected.set(item.upgrade().is_some_and(|item| item.is_selected()));
                    }
                });
                click.connect_released({
                    let item = item.downgrade();
                    let sidebar = sidebar.clone();
                    move |_, _, _, _| {
                        if !was_selected.get() {
                            return;
                        }
                        let entry = item.upgrade().and_then(|item| item.item()).as_ref().and_then(Entry::of);
                        if let (Some(sidebar), Some(entry)) = (sidebar.upgrade(), entry) {
                            sidebar.open(&entry);
                        }
                    }
                });
                row.root.add_controller(click);
            }
        });
        factory
    }

    fn bind(&self, row: &Row, entry: Option<Entry>) {
        row.ticket.take();
        row.cover.set_paintable(None::<&gtk::gdk::Paintable>);
        row.root.set_tooltip_text(None);
        row.root.remove_css_class("playlist");
        let Some(entry) = entry else {
            row.key.replace(None);
            return;
        };
        row.key.replace(entry.key());
        match &entry {
            Entry::Nav { title, icon, .. } => {
                row.icon.set_icon_name(Some(icon));
                row.icon.set_visible(true);
                row.cover.set_visible(false);
                row.label.set_label(title);
            }
            Entry::NewPlaylist => {
                row.icon.set_icon_name(Some("list-add-symbolic"));
                row.icon.set_visible(true);
                row.cover.set_visible(false);
                row.label.set_label("New Playlist");
            }
            Entry::Playlist {
                title,
                image,
                owner,
                ..
            } => {
                row.root.add_css_class("playlist");
                row.icon.set_visible(false);
                row.cover.set_visible(true);
                row.label.set_label(title);
                if let Some(owner) = owner {
                    row.root.set_tooltip_text(Some(&format!("by {owner}")));
                }
                if let (Some(ctx), Some(image)) = (self.ctx.upgrade(), image) {
                    let pixels = (32 * row.root.scale_factor().max(1)) as u32;
                    let cover = row.cover.downgrade();
                    let ticket = ctx.covers.load(&Image::Playlist(image.clone()), pixels, 0, move |texture| {
                        if let Some(cover) = cover.upgrade() {
                            cover.set_paintable(Some(texture));
                        }
                    });
                    row.ticket.replace(ticket);
                }
            }
        }
        self.show_speaker(row);
    }

    /// Section headers: none above the navigation rows, "Collection"
    /// (sidebar only) and "Playlists" with the New Playlist button.
    fn header_factory(self: &Rc<Self>, collection_header: bool) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let ctx = self.ctx.clone();
        factory.connect_setup(move |_, header| {
            let Some(header) = header.downcast_ref::<gtk::ListHeader>() else {
                return;
            };
            let label = gtk::Label::builder()
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["caption-heading", "dimmed"])
                .build();
            let add = gtk::Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text("New Playlist")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .visible(false)
                .build();
            add.connect_clicked({
                let ctx = ctx.clone();
                move |_| {
                    if let Some(ctx) = ctx.upgrade() {
                        new_playlist(&ctx);
                    }
                }
            });
            let content = gtk::Box::builder()
                .spacing(6)
                .css_classes(["sidebar-header"])
                .build();
            content.append(&label);
            content.append(&add);
            header.set_child(Some(&content));
            header.connect_item_notify(move |header| {
                let entry = header.item().as_ref().and_then(Entry::of);
                let (title, show_add) = match entry {
                    Some(Entry::Nav { route, .. }) if route.section().is_some() && !route.is_collection_root() => (None, false),
                    Some(Entry::Nav { .. }) => (collection_header.then_some("COLLECTION"), false),
                    Some(Entry::Playlist { .. } | Entry::NewPlaylist) => (Some("PLAYLISTS"), true),
                    None => (None, false),
                };
                content.set_visible(title.is_some());
                label.set_label(title.unwrap_or_default());
                add.set_visible(show_add);
            });
        });
        factory
    }
}

impl Drop for Sidebar {
    fn drop(&mut self) {
        if let Some(ctx) = self.ctx.upgrade() {
            ctx.library.disconnect(self.library_listener.get());
            ctx.playback.unsubscribe(self.playback_subscriber.get());
        }
    }
}

impl Row {
    fn new() -> Self {
        let icon = gtk::Image::builder().pixel_size(16).build();
        let cover = gtk::Image::builder()
            .pixel_size(32)
            .valign(gtk::Align::Center)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["sidebar-cover"])
            .visible(false)
            .build();
        let label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let speaker = gtk::Image::builder()
            .icon_name("audio-volume-high-symbolic")
            .tooltip_text("Playing")
            .visible(false)
            .build();
        speaker.update_property(&[gtk::accessible::Property::Label("Playing")]);
        let root = gtk::Box::builder()
            .spacing(12)
            .css_classes(["sidebar-row"])
            .build();
        root.append(&icon);
        root.append(&cover);
        root.append(&label);
        root.append(&speaker);
        Self {
            icon,
            cover,
            label,
            speaker,
            root,
            key: RefCell::default(),
            ticket: RefCell::default(),
        }
    }
}

fn playlist_entry(ctx: &Ctx, playlist: &Playlist) -> Entry {
    let owner = (!ctx.library.is_own_playlist(playlist))
        .then(|| playlist.creator.name.clone())
        .flatten()
        .filter(|name| !name.is_empty());
    Entry::Playlist {
        route: Route::Playlist(playlist.uuid.clone()),
        title: playlist.title.clone(),
        image: playlist
            .square_image
            .clone()
            .or_else(|| playlist.image.clone()),
        owner,
    }
}

/// Starts creating a playlist.
fn new_playlist(ctx: &Ctx) {
    // The playlist dialog (ui/dialogs/playlist.rs) is not built yet.
    ctx.toast("Creating playlists is coming soon");
}

/// The main menu (docs/design.md §2.3).
pub fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let general = gio::Menu::new();
    general.append(Some("_Preferences"), Some("win.preferences"));
    general.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
    general.append(Some("_About Swell"), Some("win.about"));
    menu.append_section(None, &general);
    let account = gio::Menu::new();
    account.append(Some("_Sign Out…"), Some("win.sign-out"));
    menu.append_section(None, &account);
    let quit = gio::Menu::new();
    quit.append(Some("_Quit"), Some("app.quit"));
    menu.append_section(None, &quit);
    menu
}

/// A "Main Menu" button (☰) with [`main_menu`]. Pages whose header should
/// carry it in the narrow layout can pack one and keep it visible only
/// while the window has the `narrow` CSS class.
pub fn main_menu_button() -> gtk::MenuButton {
    let button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
        .menu_model(&main_menu())
        .primary(true)
        .build();
    button.update_property(&[gtk::accessible::Property::Label("Main Menu")]);
    button
}

/// The main menu button for the header of a section's root page: visible
/// only in the narrow layout, where there is no sidebar to carry it.
pub fn narrow_main_menu_button(window: &adw::ApplicationWindow) -> gtk::MenuButton {
    let button = main_menu_button();
    follow_window(&button, window, |window| window.has_css_class("narrow"));
    button
}

/// A "Show Sidebar" button for the start of a content header: visible only
/// while the sidebar is hidden (F9) in the wide layout.
pub fn show_sidebar_button(window: &adw::ApplicationWindow) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Show Sidebar")
        .action_name("win.show-sidebar")
        .build();
    button.update_property(&[gtk::accessible::Property::Label("Show Sidebar")]);
    let hidden = |window: &adw::ApplicationWindow| {
        let shown = window
            .lookup_action("show-sidebar")
            .and_then(|action| action.state())
            .and_then(|state| state.get::<bool>())
            .unwrap_or(true);
        !shown && !window.has_css_class("narrow")
    };
    follow_window(&button, window, hidden);
    if let Some(action) = window.lookup_action("show-sidebar") {
        let weak = button.downgrade();
        let window = window.downgrade();
        let id = action.connect_notify_local(Some("state"), move |_, _| {
            if let (Some(button), Some(window)) = (weak.upgrade(), window.upgrade()) {
                button.set_visible(hidden(&window));
            }
        });
        let action = action.downgrade();
        let id = Cell::new(Some(id));
        button.connect_destroy(move |_| {
            if let (Some(action), Some(id)) = (action.upgrade(), id.take()) {
                action.disconnect(id);
            }
        });
    }
    button
}

/// Keeps `widget` visible exactly when `visible(window)`, re-checked when
/// the window's CSS classes change (the `narrow` class).
fn follow_window(
    widget: &impl IsA<gtk::Widget>,
    window: &adw::ApplicationWindow,
    visible: impl Fn(&adw::ApplicationWindow) -> bool + 'static,
) {
    widget.set_visible(visible(window));
    let weak = widget.upcast_ref::<gtk::Widget>().downgrade();
    let id = window.connect_css_classes_notify(move |window| {
        if let Some(widget) = weak.upgrade() {
            widget.set_visible(visible(window));
        }
    });
    let window = window.downgrade();
    let id = Cell::new(Some(id));
    widget.connect_destroy(move |_| {
        if let (Some(window), Some(id)) = (window.upgrade(), id.take()) {
            window.disconnect(id);
        }
    });
}
