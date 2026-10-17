//! Collection sub-views (docs/design.md §3.5): the favourite tracks here,
//! the grids of albums, artists, playlists and mixes in `collection_grid`.
//!
//! Every sub-view shares one [`Template`]: a header bar with the view's
//! name and count, and a toolbar under it with the view's own buttons, a
//! filter (typing in the list filters too) and the sort menu. Filtering and
//! sorting work on a `Vec` kept beside the model and then replace the
//! model's items: instant for thousands of rows, and the virtualised view
//! only binds what is on screen.
//!
//! Favourite tracks are drawn from the SQLite cache first, then checked
//! against TIDAL with one request (`Api::sync_favorite_tracks`); hearts
//! flipped anywhere in the app add or remove rows, and the cache follows.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gio, glib};

use super::super::context::Ctx;
use super::super::format;
use super::super::menus;
use super::super::nav::{CollectionView, Route};
use super::super::sidebar;
use super::super::widgets::skeleton;
use super::super::widgets::status::PageStack;
use super::super::widgets::track_list::{TrackList, Variant, compact_bin};
use super::collection_grid;
use crate::api::{CachedHead, FavoritesUpdate};
use crate::i18n::{tr, trn};
use crate::library::{Change, Item};
use crate::model::TrackInfo;
use crate::playback::Source;
use crate::runtime;
use crate::tidal::pages::Target;

pub fn page(ctx: &Rc<Ctx>, view: CollectionView) -> adw::NavigationPage {
    match view {
        CollectionView::Tracks => tracks_page(ctx),
        other => collection_grid::page(ctx, other),
    }
}

// ---- The shared template ----

/// A sort order a view offers: id, label (untranslated: `tr` is applied
/// where it is shown), and whether it starts descending when chosen
/// (newest first, for dates).
pub(super) type SortOption = (&'static str, &'static str, bool);

thread_local! {
    /// Each view's sort order, remembered while the app runs.
    static SORTS: RefCell<HashMap<&'static str, (usize, bool)>> = RefCell::default();
}

type Hook = RefCell<Option<Rc<dyn Fn()>>>;
type CompactHook = RefCell<Option<Rc<dyn Fn(bool)>>>;

/// Header bar, toolbar (own buttons · filter · sort) and the page states.
/// The page's state keeps it and the page owns that state, so it refers to
/// the page weakly.
pub(super) struct Template {
    page: glib::WeakRef<adw::NavigationPage>,
    pub title: adw::WindowTitle,
    pub header_bar: adw::HeaderBar,
    /// The start of the toolbar, for the view's own buttons.
    pub actions: gtk::Box,
    pub filter: gtk::SearchEntry,
    pub stack: Rc<PageStack>,
    bar: gtk::Box,
    row: gtk::Box,
    /// Where wide `actions` go in narrow windows.
    second_row: gtk::Box,
    wrap_actions: Cell<bool>,
    spacer: gtk::Box,
    sort_button: gtk::MenuButton,
    /// The filter words last applied.
    last_query: RefCell<Vec<String>>,
    content: gtk::Stack,
    no_matches: adw::StatusPage,
    name: &'static str,
    sorts: &'static [SortOption],
    sort: Cell<usize>,
    descending: Cell<bool>,
    compact: Cell<bool>,
    on_change: Hook,
    on_compact: CompactHook,
}

impl Template {
    /// The template and its page. `main` is the list or grid (in its
    /// scrolled window); `name` keys the remembered sort order. Set the
    /// empty state with [`Template::set_empty`].
    pub fn new(
        ctx: &Rc<Ctx>,
        name: &'static str,
        title: &str,
        placeholder: &str,
        sorts: &'static [SortOption],
        skeleton: &impl IsA<gtk::Widget>,
        main: &impl IsA<gtk::Widget>,
    ) -> (Rc<Self>, adw::NavigationPage) {
        let window = &ctx.window;
        let title_widget = adw::WindowTitle::new(title, "");
        let header_bar = adw::HeaderBar::builder()
            .title_widget(&title_widget)
            .build();
        header_bar.pack_start(&sidebar::show_sidebar_button(window));
        header_bar.pack_end(&sidebar::narrow_main_menu_button(window));

        let actions = gtk::Box::builder().spacing(6).visible(false).build();
        let spacer = gtk::Box::builder().hexpand(true).build();
        let filter = gtk::SearchEntry::builder()
            .placeholder_text(placeholder)
            .width_chars(18)
            .search_delay(60)
            .build();
        filter.update_property(&[gtk::accessible::Property::Label(placeholder)]);
        let sort_button = gtk::MenuButton::builder()
            .always_show_arrow(true)
            .tooltip_text(tr("Sort"))
            .build();
        let row = gtk::Box::builder().spacing(6).build();
        row.append(&actions);
        row.append(&spacer);
        row.append(&filter);
        row.append(&sort_button);
        let second_row = gtk::Box::builder().visible(false).build();
        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .css_classes(["page-content"])
            .visible(false)
            .build();
        bar.append(&row);
        bar.append(&second_row);

        let no_matches = adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .css_classes(["compact"])
            .build();
        let content = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        content.add_named(main, Some("main"));
        content.add_named(&no_matches, Some("no-matches"));
        let stack = PageStack::new(skeleton);
        stack.set_content(&content);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header_bar);
        toolbar.add_top_bar(&bar);
        toolbar.set_content(Some(stack.widget()));
        let page = adw::NavigationPage::builder().title(title).build();

        let (sort, descending) = SORTS
            .with(|s| s.borrow().get(name).copied())
            .unwrap_or((0, sorts[0].2));
        let this = Rc::new(Self {
            page: page.downgrade(),
            title: title_widget,
            header_bar,
            actions,
            filter,
            stack,
            bar,
            row,
            second_row,
            wrap_actions: Cell::new(false),
            spacer,
            sort_button,
            last_query: RefCell::default(),
            content,
            no_matches,
            name,
            sorts,
            sort: Cell::new(sort.min(sorts.len() - 1)),
            descending: Cell::new(descending),
            compact: Cell::new(false),
            on_change: RefCell::default(),
            on_compact: RefCell::default(),
        });

        let weak = Rc::downgrade(&this);
        let bin = compact_bin(&toolbar, move |compact| {
            if let Some(this) = weak.upgrade() {
                this.set_compact(compact);
            }
        });
        page.set_child(Some(&bin));

        let weak = Rc::downgrade(&this);
        this.filter.connect_search_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.filter_changed();
            }
        });
        this.filter.connect_stop_search(|entry| entry.set_text(""));
        this.install_sort_menu();
        (this, page)
    }

    /// Whether the page is on screen.
    pub fn is_mapped(&self) -> bool {
        self.page.upgrade().is_some_and(|page| page.is_mapped())
    }

    /// The filter's text changed; `search-changed` also fires with nothing
    /// typed (when the entry is set up), which changes nothing.
    fn filter_changed(&self) {
        let query = self.query();
        if self.last_query.replace(query.clone()) != query {
            self.changed();
        }
    }

    fn install_sort_menu(self: &Rc<Self>) {
        let group = gio::SimpleActionGroup::new();
        let order = gio::SimpleAction::new_stateful(
            "order",
            Some(glib::VariantTy::STRING),
            &order_name(self.descending.get()).to_variant(),
        );
        let weak = Rc::downgrade(self);
        order.connect_change_state(move |action, value| {
            let (Some(this), Some(value)) = (weak.upgrade(), value) else {
                return;
            };
            action.set_state(value);
            this.descending
                .set(value.get::<String>().as_deref() == Some("descending"));
            this.sort_changed();
        });
        group.add_action(&order);
        let by = gio::SimpleAction::new_stateful(
            "by",
            Some(glib::VariantTy::STRING),
            &self.sorts[self.sort.get()].0.to_variant(),
        );
        let weak = Rc::downgrade(self);
        let order_action = order.clone();
        by.connect_change_state(move |action, value| {
            let (Some(this), Some(id)) = (weak.upgrade(), value.and_then(|v| v.get::<String>()))
            else {
                return;
            };
            let Some(index) = this.sorts.iter().position(|s| s.0 == id) else {
                return;
            };
            action.set_state(&id.to_variant());
            // Each order starts the natural way round (dates newest first).
            this.sort.set(index);
            this.descending.set(this.sorts[index].2);
            order_action.set_state(&order_name(this.descending.get()).to_variant());
            this.sort_changed();
        });
        group.add_action(&by);
        self.sort_button.insert_action_group("sort", Some(&group));

        let menu = gio::Menu::new();
        let keys = gio::Menu::new();
        for (id, label, _) in self.sorts {
            keys.append(Some(tr(label)), Some(&format!("sort.by::{id}")));
        }
        menu.append_section(None, &keys);
        let orders = gio::Menu::new();
        orders.append(Some(tr("Ascending")), Some("sort.order::ascending"));
        orders.append(Some(tr("Descending")), Some("sort.order::descending"));
        menu.append_section(None, &orders);
        self.sort_button.set_menu_model(Some(&menu));
        self.update_sort_label();
    }

    fn sort_changed(&self) {
        SORTS.with(|s| {
            s.borrow_mut()
                .insert(self.name, (self.sort.get(), self.descending.get()))
        });
        self.update_sort_label();
        self.changed();
    }

    fn update_sort_label(&self) {
        let label = tr(self.sorts[self.sort.get()].1);
        self.sort_button.set_label(label);
        let description = if self.descending.get() {
            tr("Sort by {label}, descending")
        } else {
            tr("Sort by {label}, ascending")
        };
        self.sort_button
            .update_property(&[gtk::accessible::Property::Label(
                &description.replace("{label}", label),
            )]);
    }

    fn changed(&self) {
        let hook = self.on_change.borrow().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// What shows when the collection has nothing yet.
    pub fn set_empty(&self, empty: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.content.child_by_name("empty") {
            self.content.remove(&old);
        }
        self.content.add_named(empty, Some("empty"));
    }

    /// Called when the filter or the sort order changes.
    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        self.on_change.replace(Some(Rc::new(f)));
    }

    /// Called when the page gets narrower or wider than 640 px.
    pub fn connect_compact(&self, f: impl Fn(bool) + 'static) {
        let f: Rc<dyn Fn(bool)> = Rc::new(f);
        f(self.compact.get());
        self.on_compact.replace(Some(f));
    }

    fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        self.filter.set_hexpand(compact);
        self.filter.set_width_chars(if compact { 8 } else { 18 });
        self.spacer.set_visible(!compact);
        self.place_actions();
        let hook = self.on_compact.borrow().clone();
        if let Some(hook) = hook {
            hook(compact);
        }
    }

    /// Moves the view's own buttons to a row of their own, full width, in
    /// narrow windows (for a toggle group too wide to share the row).
    pub fn wrap_actions_when_compact(&self) {
        self.wrap_actions.set(true);
        self.place_actions();
    }

    fn place_actions(&self) {
        let wrap = self.wrap_actions.get() && self.compact.get();
        let target = if wrap { &self.second_row } else { &self.row };
        if self.actions.parent().as_ref() == Some(target.upcast_ref()) {
            return;
        }
        if let Some(parent) = self.actions.parent().and_downcast::<gtk::Box>() {
            parent.remove(&self.actions);
        }
        if wrap {
            self.second_row.append(&self.actions);
        } else {
            self.row.prepend(&self.actions);
        }
        let mut child = self.actions.first_child();
        while let Some(widget) = child {
            widget.set_hexpand(wrap);
            child = widget.next_sibling();
        }
        self.second_row.set_visible(wrap);
    }

    /// Typing with the focus in `widget` (the list or grid) filters.
    pub fn capture_keys_from(&self, widget: &impl IsA<gtk::Widget>) {
        self.filter.set_key_capture_widget(Some(widget));
    }

    /// The filter as folded words, each of which must match.
    pub fn query(&self) -> Vec<String> {
        fold(&self.filter.text())
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    /// The chosen sort id and whether it is descending.
    pub fn sort(&self) -> (&'static str, bool) {
        (self.sorts[self.sort.get()].0, self.descending.get())
    }

    /// Whether the view shows everything in its natural order (the first
    /// sort, as it starts, without a filter): new items can be appended.
    pub fn is_natural(&self) -> bool {
        self.sort.get() == 0
            && self.descending.get() == self.sorts[0].2
            && self.filter.text().is_empty()
    }

    /// Shows the content: the list, the empty state (`total` is 0) or "No
    /// Matches" (`shown` is 0).
    pub fn show_items(&self, shown: usize, total: usize) {
        self.bar.set_visible(total > 0);
        let name = if total == 0 {
            "empty"
        } else if shown == 0 {
            self.no_matches.set_title(
                &tr("No Matches for “{query}”").replace("{query}", self.filter.text().trim()),
            );
            "no-matches"
        } else {
            "main"
        };
        self.content.set_visible_child_name(name);
        self.stack.show_content();
    }

    pub fn is_showing_content(&self) -> bool {
        self.stack.is_showing_content()
    }
}

fn order_name(descending: bool) -> &'static str {
    if descending {
        "descending"
    } else {
        "ascending"
    }
}

/// Lowercase without accents ("Beyoncé" → "beyonce"), for filtering.
pub(super) fn fold(text: &str) -> String {
    glib::normalize(text, glib::NormalizeMode::All)
        .chars()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// "1,234 albums", "1 album", or "12 of 1,234 albums" while filtering.
/// `noun` is the translated count with `{n}` for the number:
/// `|n| trn("{n} album", "{n} albums", n)`.
pub(super) fn count_label(
    shown: usize,
    total: usize,
    noun: impl Fn(u64) -> &'static str,
) -> String {
    if total == 0 {
        // The empty state says it.
        return String::new();
    }
    let count = noun(total as u64).replace("{n}", &format::group_digits(total));
    if shown == total {
        count
    } else {
        tr("{shown} of {count}")
            .replace("{shown}", &format::group_digits(shown))
            .replace("{count}", &count)
    }
}

/// An empty state with a hint and, optionally, a button.
pub(super) fn empty_page(
    icon: &str,
    title: &str,
    description: &str,
    button: Option<&gtk::Button>,
) -> adw::StatusPage {
    let status = adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(glib::markup_escape_text(description).as_str())
        .build();
    if let Some(button) = button {
        button.add_css_class("pill");
        button.set_halign(gtk::Align::Center);
        status.set_child(Some(button));
    }
    status
}

/// Ghost track rows, as many as fit.
pub(super) fn skeleton_rows() -> gtk::Widget {
    let rows = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(6)
        .build();
    rows.update_property(&[gtk::accessible::Property::Label(tr("Loading"))]);
    for _ in 0..14 {
        rows.append(&skeleton::row());
    }
    let clamp = adw::Clamp::builder()
        .maximum_size(1200)
        .tightening_threshold(1200)
        .child(&rows)
        .valign(gtk::Align::Start)
        .build();
    clipped(&clamp)
}

/// Ghost cards, as many as fit; `round` for artists.
pub(super) fn skeleton_cards(round: bool) -> gtk::Widget {
    let cards = adw::WrapBox::builder()
        .child_spacing(12)
        .line_spacing(12)
        .margin_top(12)
        .margin_start(18)
        .margin_end(18)
        .build();
    cards
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label(tr("Loading"))]);
    for _ in 0..18 {
        let card = skeleton::card(160);
        if round && let Some(cover) = card.first_child() {
            cover.set_overflow(gtk::Overflow::Hidden);
            cover.add_css_class("circular");
        }
        cards.append(&card);
    }
    clipped(&cards)
}

/// Clipped rather than scrolled: a placeholder never asks for room.
fn clipped(child: &impl IsA<gtk::Widget>) -> gtk::Widget {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(child)
        .build()
        .upcast()
}

/// A toolbar button that is a pill with a label in wide windows and a
/// round icon in narrow ones.
pub(super) struct ToolButton {
    pub button: gtk::Button,
    content: adw::ButtonContent,
    icon: &'static str,
}

impl ToolButton {
    pub fn new(icon: &'static str, label: &'static str, primary: bool) -> Self {
        let content = adw::ButtonContent::builder()
            .icon_name(icon)
            .label(label)
            .build();
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["pill"])
            .build();
        if primary {
            button.add_css_class("suggested-action");
        }
        super::super::widgets::label_button(&button, label);
        Self {
            button,
            content,
            icon,
        }
    }

    pub fn set_compact(&self, compact: bool) {
        if compact {
            self.button.set_icon_name(self.icon);
            self.button.remove_css_class("pill");
            self.button.add_css_class("circular");
        } else {
            self.button.set_child(Some(&self.content));
            self.button.remove_css_class("circular");
            self.button.add_css_class("pill");
        }
    }
}

/// Runs `f` for every change of the library while `page` lives.
pub(super) fn follow_library(
    ctx: &Rc<Ctx>,
    page: &adw::NavigationPage,
    f: impl Fn(&Change) + 'static,
) {
    let id = ctx.library.connect_changed(f);
    let weak = Rc::downgrade(ctx);
    page.connect_destroy(move |_| {
        if let Some(ctx) = weak.upgrade() {
            ctx.library.disconnect(id);
        }
    });
}

// ---- Tracks ----

const TRACK_SORTS: &[SortOption] = &[
    ("added", "Date Added", true),
    ("title", "Title", false),
    ("artist", "Artist", false),
    ("album", "Album", false),
    ("duration", "Duration", false),
];

/// A favourite track with its folded texts, for filtering and sorting.
struct Row {
    info: TrackInfo,
    title: String,
    artist: String,
    album: String,
}

impl Row {
    fn new(info: TrackInfo) -> Rc<Self> {
        Rc::new(Self {
            title: fold(&info.title),
            artist: fold(&info.artist_names()),
            album: fold(&info.album),
            info,
        })
    }

    fn matches(&self, words: &[String]) -> bool {
        words.iter().all(|word| {
            self.title.contains(word.as_str())
                || self.artist.contains(word.as_str())
                || self.album.contains(word.as_str())
        })
    }
}

struct TracksPage {
    ctx: Weak<Ctx>,
    template: Rc<Template>,
    list: TrackList,
    play: ToolButton,
    shuffle: ToolButton,
    /// Every favourite, newest first.
    all: RefCell<Vec<Rc<Row>>>,
    /// Ids of the rows in the list, in order.
    shown: RefCell<Vec<u64>>,
    /// The list matches TIDAL (so local changes may be cached).
    synced: Cell<bool>,
    /// Rows from the cache or TIDAL have been drawn once.
    drawn: Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
}

fn source() -> Source {
    Source {
        title: tr("Favourite Tracks").into(),
        target: Some(Target::Collection("tracks".into())),
    }
}

fn tracks_page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let list = TrackList::new(ctx, Variant::Cover);
    list.set_source(source());
    let explore = gtk::Button::builder()
        .label(tr("Explore"))
        .css_classes(["suggested-action"])
        .build();
    let weak_ctx = Rc::downgrade(ctx);
    explore.connect_clicked(move |_| {
        if let Some(ctx) = weak_ctx.upgrade() {
            ctx.navigate(Route::Explore);
        }
    });
    let empty = empty_page(
        "swell-heart-symbolic",
        tr("No Favourite Tracks"),
        tr("Use the heart on a track to save it here."),
        Some(&explore),
    );
    let (template, page) = Template::new(
        ctx,
        "tracks",
        tr("Tracks"),
        tr("Filter tracks"),
        TRACK_SORTS,
        &skeleton_rows(),
        list.widget(),
    );
    template.set_empty(&empty);
    if let Some(view) = list.view() {
        template.capture_keys_from(&view);
    }

    let play = ToolButton::new("media-playback-start-symbolic", tr("Play"), true);
    let shuffle = ToolButton::new("media-playlist-shuffle-symbolic", tr("Shuffle"), false);
    template.actions.append(&play.button);
    template.actions.append(&shuffle.button);
    template.actions.set_visible(true);

    let state = Rc::new(TracksPage {
        ctx: Rc::downgrade(ctx),
        template: template.clone(),
        list,
        play,
        shuffle,
        all: RefCell::default(),
        shown: RefCell::default(),
        synced: Cell::new(false),
        drawn: Cell::new(false),
        task: RefCell::default(),
    });

    let weak = Rc::downgrade(&state);
    state.play.button.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.play_all(false);
        }
    });
    let weak = Rc::downgrade(&state);
    state.shuffle.button.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.play_all(true);
        }
    });
    let weak = Rc::downgrade(&state);
    template.connect_changed(move || {
        if let Some(state) = weak.upgrade() {
            state.apply();
            state.list.scroll_to_top();
        }
    });
    let weak = Rc::downgrade(&state);
    template.connect_compact(move |compact| {
        if let Some(state) = weak.upgrade() {
            state.list.set_compact(compact);
            state.play.set_compact(compact);
            state.shuffle.set_compact(compact);
        }
    });
    let weak = Rc::downgrade(&state);
    follow_library(ctx, &page, move |change| {
        let Some(state) = weak.upgrade() else {
            return;
        };
        match change {
            Change::Favorite(Item::Track(id), false) => state.removed(*id),
            Change::Favorite(Item::Track(id), true) => state.added(*id),
            _ => {}
        }
    });
    let first_map = Cell::new(true);
    page.connect_map(move |_| {
        if first_map.replace(false) {
            crate::timing::mark("tracks page mapped");
        }
    });

    state.load();
    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(state) = owned.take()
            && let Some(task) = state.task.take()
        {
            task.abort();
        }
    });
    page
}

impl TracksPage {
    fn load(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if !self.template.is_showing_content() {
            self.template.stack.show_loading();
        }
        let api = ctx.api.clone();
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let reader = api.clone();
            let (cache, cached) = runtime::spawn(async move {
                let Some(cache) = reader.cache().await.cloned() else {
                    return Ok((None, None));
                };
                let cached = cache
                    .favorite_tracks(reader.user_id())
                    .await
                    .inspect_err(|err| log::warn!("could not read cached favourites: {err:#}"))
                    .ok()
                    .flatten();
                Ok((Some(cache), cached))
            })
            .await
            .unwrap_or((None, None));
            let head = cached.as_ref().map(CachedHead::of);
            if let Some(cached) = cached {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                state.set_all(cached.tracks);
                crate::timing::mark_after_next_frame(
                    state.list.widget(),
                    "cached favourites on screen",
                );
            }

            let (updates_tx, mut updates) = tokio::sync::mpsc::unbounded_channel();
            let sync =
                runtime::runtime().spawn(api.clone().sync_favorite_tracks(cache, head, updates_tx));
            let _sync = runtime::AbortOnDrop(sync.abort_handle());
            while let Some(update) = updates.recv().await {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                match update {
                    Ok(FavoritesUpdate::Append(tracks)) => {
                        if !state.drawn.get() {
                            crate::timing::mark_after_next_frame(
                                state.list.widget(),
                                "first favourites on screen",
                            );
                        }
                        state.append(tracks);
                    }
                    Ok(FavoritesUpdate::Replace(tracks)) => state.set_all(tracks),
                    Ok(FavoritesUpdate::Finished) => {
                        state.synced.set(true);
                        state.refresh_state();
                        crate::timing::mark("favourites up to date");
                    }
                    Err(err) => return state.failed(&err),
                }
            }
        });
        self.task.replace(Some(task));
    }

    fn failed(self: &Rc<Self>, err: &anyhow::Error) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if crate::tidal::is_auth_error(err) || self.drawn.get() {
            ctx.report(tr("Couldn't update your favourite tracks"), err);
            return;
        }
        let weak = Rc::downgrade(self);
        self.template.stack.show_error(err, move || {
            if let Some(state) = weak.upgrade() {
                state.load();
            }
        });
    }

    fn set_all(&self, tracks: Vec<TrackInfo>) {
        self.all.replace(tracks.into_iter().map(Row::new).collect());
        self.apply();
    }

    fn append(&self, tracks: Vec<TrackInfo>) {
        let rows: Vec<Rc<Row>> = tracks.into_iter().map(Row::new).collect();
        self.all.borrow_mut().extend(rows.iter().cloned());
        if self.template.is_natural() {
            self.shown
                .borrow_mut()
                .extend(rows.iter().map(|row| row.info.id));
            self.list
                .append(rows.iter().map(|row| row.info.clone()).collect());
            self.refresh_state();
        } else {
            self.apply();
        }
    }

    /// Filters and sorts `all` into the list.
    fn apply(&self) {
        let words = self.template.query();
        let (sort, descending) = self.template.sort();
        let all = self.all.borrow();
        let mut rows: Vec<&Rc<Row>> = all.iter().filter(|row| row.matches(&words)).collect();
        match sort {
            // `all` is newest first.
            "added" => {
                if !descending {
                    rows.reverse();
                }
            }
            other => {
                rows.sort_by(|a, b| {
                    let order = match other {
                        "title" => a.title.cmp(&b.title),
                        "artist" => a.artist.cmp(&b.artist).then(a.album.cmp(&b.album)),
                        "album" => a
                            .album
                            .cmp(&b.album)
                            .then(a.info.volume_number.cmp(&b.info.volume_number))
                            .then(a.info.track_number.cmp(&b.info.track_number)),
                        _ => a.info.duration.cmp(&b.info.duration),
                    };
                    if descending { order.reverse() } else { order }
                });
            }
        }
        self.shown
            .replace(rows.iter().map(|row| row.info.id).collect());
        let tracks = rows.iter().map(|row| row.info.clone()).collect();
        drop(all);
        self.list.set_tracks(tracks);
        self.refresh_state();
    }

    /// Count, buttons and which state shows.
    fn refresh_state(&self) {
        let all = self.all.borrow();
        let shown = self.shown.borrow().len();
        if all.is_empty() && !self.synced.get() {
            // Nothing cached: keep loading until TIDAL answers.
            return;
        }
        self.drawn.set(true);
        let seconds: u64 = all.iter().map(|row| u64::from(row.info.duration)).sum();
        let subtitle = if shown == all.len() {
            format::tracks_summary(shown, seconds)
        } else {
            count_label(shown, all.len(), |n| trn("{n} track", "{n} tracks", n))
        };
        self.template.title.set_subtitle(&subtitle);
        // The sidebar shows the length of this list (what TIDAL serves).
        if let Some(ctx) = self.ctx.upgrade() {
            ctx.set_favorite_track_count(all.len());
        }
        self.play.button.set_sensitive(shown > 0);
        self.shuffle.button.set_sensitive(shown > 0);
        self.template.show_items(shown, all.len());
    }

    fn play_all(&self, shuffle: bool) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let tracks = self.list.tracks();
        if shuffle {
            menus::shuffle_tracks(&ctx, tracks, source());
        } else {
            menus::play_tracks(&ctx, tracks, 0, source(), false);
        }
    }

    /// A track stopped being a favourite: its row goes, with "Undo".
    fn removed(&self, id: u64) {
        let Some(position) = self.all.borrow().iter().position(|row| row.info.id == id) else {
            return;
        };
        let row = self.all.borrow_mut().remove(position);
        let index = self.shown.borrow().iter().position(|other| *other == id);
        if let Some(index) = index {
            self.shown.borrow_mut().remove(index);
            self.list.remove(index);
        }
        self.refresh_state();
        self.save(-1);
        if let Some(ctx) = self.ctx.upgrade()
            && self.template.is_mapped()
        {
            let weak = Rc::downgrade(&ctx);
            ctx.toast_with_undo(
                &tr("Removed “{title}” from Favourites")
                    .replace("{title}", &glib::markup_escape_text(&row.info.title)),
                move || {
                    if let Some(ctx) = weak.upgrade() {
                        let report = Rc::downgrade(&ctx);
                        ctx.library
                            .set_favorite(Item::Track(id), true, move |result| {
                                if let (Err(err), Some(ctx)) = (result, report.upgrade()) {
                                    ctx.report(tr("Couldn't add the track back"), &err);
                                }
                            });
                    }
                },
            );
        }
    }

    /// A track became a favourite: fetch it whole and put it first.
    fn added(self: &Rc<Self>, id: u64) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if self.all.borrow().iter().any(|row| row.info.id == id) {
            return;
        }
        let api = ctx.api.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let track = runtime::spawn(async move { api.track(id).await }).await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            let Some(ctx) = state.ctx.upgrade() else {
                return;
            };
            let track = match track {
                Ok(track) => track,
                Err(err) => return log::warn!("could not fetch new favourite {id}: {err:#}"),
            };
            // Undone meanwhile, or already listed by a sync.
            if !ctx.library.is_favorite(&Item::Track(id))
                || state.all.borrow().iter().any(|row| row.info.id == id)
            {
                return;
            }
            let mut info = TrackInfo::from(track);
            info.added = glib::DateTime::now_utc()
                .ok()
                .and_then(|now| now.format_iso8601().ok())
                .map(Into::into);
            let row = Row::new(info);
            state.all.borrow_mut().insert(0, row.clone());
            if state.template.is_natural() {
                state.shown.borrow_mut().insert(0, id);
                state.list.insert(0, row.info.clone());
                state.refresh_state();
            } else {
                state.apply();
            }
            state.save(1);
        });
    }

    /// Writes the list back to the cache after a local change (`delta`
    /// tracks added or removed), so the next start still validates it with
    /// one request.
    fn save(&self, delta: isize) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if !self.synced.get() {
            return;
        }
        let tracks: Vec<TrackInfo> = self.all.borrow().iter().map(|r| r.info.clone()).collect();
        let api = ctx.api.clone();
        runtime::runtime().spawn(async move {
            let Some(cache) = api.cache().await else {
                return;
            };
            let user = api.user_id();
            // TIDAL's total also counts tracks it no longer lists.
            let total = match cache.favorite_tracks_total(user).await {
                Ok(Some(total)) => total.saturating_add_signed(delta),
                _ => tracks.len(),
            };
            if !api.is_signed_out()
                && let Err(err) = cache.store_favorite_tracks(user, total, tracks).await
            {
                log::warn!("could not cache favourites: {err:#}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_and_counts() {
        assert_eq!(fold("Beyoncé ÀLVARO"), "beyonce alvaro");
        assert_eq!(
            fold("夜に駆ける"),
            glib::normalize("夜に駆ける", glib::NormalizeMode::All)
        );
        let tracks = |n| trn("{n} track", "{n} tracks", n);
        assert_eq!(count_label(0, 0, tracks), "");
        // The English source (tests may run in a translated locale).
        if crate::i18n::ui_language().is_none() {
            assert_eq!(count_label(3, 1317, tracks), "3 of 1,317 tracks");
            assert_eq!(count_label(1, 1, tracks), "1 track");
        }
    }
}
