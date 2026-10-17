//! The Search page (docs/design.md §3.4): find while typing.
//!
//! - The field searches 250 ms after the last keystroke (Enter at once;
//!   Escape empties it). Each search aborts the one in flight and answers
//!   carry a generation number, so a stale answer is never shown. The
//!   results on screen stay until the new ones arrive; a spinner appears in
//!   the header if that takes over 300 ms.
//! - Filters: All (top hit, five tracks, carousels of artists, albums and
//!   playlists, each with "View All" switching the filter) or one type,
//!   paged 50 at a time as the list scrolls: tracks in a playable
//!   [`TrackList`], the rest in a virtualised grid of cards.
//! - With an empty field: the recent searches (saved on disk), or a hint.
//!   A TIDAL link in the field offers to open it instead of searching.
//!
//! Developer aid: `SWELL_SEARCH_FILTER=tracks|albums|artists|playlists`
//! starts the page with that filter (for headless screenshots with
//! `SWELL_ROUTE=search:<query>`).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use super::super::context::Ctx;
use super::super::sidebar;
use super::super::widgets::section::activate_target;
use super::super::widgets::status::error_page;
use super::super::widgets::track_list::{TrackList, Variant, compact_bin};
use super::search_data::{self, Filter, Results};
use super::search_views::{self, AllView, CardGrid, PagingStatus};
use crate::runtime;
use crate::tidal;
use crate::tidal::pages::Target;

/// The header spinner appears only for answers slower than this.
const SPINNER_DELAY: Duration = Duration::from_millis(300);
/// Below this width the top hit sits above the tracks.
const STACK_TOP_HIT: &str = "max-width: 900px";

/// Pages of the results stack that show an answer (kept while the next
/// one loads).
const RESULT_PAGES: [&str; 4] = ["all", "tracks", "cards", "empty"];

#[derive(Default)]
struct Paging {
    /// Items loaded so far (the offset of the next page).
    offset: usize,
    total: usize,
    loading: bool,
    failed: bool,
}

struct SearchPage {
    ctx: Weak<Ctx>,
    entry: gtk::SearchEntry,
    filters: adw::ToggleGroup,
    filter_bar: gtk::Widget,
    filter_scrolled: gtk::ScrolledWindow,
    spinner: adw::Spinner,
    stack: gtk::Stack,
    all_scrolled: gtk::ScrolledWindow,
    all_clamp: adw::Clamp,
    all: RefCell<Option<AllView>>,
    tracks: TrackList,
    track_paging: PagingStatus,
    grid: CardGrid,
    empty: adw::StatusPage,
    link: adw::StatusPage,
    link_button: gtk::Button,
    link_target: RefCell<Option<Target>>,
    filter: Cell<Filter>,
    compact: Cell<bool>,
    stacked: Cell<bool>,
    /// Query and filter of the last search started or shown, so the
    /// delayed `search-changed` after Enter does not search again.
    requested: RefCell<Option<(String, Filter)>>,
    /// The query whose results are shown.
    shown: RefCell<String>,
    paging: RefCell<Paging>,
    generation: Cell<u64>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    more_task: RefCell<Option<glib::JoinHandle<()>>>,
    recent_task: RefCell<Option<glib::JoinHandle<()>>>,
    spinner_timer: RefCell<Option<glib::SourceId>>,
    network: RefCell<Option<glib::SignalHandlerId>>,
}

pub fn page(ctx: &Rc<Ctx>, query: Option<String>) -> adw::NavigationPage {
    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Search TIDAL")
        .search_delay(250)
        .hexpand(true)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label("Search TIDAL")]);
    let entry_clamp = adw::Clamp::builder()
        .maximum_size(520)
        .tightening_threshold(520)
        .hexpand(true)
        .child(&entry)
        .build();
    let spinner = adw::Spinner::builder()
        .width_request(16)
        .height_request(16)
        .visible(false)
        .build();
    spinner
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label("Searching")]);
    let header_bar = adw::HeaderBar::builder().title_widget(&entry_clamp).build();
    header_bar.pack_start(&sidebar::show_sidebar_button(&ctx.window));
    header_bar.pack_end(&sidebar::narrow_main_menu_button(&ctx.window));
    header_bar.pack_end(&spinner);

    let filters = adw::ToggleGroup::builder()
        .halign(gtk::Align::Start)
        .build();
    for filter in Filter::ALL {
        filters.add(
            adw::Toggle::builder()
                .name(filter.name())
                .label(filter.title())
                .build(),
        );
    }
    filters
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label("Show")]);
    let initial_filter = std::env::var("SWELL_SEARCH_FILTER")
        .ok()
        .and_then(|name| Filter::from_name(&name))
        .unwrap_or(Filter::All);
    filters.set_active_name(Some(initial_filter.name()));
    let filter_scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&filters)
        .build();
    let filter_bar = adw::Clamp::builder()
        .maximum_size(1200)
        .tightening_threshold(1200)
        .child(&filter_scrolled)
        .visible(false)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(200)
        .vexpand(true)
        .hexpand(true)
        .build();
    stack.add_named(
        &gtk::Box::new(gtk::Orientation::Vertical, 0),
        Some("recent"),
    );
    let big_spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    big_spinner
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label("Searching")]);
    stack.add_named(&big_spinner, Some("loading"));

    let all_clamp = adw::Clamp::builder()
        .maximum_size(1200)
        .tightening_threshold(1200)
        .build();
    let all_scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&all_clamp)
        .build();
    let all_bin = adw::BreakpointBin::builder()
        .width_request(360)
        .height_request(200)
        .child(&all_scrolled)
        .build();
    stack.add_named(&all_bin, Some("all"));

    let tracks = TrackList::new(ctx, Variant::Cover);
    stack.add_named(tracks.widget(), Some("tracks"));

    let empty = adw::StatusPage::builder()
        .icon_name("system-search-symbolic")
        .css_classes(["compact"])
        .vexpand(true)
        .build();
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&gtk::Box::new(gtk::Orientation::Vertical, 0), Some("error"));

    let retry = gtk::Button::builder()
        .label("Retry")
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    let offline = adw::StatusPage::builder()
        .icon_name("network-offline-symbolic")
        .title("You're Offline")
        .description("Searching needs an internet connection.")
        .child(&retry)
        .vexpand(true)
        .build();
    stack.add_named(&offline, Some("offline"));

    let link_button = gtk::Button::builder()
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    let link = adw::StatusPage::builder()
        .title("TIDAL Link")
        .child(&link_button)
        .vexpand(true)
        .build();
    stack.add_named(&link, Some("link"));

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&filter_bar);
    content.append(&stack);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header_bar);
    let page = adw::NavigationPage::builder()
        .title("Search")
        .child(&toolbar)
        .build();

    let state = Rc::new_cyclic(|weak: &Weak<SearchPage>| {
        let grid = CardGrid::new(ctx, {
            let weak = weak.clone();
            move || {
                if let Some(state) = weak.upgrade() {
                    state.load_more();
                }
            }
        });
        stack.add_named(&grid.widget, Some("cards"));
        SearchPage {
            ctx: Rc::downgrade(ctx),
            entry: entry.clone(),
            filters: filters.clone(),
            filter_bar: filter_bar.clone().upcast(),
            filter_scrolled,
            spinner,
            stack: stack.clone(),
            all_scrolled,
            all_clamp,
            all: RefCell::default(),
            tracks: tracks.clone(),
            track_paging: PagingStatus::new(),
            grid,
            empty,
            link,
            link_button: link_button.clone(),
            link_target: RefCell::default(),
            filter: Cell::new(initial_filter),
            compact: Cell::new(false),
            stacked: Cell::new(false),
            requested: RefCell::default(),
            shown: RefCell::default(),
            paging: RefCell::default(),
            generation: Cell::new(0),
            task: RefCell::default(),
            more_task: RefCell::default(),
            recent_task: RefCell::default(),
            spinner_timer: RefCell::default(),
            network: RefCell::default(),
        }
    });

    let weak = Rc::downgrade(&state);
    let bin = compact_bin(&content, move |compact| {
        if let Some(state) = weak.upgrade() {
            state.set_compact(compact);
        }
    });
    toolbar.set_content(Some(&bin));
    state.set_compact(false);

    let breakpoint = adw::Breakpoint::new(
        adw::BreakpointCondition::parse(STACK_TOP_HIT).expect("valid breakpoint condition"),
    );
    for stacked in [true, false] {
        let weak = Rc::downgrade(&state);
        let apply = move |_: &adw::Breakpoint| {
            if let Some(state) = weak.upgrade() {
                state.set_stacked(stacked);
            }
        };
        if stacked {
            breakpoint.connect_apply(apply);
        } else {
            breakpoint.connect_unapply(apply);
        }
    }
    all_bin.add_breakpoint(breakpoint);

    state.connect_signals(ctx, &page, &retry);

    if let Some(query) = query.filter(|q| !q.trim().is_empty()) {
        entry.set_text(&query);
        entry.set_position(-1);
        state.run(false);
    } else {
        state.show_recent();
    }

    // Typing anywhere on the page goes to the field.
    entry.set_key_capture_widget(Some(&toolbar));
    entry.connect_map(|entry| {
        let entry = entry.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(entry) = entry.upgrade()
                && !entry.has_focus()
            {
                entry.grab_focus();
                entry.set_position(-1);
            }
        });
    });

    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(state) = owned.take() {
            state.cancel();
            if let Some(task) = state.recent_task.take() {
                task.abort();
            }
            if let Some(handler) = state.network.take() {
                gio::NetworkMonitor::default().disconnect(handler);
            }
        }
    });
    page
}

impl SearchPage {
    fn connect_signals(
        self: &Rc<Self>,
        ctx: &Rc<Ctx>,
        page: &adw::NavigationPage,
        retry: &gtk::Button,
    ) {
        let weak = Rc::downgrade(self);
        self.entry.connect_search_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                state.run(false);
            }
        });
        let weak = Rc::downgrade(self);
        self.entry.connect_activate(move |entry| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            if Target::from_url(entry.text().trim()).is_some() {
                return state.open_link();
            }
            search_data::remember(entry.text().as_str());
            state.run(false);
        });
        self.entry.connect_stop_search(|entry| {
            if !entry.text().is_empty() {
                entry.set_text("");
            }
        });

        let weak = Rc::downgrade(self);
        self.filters.connect_active_name_notify(move |filters| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let filter = filters
                .active_name()
                .and_then(|name| Filter::from_name(&name))
                .unwrap_or(Filter::All);
            if state.filter.replace(filter) != filter {
                state.run(false);
            }
        });

        let weak = Rc::downgrade(self);
        self.tracks.connect_near_end(move || {
            if let Some(state) = weak.upgrade() {
                state.load_more();
            }
        });
        let weak = Rc::downgrade(self);
        search_views::play_on_activate(
            ctx,
            &self.tracks,
            move || {
                let query = weak.upgrade().map(|s| s.shown.borrow().clone());
                search_views::source(&query.unwrap_or_default())
            },
            self.played_hook(),
        );

        let weak = Rc::downgrade(self);
        self.link_button.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.open_link();
            }
        });
        let weak = Rc::downgrade(self);
        retry.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.run(true);
            }
        });
        let weak = Rc::downgrade(self);
        let handler =
            gio::NetworkMonitor::default().connect_network_changed(move |_, available| {
                if let Some(state) = weak.upgrade()
                    && available
                    && state.visible() == "offline"
                {
                    state.run(true);
                }
            });
        self.network.replace(Some(handler));

        // Opening a result remembers the search.
        let weak = Rc::downgrade(self);
        page.connect_hidden(move |_| {
            if let Some(state) = weak.upgrade() {
                state.remember_shown();
            }
        });
    }

    /// Called when a result is played or opened from the page.
    fn played_hook(self: &Rc<Self>) -> Rc<dyn Fn()> {
        let weak = Rc::downgrade(self);
        Rc::new(move || {
            if let Some(state) = weak.upgrade() {
                state.remember_shown();
            }
        })
    }

    fn remember_shown(&self) {
        if RESULT_PAGES[..3].contains(&self.visible().as_str()) {
            search_data::remember(&self.shown.borrow());
        }
    }

    fn visible(&self) -> String {
        self.stack
            .visible_child_name()
            .map(Into::into)
            .unwrap_or_default()
    }

    /// Searches for what the field holds (unless that is what is already
    /// shown or loading, or `force`).
    fn run(self: &Rc<Self>, force: bool) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let query = self.entry.text().trim().to_owned();
        let filter = self.filter.get();
        if query.is_empty() {
            self.cancel();
            self.requested.take();
            return self.show_recent();
        }
        if let Some(target) = Target::from_url(&query) {
            self.cancel();
            self.requested.take();
            return self.show_link(target);
        }
        let key = (query.clone(), filter);
        if !force && self.requested.borrow().as_ref() == Some(&key) {
            return;
        }
        self.requested.replace(Some(key));
        self.cancel();
        if !force && let Some(results) = search_data::cached(&query, filter) {
            return self.show(&query, filter, &results);
        }

        let generation = self.generation.get();
        if RESULT_PAGES.contains(&self.visible().as_str()) {
            self.start_spinner();
        } else {
            self.filter_bar.set_visible(true);
            self.stack.set_visible_child_name("loading");
        }
        let api = ctx.api.clone();
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let answer = runtime::spawn(search_data::fetch(api, query.clone(), filter, 0)).await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            if state.generation.get() != generation {
                return;
            }
            state.stop_spinner();
            match answer {
                Ok(results) => {
                    let results = Rc::new(results);
                    search_data::remember_answer(&query, filter, results.clone());
                    state.show(&query, filter, &results);
                }
                Err(err) => state.show_error(&err),
            }
        });
        self.task.replace(Some(task));
    }

    /// Stops the search and paging in flight; their answers are ignored.
    fn cancel(&self) {
        self.generation.set(self.generation.get() + 1);
        for task in [self.task.take(), self.more_task.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
        }
        self.paging.borrow_mut().loading = false;
        self.stop_spinner();
    }

    fn start_spinner(self: &Rc<Self>) {
        self.stop_spinner();
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(SPINNER_DELAY, move || {
            if let Some(state) = weak.upgrade() {
                state.spinner_timer.take();
                state.spinner.set_visible(true);
            }
        });
        self.spinner_timer.replace(Some(timer));
    }

    fn stop_spinner(&self) {
        if let Some(timer) = self.spinner_timer.take() {
            timer.remove();
        }
        self.spinner.set_visible(false);
    }

    fn show(self: &Rc<Self>, query: &str, filter: Filter, results: &Results) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.shown.replace(query.to_owned());
        self.filter_bar.set_visible(true);
        self.reset_paging(filter, results);
        if results.is_empty(filter) {
            self.empty.set_title(&format!("No Results for “{query}”"));
            self.empty.set_description(Some(if filter == Filter::All {
                "Check the spelling or try other words."
            } else {
                "Try another filter or other words."
            }));
            self.stack.set_visible_child_name("empty");
            return;
        }
        match filter {
            Filter::All => {
                let weak = Rc::downgrade(self);
                let show_all: Rc<dyn Fn(Filter)> = Rc::new(move |filter| {
                    if let Some(state) = weak.upgrade() {
                        search_data::remember(&state.shown.borrow());
                        state.filters.set_active_name(Some(filter.name()));
                        state.filter_scrolled.hadjustment().set_value(0.0);
                    }
                });
                let view = AllView::new(&ctx, query, results, show_all, self.played_hook());
                view.set_stacked(self.stacked.get());
                if let Some(list) = &view.tracks {
                    list.set_compact(self.compact.get());
                }
                self.all_clamp.set_child(Some(&view.widget));
                self.all.replace(Some(view));
                self.set_margins();
                self.all_scrolled.vadjustment().set_value(0.0);
                self.stack.set_visible_child_name("all");
            }
            Filter::Tracks => {
                self.tracks.set_source(search_views::source(query));
                self.tracks.set_tracks(results.tracks.clone());
                self.tracks.scroll_to_top();
                self.stack.set_visible_child_name("tracks");
            }
            cards => {
                self.grid.set_items(results.cards(cards), cards.title());
                self.stack.set_visible_child_name("cards");
            }
        }
    }

    fn reset_paging(&self, filter: Filter, results: &Results) {
        self.paging.replace(Paging {
            offset: results.len(filter),
            total: results.total(filter),
            loading: false,
            failed: false,
        });
        self.paging_done(filter);
    }

    /// Loads the next page of a single-type filter.
    fn load_more(self: &Rc<Self>) {
        let filter = self.filter.get();
        let page = match filter {
            Filter::All => return,
            Filter::Tracks => "tracks",
            _ => "cards",
        };
        if self.visible() != page {
            return;
        }
        let offset = {
            let paging = self.paging.borrow();
            if paging.loading || paging.failed || paging.offset >= paging.total {
                return;
            }
            paging.offset
        };
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.paging.borrow_mut().loading = true;
        self.paging_loading(filter);
        let generation = self.generation.get();
        let query = self.shown.borrow().clone();
        let api = ctx.api.clone();
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let answer = runtime::spawn(search_data::fetch(api, query, filter, offset)).await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            if state.generation.get() != generation {
                return;
            }
            state.paging.borrow_mut().loading = false;
            match answer {
                Ok(results) => {
                    let count = results.len(filter);
                    {
                        let mut paging = state.paging.borrow_mut();
                        if count == 0 {
                            // TIDAL counted more than it gives.
                            paging.total = paging.offset;
                        }
                        paging.offset += count;
                    }
                    if filter == Filter::Tracks {
                        state.tracks.append(results.tracks);
                    } else {
                        state.grid.append(results.cards(filter));
                    }
                    state.paging_done(filter);
                }
                Err(err) => {
                    if tidal::is_auth_error(&err)
                        && let Some(ctx) = state.ctx.upgrade()
                    {
                        ctx.report("Couldn't load more results", &err);
                    }
                    state.paging.borrow_mut().failed = true;
                    let weak = Rc::downgrade(&state);
                    state.paging_failed(filter, move || {
                        if let Some(state) = weak.upgrade() {
                            state.paging.borrow_mut().failed = false;
                            state.load_more();
                        }
                    });
                }
            }
        });
        if let Some(old) = self.more_task.replace(Some(task)) {
            old.abort();
        }
    }

    fn paging_loading(&self, filter: Filter) {
        if filter == Filter::Tracks {
            self.track_paging.show_loading();
            self.tracks.set_footer(Some(&self.track_paging.widget));
        } else {
            self.grid.paging().show_loading();
        }
    }

    fn paging_failed(&self, filter: Filter, retry: impl Fn() + 'static) {
        if filter == Filter::Tracks {
            self.track_paging.show_failed(retry);
            self.tracks.set_footer(Some(&self.track_paging.widget));
        } else {
            self.grid.paging().show_failed(retry);
        }
    }

    fn paging_done(&self, filter: Filter) {
        if filter == Filter::Tracks {
            self.track_paging.hide();
            self.tracks.set_footer(None::<&gtk::Widget>);
        } else {
            self.grid.paging().hide();
        }
    }

    fn show_error(self: &Rc<Self>, err: &anyhow::Error) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        // Searching the same words again (Enter) retries.
        self.requested.take();
        if tidal::is_auth_error(err) {
            ctx.report("Couldn't search", err);
        }
        if is_offline(err) {
            self.stack.set_visible_child_name("offline");
            return;
        }
        let weak = Rc::downgrade(self);
        let status = error_page(err, move || {
            if let Some(state) = weak.upgrade() {
                state.run(true);
            }
        });
        if let Some(old) = self.stack.child_by_name("error") {
            self.stack.remove(&old);
        }
        self.stack.add_named(&status, Some("error"));
        self.stack.set_visible_child_name("error");
    }

    /// The recent searches (read from disk the first time).
    fn show_recent(self: &Rc<Self>) {
        self.filter_bar.set_visible(false);
        self.link_target.take();
        if let Some(recent) = search_data::recent_now() {
            return self.render_recent(&recent);
        }
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let recent = search_data::recent().await;
            if let Some(state) = weak.upgrade()
                && state.entry.text().trim().is_empty()
            {
                state.render_recent(&recent);
            }
        });
        if let Some(old) = self.recent_task.replace(Some(task)) {
            old.abort();
        }
    }

    fn render_recent(self: &Rc<Self>, recent: &[String]) {
        let weak = Rc::downgrade(self);
        let pick: Rc<dyn Fn(&str)> = Rc::new(move |query| {
            if let Some(state) = weak.upgrade() {
                search_data::remember(query);
                state.entry.set_text(query);
                state.entry.set_position(-1);
                state.entry.grab_focus();
                state.entry.set_position(-1);
                state.run(false);
            }
        });
        let weak = Rc::downgrade(self);
        let forget: Rc<dyn Fn(&str)> = Rc::new(move |query| {
            search_data::forget(query);
            if let Some(state) = weak.upgrade() {
                state.show_recent();
            }
        });
        let weak = Rc::downgrade(self);
        let clear: Rc<dyn Fn()> = Rc::new(move || {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let before = search_data::recent_now().unwrap_or_default();
            search_data::clear_recent();
            state.show_recent();
            if let Some(ctx) = state.ctx.upgrade() {
                let weak = weak.clone();
                ctx.toast_with_undo("Recent searches cleared", move || {
                    search_data::set_recent(before.clone());
                    if let Some(state) = weak.upgrade()
                        && state.visible() == "recent"
                    {
                        state.show_recent();
                    }
                });
            }
        });
        let view = search_views::recent_view(recent, pick, forget, clear);
        if self.compact.get() {
            view.set_margin_start(12);
            view.set_margin_end(12);
        } else {
            view.set_margin_start(24);
            view.set_margin_end(24);
        }
        if let Some(old) = self.stack.child_by_name("recent") {
            self.stack.remove(&old);
        }
        self.stack.add_named(&view, Some("recent"));
        self.stack.set_visible_child_name("recent");
    }

    /// Offers to open the TIDAL link in the field.
    fn show_link(&self, target: Target) {
        self.filter_bar.set_visible(false);
        let (icon, description, label) = match &target {
            Target::Album(_) => (
                "media-optical-symbolic",
                "This is a link to an album.",
                "Open Album",
            ),
            Target::Artist(_) => (
                "avatar-default-symbolic",
                "This is a link to an artist.",
                "Open Artist",
            ),
            Target::Playlist(_) => (
                "view-list-symbolic",
                "This is a link to a playlist.",
                "Open Playlist",
            ),
            Target::Mix(_) => ("view-list-symbolic", "This is a link to a mix.", "Open Mix"),
            Target::Track(_) => (
                "audio-x-generic-symbolic",
                "This is a link to a track.",
                "Play Track",
            ),
            Target::Collection(_) => (
                "folder-music-symbolic",
                "This is a link to your collection.",
                "Open Collection",
            ),
            Target::Page(_) | Target::Url(_) => (
                "folder-music-symbolic",
                "This is a link to a TIDAL page.",
                "Open Link",
            ),
        };
        self.link.set_icon_name(Some(icon));
        self.link.set_description(Some(description));
        self.link_button.set_label(label);
        self.link_target.replace(Some(target));
        self.stack.set_visible_child_name("link");
    }

    fn open_link(&self) {
        let (Some(ctx), Some(target)) = (self.ctx.upgrade(), self.link_target.borrow().clone())
        else {
            return;
        };
        activate_target(&ctx, &target);
    }

    fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        self.tracks.set_compact(compact);
        self.grid.set_compact(compact);
        if let Some(list) = self.all.borrow().as_ref().and_then(|a| a.tracks.as_ref()) {
            list.set_compact(compact);
        }
        if let Some(recent) = self.stack.child_by_name("recent") {
            let margin = if compact { 12 } else { 24 };
            recent.set_margin_start(margin);
            recent.set_margin_end(margin);
        }
        self.set_margins();
    }

    fn set_margins(&self) {
        let margin = if self.compact.get() { 12 } else { 24 };
        self.filter_scrolled.set_margin_start(margin);
        self.filter_scrolled.set_margin_end(margin);
        if let Some(all) = self.all.borrow().as_ref() {
            all.widget.set_margin_start(margin);
            all.widget.set_margin_end(margin);
        }
    }

    fn set_stacked(&self, stacked: bool) {
        self.stacked.set(stacked);
        if let Some(all) = self.all.borrow().as_ref() {
            all.set_stacked(stacked);
        }
    }
}

/// Whether `err` comes from having no network.
fn is_offline(err: &anyhow::Error) -> bool {
    !gio::NetworkMonitor::default().is_network_available()
        || err.chain().any(|cause| {
            cause
                .downcast_ref::<reqwest::Error>()
                .is_some_and(|e| e.is_connect())
        })
}
