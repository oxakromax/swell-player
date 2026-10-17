//! The pieces of the Search page (docs/design.md §3.4): the top hit, the
//! "All" results, the virtualised grid of cards of a single-type filter,
//! the recent searches and the small status rows used while paging.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gio, glib};

use super::super::context::Ctx;
use super::super::menus::{self, ItemMenu, PlayMode, Subject};
use super::super::widgets::carousel::{CarouselSection, MAX_CARDS, title_row};
use super::super::widgets::media_card::{self, MediaCard, attach_menu};
use super::super::widgets::section::{SHORT_LIST, activate_item};
use super::super::widgets::track_list::{TrackList, Variant};
use super::search_data::{Filter, Results};
use crate::playback::Source;
use crate::tidal::pages::Item;

/// Logical size of the top hit's artwork (§1.5).
const TOP_HIT_SIZE: i32 = 120;
/// Width of the top hit beside the tracks.
const TOP_HIT_WIDTH: i32 = 360;

/// What "Playing from …" says for search results.
pub fn source(query: &str) -> Source {
    Source {
        title: format!("Search “{query}”"),
        target: None,
    }
}

/// The "All" results: the top hit beside five tracks, then carousels of
/// artists, albums and playlists.
pub struct AllView {
    pub widget: gtk::Box,
    /// Top hit and tracks; vertical when the page is narrow.
    pub top_row: gtk::Box,
    pub top_hit: Option<gtk::Widget>,
    pub tracks: Option<TrackList>,
}

impl AllView {
    /// `show_all(filter)` switches the filter; `played()` is told when a
    /// result is played from here.
    pub fn new(
        ctx: &Rc<Ctx>,
        query: &str,
        results: &Results,
        show_all: Rc<dyn Fn(Filter)>,
        played: Rc<dyn Fn()>,
    ) -> Self {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(36)
            .margin_top(12)
            .margin_bottom(36)
            .build();
        let top_row = gtk::Box::builder().spacing(24).build();
        // Both titles get the height of the one with "View All".
        let titles = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);

        let top_hit = results.top_hit.as_ref().map(|item| {
            let column = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(12)
                .build();
            let header = title_row("Top Hit", None);
            titles.add_widget(&header);
            column.append(&header);
            column.append(&top_hit(ctx, item, played.clone()));
            top_row.append(&column);
            column.upcast::<gtk::Widget>()
        });

        let tracks = (!results.tracks.is_empty()).then(|| {
            let column = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(12)
                .hexpand(true)
                .build();
            let header = title_row("Tracks", None);
            titles.add_widget(&header);
            if results.total_tracks > SHORT_LIST {
                header.append(&show_all_button(Filter::Tracks, show_all.clone()));
            }
            column.append(&header);
            let list = TrackList::embedded(ctx, Variant::Cover);
            list.set_source(source(query));
            list.set_tracks(results.tracks.iter().take(SHORT_LIST).cloned().collect());
            let query = query.to_owned();
            play_on_activate(ctx, &list, move || source(&query), played.clone());
            column.append(list.widget());
            top_row.append(&column);
            list
        });
        if top_row.first_child().is_some() {
            widget.append(&top_row);
        }

        for (filter, items, total) in [
            (Filter::Artists, &results.artists, results.total_artists),
            (Filter::Albums, &results.albums, results.total_albums),
            (
                Filter::Playlists,
                &results.playlists,
                results.total_playlists,
            ),
        ] {
            if items.is_empty() {
                continue;
            }
            let carousel = CarouselSection::new(ctx, filter.title(), None, items);
            if total > items.len().min(MAX_CARDS) {
                let show_all = show_all.clone();
                carousel.connect_view_all(move || show_all(filter));
            }
            widget.append(carousel.widget());
        }

        Self {
            widget,
            top_row,
            top_hit,
            tracks,
        }
    }

    /// Stacks the top hit over the tracks when there is no room for both
    /// side by side.
    pub fn set_stacked(&self, stacked: bool) {
        self.top_row.set_orientation(if stacked {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        });
        self.top_row.set_spacing(if stacked { 36 } else { 24 });
        if let Some(top_hit) = &self.top_hit {
            // Alone, or above the tracks, it takes the whole width.
            let fixed = !stacked && self.tracks.is_some();
            top_hit.set_width_request(if fixed { TOP_HIT_WIDTH } else { -1 });
            top_hit.set_hexpand(!fixed);
        }
    }
}

/// "View All", which switches to `filter`.
fn show_all_button(filter: Filter, show_all: Rc<dyn Fn(Filter)>) -> gtk::Button {
    let button = gtk::Button::builder()
        .label("View All")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    button.connect_clicked(move |_| show_all(filter));
    button
}

/// Rows play the list from that track (as `source()`), and `played` is
/// told.
pub fn play_on_activate(
    ctx: &Rc<Ctx>,
    list: &TrackList,
    source: impl Fn() -> Source + 'static,
    played: Rc<dyn Fn()>,
) {
    let ctx = Rc::downgrade(ctx);
    let weak = list.downgrade();
    list.connect_activated(move |index| {
        let (Some(ctx), Some(list)) = (ctx.upgrade(), weak.upgrade()) else {
            return;
        };
        played();
        menus::play_tracks(&ctx, list.tracks(), index, source(), true);
    });
}

/// The top hit: a large button (120 px artwork, name, type) and a play
/// button over its corner, as siblings.
fn top_hit(ctx: &Rc<Ctx>, item: &Item, played: Rc<dyn Fn()>) -> gtk::Widget {
    let (name, kind) = describe(item);
    let content = gtk::Box::builder()
        .spacing(18)
        .valign(gtk::Align::Center)
        .build();
    let art: gtk::Widget = if let Item::Artist(_) = item {
        media_card::avatar(ctx, media_card::item_image(item), &name, TOP_HIT_SIZE).upcast()
    } else {
        let placeholder = if let Item::Playlist(_) = item {
            "view-list-symbolic"
        } else {
            "audio-x-generic-symbolic"
        };
        media_card::cover(
            ctx,
            media_card::item_image(item),
            TOP_HIT_SIZE,
            placeholder,
            true,
        )
    };
    art.set_valign(gtk::Align::Center);
    content.append(&art);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .valign(gtk::Align::Center)
        .hexpand(true)
        // Leaves room for the play button in the corner.
        .margin_end(48)
        .build();
    text.append(
        &gtk::Label::builder()
            .label(&name)
            .xalign(0.0)
            .wrap(true)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(1)
            .hexpand(true)
            .tooltip_text(&name)
            .css_classes(["title-2"])
            .build(),
    );
    text.append(
        &gtk::Label::builder()
            .label(&kind)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(1)
            .hexpand(true)
            .css_classes(["dimmed"])
            .build(),
    );
    content.append(&text);

    let button = gtk::Button::builder()
        .child(&content)
        .vexpand(true)
        .css_classes(["card"])
        .build();
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    button.update_property(&[gtk::accessible::Property::Label(&format!(
        "Top hit: {name}, {kind}"
    ))]);
    let weak = Rc::downgrade(ctx);
    let open = item.clone();
    let opened = played.clone();
    button.connect_clicked(move |_| {
        if let Some(ctx) = weak.upgrade() {
            opened();
            activate_item(&ctx, &open);
        }
    });

    let widget = gtk::Overlay::builder().child(&button).build();
    if let Some(subject) = Subject::from_item(item) {
        let play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .width_request(48)
            .height_request(48)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_end(12)
            .margin_bottom(12)
            .css_classes(["circular", "suggested-action"])
            .build();
        super::super::widgets::label_button(&play, &format!("Play {name}"));
        let weak = Rc::downgrade(ctx);
        let to_play = subject.clone();
        play.connect_clicked(move |_| {
            if let Some(ctx) = weak.upgrade() {
                played();
                menus::play(&ctx, &to_play, PlayMode::Play);
            }
        });
        widget.add_overlay(&play);
        let menu = ItemMenu::new(ctx, &widget);
        attach_menu(&button, &menu, subject);
    }
    widget.upcast()
}

/// Name and "Artist" / "Album · Artist"-style line of a top hit.
fn describe(item: &Item) -> (String, String) {
    let with = |kind: &str, artist: Option<&str>| match artist.filter(|a| !a.is_empty()) {
        Some(artist) => format!("{kind} · {artist}"),
        None => kind.to_owned(),
    };
    match item {
        Item::Artist(artist) => (artist.name.clone(), "Artist".into()),
        Item::Album(album) => (
            album.title.clone(),
            with("Album", album.artists.first().map(|a| a.name.as_str())),
        ),
        Item::Track(track) => (
            track.title.clone(),
            with("Track", track.artists.first().map(|a| a.name.as_str())),
        ),
        Item::Playlist(playlist) => (
            playlist.title.clone(),
            with("Playlist", playlist.creator.name.as_deref()),
        ),
        Item::Mix(mix) => (mix.title.clone(), "Mix".into()),
        Item::Link(link) => (link.title.clone(), String::new()),
    }
}

/// A virtualised grid of cards (`GridView.media-grid`) that asks for more
/// near its end, with a status bar over its bottom while paging.
pub struct CardGrid {
    pub widget: gtk::Overlay,
    store: gio::ListStore,
    view: gtk::GridView,
    scrolled: gtk::ScrolledWindow,
    paging: PagingStatus,
}

impl CardGrid {
    pub fn new(ctx: &Rc<Ctx>, near_end: impl Fn() + 'static) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let factory = gtk::SignalListItemFactory::new();
        let view = gtk::GridView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .factory(&factory)
            .min_columns(2)
            .max_columns(12)
            .tab_behavior(gtk::ListTabBehavior::Item)
            .css_classes(["media-grid"])
            .margin_top(6)
            .margin_bottom(24)
            .build();
        let menu = ItemMenu::new(ctx, &view);
        let weak_ctx: Weak<Ctx> = Rc::downgrade(ctx);
        let weak_menu = Rc::downgrade(&menu);
        factory.connect_bind(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(ctx), Some(boxed)) = (
                weak_ctx.upgrade(),
                list_item.item().and_downcast::<glib::BoxedAnyObject>(),
            ) else {
                return;
            };
            let item = boxed.borrow::<Item>();
            let menu = weak_menu.upgrade();
            let card = MediaCard::new(&ctx, &item, menu.as_ref());
            card.widget().set_halign(gtk::Align::Center);
            list_item.set_child(Some(card.widget()));
            list_item.set_activatable(false);
        });
        factory.connect_unbind(|_, object| {
            if let Some(list_item) = object.downcast_ref::<gtk::ListItem>() {
                list_item.set_child(None::<&gtk::Widget>);
            }
        });

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build();
        let near_end = Rc::new(near_end);
        let check = move |adjustment: &gtk::Adjustment| {
            let remaining = adjustment.upper() - adjustment.value() - adjustment.page_size();
            if adjustment.upper() > 0.0 && remaining < adjustment.page_size() * 2.0 {
                near_end();
            }
        };
        scrolled.vadjustment().connect_value_changed(check.clone());
        scrolled.vadjustment().connect_changed(check);

        let paging = PagingStatus::new();
        paging.widget.set_halign(gtk::Align::Center);
        paging.widget.set_valign(gtk::Align::End);
        paging.widget.set_margin_bottom(18);
        paging.widget.add_css_class("osd");
        paging.widget.add_css_class("toolbar");
        let widget = gtk::Overlay::builder().child(&scrolled).build();
        widget.add_overlay(&paging.widget);
        Self {
            widget,
            store,
            view,
            scrolled,
            paging,
        }
    }

    /// Replaces the cards and scrolls back to the top.
    pub fn set_items(&self, items: &[Item], label: &str) {
        self.view
            .update_property(&[gtk::accessible::Property::Label(label)]);
        let objects: Vec<glib::BoxedAnyObject> = items
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        self.store.splice(0, self.store.n_items(), &objects);
        self.scrolled.vadjustment().set_value(0.0);
    }

    pub fn append(&self, items: &[Item]) {
        let objects: Vec<glib::BoxedAnyObject> = items
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        self.store.splice(self.store.n_items(), 0, &objects);
    }

    pub fn paging(&self) -> &PagingStatus {
        &self.paging
    }

    pub fn set_compact(&self, compact: bool) {
        let margin = if compact { 6 } else { 18 };
        self.view.set_margin_start(margin);
        self.view.set_margin_end(margin);
    }
}

/// The end of a paged list: nothing, a spinner while the next page loads,
/// or "Couldn't load more results" with Retry.
pub struct PagingStatus {
    pub widget: gtk::Box,
    spinner: adw::Spinner,
    label: gtk::Label,
    retry: gtk::Button,
    retry_handler: Cell<Option<glib::SignalHandlerId>>,
}

impl PagingStatus {
    pub fn new() -> Self {
        let widget = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_top(12)
            .margin_bottom(12)
            .visible(false)
            .build();
        let spinner = adw::Spinner::builder()
            .width_request(24)
            .height_request(24)
            .build();
        spinner
            .upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Label("Loading more results")]);
        let label = gtk::Label::new(Some("Couldn't load more results"));
        let retry = gtk::Button::builder()
            .label("Retry")
            .css_classes(["pill"])
            .build();
        widget.append(&spinner);
        widget.append(&label);
        widget.append(&retry);
        Self {
            widget,
            spinner,
            label,
            retry,
            retry_handler: Cell::default(),
        }
    }

    pub fn hide(&self) {
        self.widget.set_visible(false);
    }

    pub fn show_loading(&self) {
        self.spinner.set_visible(true);
        self.label.set_visible(false);
        self.retry.set_visible(false);
        self.widget.set_visible(true);
    }

    pub fn show_failed(&self, retry: impl Fn() + 'static) {
        self.spinner.set_visible(false);
        self.label.set_visible(true);
        self.retry.set_visible(true);
        if let Some(old) = self.retry_handler.take() {
            self.retry.disconnect(old);
        }
        self.retry_handler
            .set(Some(self.retry.connect_clicked(move |_| retry())));
        self.widget.set_visible(true);
    }
}

/// The recent searches (a boxed list with "Clear" and ✕ per row), or the
/// "Search TIDAL" hint when there are none.
pub fn recent_view(
    recent: &[String],
    pick: Rc<dyn Fn(&str)>,
    forget: Rc<dyn Fn(&str)>,
    clear: Rc<dyn Fn()>,
) -> gtk::Widget {
    if recent.is_empty() {
        return adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .title("Search TIDAL")
            .description("Tracks, albums, artists and playlists")
            .vexpand(true)
            .build()
            .upcast();
    }
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(24)
        .margin_bottom(24)
        .build();
    let header = title_row("Recent Searches", None);
    let clear_button = gtk::Button::builder()
        .label("Clear")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    clear_button.connect_clicked(move |_| clear());
    header.append(&clear_button);
    widget.append(&header);

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    list.update_property(&[gtk::accessible::Property::Label("Recent Searches")]);
    for query in recent {
        let row = adw::ActionRow::builder()
            .title(query)
            .use_markup(false)
            .activatable(true)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("document-open-recent-symbolic"));
        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        super::super::widgets::label_button(&remove, &format!("Remove “{query}”"));
        let forget = forget.clone();
        let removed = query.clone();
        remove.connect_clicked(move |_| forget(&removed));
        row.add_suffix(&remove);
        let pick = pick.clone();
        let picked = query.clone();
        row.connect_activated(move |_| pick(&picked));
        list.append(&row);
    }
    widget.append(&list);
    let clamp = adw::Clamp::builder()
        .maximum_size(720)
        .child(&widget)
        .build();
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build()
        .upcast()
}
