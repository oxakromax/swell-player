//! "View all" (docs/design.md §3.11): everything in one section.
//!
//! Where the rest comes from depends on the section ([`More`]):
//!
//! - `Data`: a v1 `pages/data/...` list, fetched [`PAGE_SIZE`] items at a
//!   time with `offset`/`limit` while the end is less than two screens
//!   away;
//! - `ViewAll`: a v2 home "view all" page (one answer);
//! - `Page`: a v1 page; with a single module it is shown as that module's
//!   list (paged through its `dataApiPath`), otherwise as sections.
//!
//! Tracks become a full, playable [`TrackList`]; cards a virtualised
//! `GridView` (`media-grid`) whose column count follows the width; chips
//! wrap. The first answer is cached (an hour) and revalidated.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use super::super::context::Ctx;
use super::super::menus::ItemMenu;
use super::super::widgets::media_card::MediaCard;
use super::super::widgets::section;
use super::super::widgets::status::{PageStack, describe_error, empty_status};
use super::super::widgets::track_list::{TrackList, Variant, compact_bin};
use super::home_sections::{self, loading_more, paging_error};
use crate::model::TrackInfo;
use crate::playback::Source;
use crate::runtime;
use crate::tidal::pages::{self, Item, More, Page, Section, SectionKind};
use crate::tidal::{self, Base, Call};

const FRESH_FOR: Duration = Duration::from_secs(60 * 60);
/// Items per request when paging a data list.
const PAGE_SIZE: usize = 50;

/// The first answer, whatever the kind of [`More`].
enum Loaded {
    Section(Section),
    /// A page of a data list, with the number of items TIDAL sent (some
    /// may not have been understood, so `items` can have fewer).
    Data(Section, usize),
    Page(Page),
}

/// Items TIDAL sent in a page of a data list, understood or not: offsets
/// count these.
fn sent_items(value: &serde_json::Value) -> usize {
    value
        .get("items")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len)
}

fn parse_data(value: &serde_json::Value) -> Loaded {
    Loaded::Data(pages::parse_paged_list(value), sent_items(value))
}

/// What the page shows once loaded.
enum Body {
    Empty,
    Tracks(TrackList),
    Grid(gio::ListStore),
    /// Chips or several sections in a scrolled column.
    Column,
}

/// The rest of a data list.
#[derive(Clone)]
struct Pager {
    path: String,
    /// Items TIDAL has sent so far (the next page's offset).
    offset: usize,
    total: usize,
    /// TIDAL sent a short page: nothing follows, whatever the total says.
    ended: bool,
    /// The next page replaces what is shown (a page module's own items,
    /// which may not match the data list's offsets).
    replace: bool,
}

impl Pager {
    /// After the first page of `path`: `sent` items of `total`.
    fn after_first(path: String, sent: usize, total: usize) -> Self {
        Self {
            path,
            offset: sent,
            total,
            ended: sent < PAGE_SIZE,
            replace: false,
        }
    }

    fn has_more(&self) -> bool {
        !self.ended && self.offset < self.total
    }

    /// After a page of `sent` items (`PAGE_SIZE` were asked for) that says
    /// the list has `total`.
    fn advance(&mut self, sent: usize, total: usize) {
        self.offset += sent;
        self.total = self.total.max(total);
        self.ended = sent < PAGE_SIZE;
        self.replace = false;
    }
}

struct MorePage {
    ctx: Weak<Ctx>,
    more: More,
    /// The route's title; empty: the section's.
    title: String,
    stack: Rc<PageStack>,
    window_title: adw::WindowTitle,
    page: glib::WeakRef<adw::NavigationPage>,
    /// Over the grid: the "loading more" spinner or a paging error.
    grid_footer: gtk::Box,
    body: RefCell<Body>,
    pager: RefCell<Option<Pager>>,
    loading_more: Cell<bool>,
    more_failed: Cell<bool>,
    compact: Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    more_task: RefCell<Option<glib::JoinHandle<()>>>,
}

pub fn page(ctx: &Rc<Ctx>, title: String, more: More) -> adw::NavigationPage {
    let spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    spinner
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label("Loading")]);
    let stack = PageStack::new(&spinner);

    let window_title = adw::WindowTitle::new(&title, "");
    let header_bar = adw::HeaderBar::builder()
        .title_widget(&window_title)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header_bar);
    let page = adw::NavigationPage::builder()
        .title(if title.is_empty() { "View All" } else { &title })
        .child(&toolbar)
        .build();

    let grid_footer = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .margin_bottom(18)
        .visible(false)
        .css_classes(["osd", "toolbar"])
        .build();

    let state = Rc::new(MorePage {
        ctx: Rc::downgrade(ctx),
        more,
        title,
        stack: stack.clone(),
        window_title,
        page: page.downgrade(),
        grid_footer,
        body: RefCell::new(Body::Empty),
        pager: RefCell::default(),
        loading_more: Cell::new(false),
        more_failed: Cell::new(false),
        compact: Cell::new(false),
        task: RefCell::default(),
        more_task: RefCell::default(),
    });

    let weak = Rc::downgrade(&state);
    let bin = compact_bin(stack.widget(), move |compact| {
        if let Some(state) = weak.upgrade() {
            state.set_compact(compact);
        }
    });
    toolbar.set_content(Some(&bin));

    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_scope(gtk::ShortcutScope::Managed);
    let weak = Rc::downgrade(&state);
    shortcuts.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("F5"),
        Some(gtk::CallbackAction::new(move |_, _| {
            if let Some(state) = weak.upgrade() {
                state.load(Duration::ZERO);
            }
            glib::Propagation::Stop
        })),
    ));
    toolbar.add_controller(shortcuts);

    state.load(FRESH_FOR);
    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(state) = owned.take() {
            state.abort();
        }
    });
    page
}

/// `path` with `query` appended (paths may already have a query).
fn with_query(path: &str, query: &str) -> String {
    let separator = if path.contains('?') { '&' } else { '?' };
    format!("{path}{separator}{query}")
}

impl MorePage {
    fn abort(&self) {
        for task in [self.task.take(), self.more_task.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
        }
        self.loading_more.set(false);
    }

    fn load(self: &Rc<Self>, fresh_for: Duration) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.abort();
        if !self.stack.is_showing_content() {
            self.stack.show_loading();
        }
        let mut answers = match &self.more {
            More::Data(path) => ctx.api.load_cached(
                Base::V1,
                with_query(path, &format!("offset=0&limit={PAGE_SIZE}")),
                fresh_for,
                parse_data,
            ),
            More::ViewAll(path) => ctx.api.load_cached(
                Base::V2,
                with_query(path, "platform=WEB"),
                fresh_for,
                |value| Loaded::Section(pages::parse_view_all(value)),
            ),
            More::Page(path) => ctx
                .api
                .load_cached(Base::V1, path.clone(), fresh_for, |value| {
                    Loaded::Page(pages::parse_v1(value))
                }),
        };
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            while let Some(answer) = answers.recv().await {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                match answer {
                    Ok(Loaded::Section(section)) => state.show_section(section, None),
                    Ok(Loaded::Data(section, sent)) => {
                        let pager = match &state.more {
                            More::Data(path) => {
                                Some(Pager::after_first(path.clone(), sent, section.total))
                            }
                            _ => None,
                        };
                        state.show_section(section, pager);
                    }
                    Ok(Loaded::Page(page)) => state.show_page(page),
                    Err(err) => state.show_error(&err),
                }
            }
        });
        self.task.replace(Some(task));
    }

    fn show_error(self: &Rc<Self>, err: &anyhow::Error) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if tidal::is_auth_error(err) {
            ctx.report("Couldn't load the page", err);
        }
        if self.stack.is_showing_content() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.stack.show_error(err, move || {
            if let Some(state) = weak.upgrade() {
                state.load(FRESH_FOR);
            }
        });
    }

    fn set_title(&self, fallback: &str) {
        if !self.title.is_empty() || fallback.is_empty() {
            return;
        }
        self.window_title.set_title(fallback);
        if let Some(page) = self.page.upgrade() {
            page.set_title(fallback);
        }
    }

    /// A v1 page: its only module as a list, or all of them as sections.
    fn show_page(self: &Rc<Self>, page: Page) {
        let sections: Vec<Section> = page
            .sections
            .into_iter()
            .filter(|s| !s.items.is_empty())
            .collect();
        if let [section] = sections.as_slice() {
            // The module shows its first items, but how many TIDAL sent for
            // it is unknown here: page its list from the start, replacing
            // them with the first page.
            let pager = match &section.more {
                Some(More::Data(path)) if section.total > section.items.len() => Some(Pager {
                    path: path.clone(),
                    offset: 0,
                    total: section.total,
                    ended: false,
                    replace: true,
                }),
                _ => None,
            };
            let title = if section.title.is_empty() {
                page.title
            } else {
                section.title.clone()
            };
            self.set_title(&title);
            let paged = pager.is_some();
            self.show_section(section.clone(), pager);
            if paged {
                self.load_more();
            }
            return;
        }
        self.set_title(&page.title);
        self.show_column(&sections);
    }

    fn show_section(self: &Rc<Self>, section: Section, pager: Option<Pager>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.more_task.take() {
            task.abort();
        }
        self.loading_more.set(false);
        self.more_failed.set(false);
        self.set_title(&section.title);
        self.pager.replace(pager);
        if section.items.is_empty() {
            self.body.replace(Body::Empty);
            self.stack.show_empty(
                "folder-music-symbolic",
                "Nothing Here Yet",
                "TIDAL has nothing in this list for now.",
            );
            return;
        }
        match section.kind {
            SectionKind::Tracks => self.show_tracks(&ctx, &section),
            SectionKind::Links => self.show_column(std::slice::from_ref(&Section {
                title: String::new(),
                subtitle: None,
                more: None,
                ..section
            })),
            _ => self.show_grid(&ctx, section.items),
        }
        self.set_footer(None);
        self.stack.show_content();
    }

    fn show_tracks(self: &Rc<Self>, ctx: &Rc<Ctx>, section: &Section) {
        let tracks = tracks_of(&section.items);
        if let Body::Tracks(list) = &*self.body.borrow() {
            list.set_tracks(tracks);
            return;
        }
        let list = TrackList::new(ctx, Variant::Cover);
        list.set_source(Source {
            title: if self.title.is_empty() {
                section.title.clone()
            } else {
                self.title.clone()
            },
            target: None,
        });
        list.set_tracks(tracks);
        list.set_compact(self.compact.get());
        if let Some(view) = list.view() {
            view.set_margin_top(6);
            view.set_margin_bottom(18);
        }
        let weak = Rc::downgrade(self);
        list.connect_near_end(move || {
            if let Some(state) = weak.upgrade() {
                state.load_more();
            }
        });
        self.stack.set_content(list.widget());
        self.body.replace(Body::Tracks(list));
    }

    fn show_grid(self: &Rc<Self>, ctx: &Rc<Ctx>, items: Vec<Item>) {
        let items: Vec<_> = items.into_iter().map(glib::BoxedAnyObject::new).collect();
        if let Body::Grid(store) = &*self.body.borrow() {
            store.remove_all();
            store.extend_from_slice(&items);
            return;
        }
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        store.extend_from_slice(&items);
        let factory = gtk::SignalListItemFactory::new();
        let grid = gtk::GridView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .factory(&factory)
            .min_columns(1)
            .max_columns(16)
            .margin_top(6)
            .margin_bottom(18)
            .css_classes(["media-grid"])
            .build();
        grid.update_property(&[gtk::accessible::Property::Label(&self.window_title.title())]);
        let menu = Rc::downgrade(&ItemMenu::new(ctx, &grid));
        let weak_ctx = Rc::downgrade(ctx);
        factory.connect_bind(move |_, object| {
            let Some(cell) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(ctx), Some(entry)) = (
                weak_ctx.upgrade(),
                cell.item().and_downcast::<glib::BoxedAnyObject>(),
            ) else {
                return;
            };
            let menu = menu.upgrade();
            let card = MediaCard::new(&ctx, &entry.borrow::<Item>(), menu.as_ref());
            card.widget().set_halign(gtk::Align::Start);
            cell.set_child(Some(card.widget()));
        });
        factory.connect_unbind(|_, object| {
            if let Some(cell) = object.downcast_ref::<gtk::ListItem>() {
                cell.set_child(None::<&gtk::Widget>);
            }
        });

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&grid)
            .build();
        // Ask for the next page while the end is less than two screens away.
        let weak = Rc::downgrade(self);
        let check = move |adjustment: &gtk::Adjustment| {
            let left = adjustment.upper() - adjustment.value() - adjustment.page_size();
            if adjustment.page_size() > 0.0
                && left < 2.0 * adjustment.page_size()
                && let Some(state) = weak.upgrade()
            {
                state.load_more();
            }
        };
        scrolled.vadjustment().connect_value_changed(check.clone());
        scrolled.vadjustment().connect_changed(check);

        let overlay = gtk::Overlay::builder().child(&scrolled).build();
        if let Some(parent) = self.grid_footer.parent()
            && let Some(old) = parent.downcast_ref::<gtk::Overlay>()
        {
            old.remove_overlay(&self.grid_footer);
        }
        overlay.add_overlay(&self.grid_footer);
        section::align_to_edge(&grid, section::GRID_INSET);
        self.set_compact(self.compact.get());
        self.stack.set_content(&overlay);
        self.body.replace(Body::Grid(store));
    }

    /// Sections (or chips) in a scrolled column.
    fn show_column(self: &Rc<Self>, sections: &[Section]) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(36)
            .margin_top(12)
            .margin_bottom(36)
            .build();
        for section in sections {
            if let Some(widget) = home_sections::section_widget(&ctx, section) {
                column.append(&widget);
            }
        }
        if column.first_child().is_none() {
            column.append(&empty_status(
                "folder-music-symbolic",
                "Nothing Here Yet",
                "TIDAL has nothing on this page in your country.",
            ));
        }
        let clamp = adw::Clamp::builder()
            .maximum_size(1600)
            .tightening_threshold(1600)
            .child(&column)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        self.set_compact(self.compact.get());
        self.pager.replace(None);
        self.stack.set_content(&scrolled);
        self.body.replace(Body::Column);
        self.stack.show_content();
    }

    /// Shows `widget` after the items (`None` removes it).
    fn set_footer(&self, widget: Option<&gtk::Widget>) {
        match &*self.body.borrow() {
            Body::Tracks(list) => {
                let footer = widget.map(|widget| {
                    let footer = gtk::Box::builder()
                        .halign(gtk::Align::Center)
                        .margin_top(12)
                        .margin_bottom(12)
                        .build();
                    footer.append(widget);
                    footer
                });
                list.set_footer(footer.as_ref());
            }
            Body::Grid(_) => {
                while let Some(child) = self.grid_footer.first_child() {
                    self.grid_footer.remove(&child);
                }
                if let Some(widget) = widget {
                    self.grid_footer.append(widget);
                }
                self.grid_footer.set_visible(widget.is_some());
            }
            Body::Empty | Body::Column => {}
        }
    }

    /// Fetches the next page of a data list, if there is one.
    fn load_more(self: &Rc<Self>) {
        if self.loading_more.get() || self.more_failed.get() {
            return;
        }
        let Some(mut pager) = self.pager.borrow().clone() else {
            return;
        };
        if !pager.has_more() {
            return;
        }
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.loading_more.set(true);
        self.set_footer(Some(&loading_more()));
        let api = ctx.api.clone();
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let (path, offset) = (pager.path.clone(), pager.offset);
            let answer = runtime::spawn(async move {
                let call = Call::get(Base::V1, path).page(offset, PAGE_SIZE);
                let value: serde_json::Value = api.get(call).await?;
                Ok((pages::parse_paged_list(&value), sent_items(&value)))
            })
            .await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            state.loading_more.set(false);
            state.more_task.take();
            match answer {
                Ok((section, sent)) => {
                    state.set_footer(None);
                    let replace = pager.replace;
                    pager.advance(sent, section.total);
                    state.pager.replace(Some(pager));
                    if replace {
                        state.replace_items(section.items);
                    } else {
                        state.append(section.items);
                    }
                }
                Err(err) => {
                    let path = &pager.path;
                    if let Some(ctx) = state.ctx.upgrade()
                        && tidal::is_auth_error(&err)
                    {
                        ctx.report("Couldn't load more", &err);
                    }
                    log::warn!("paging {path}: {err:#}");
                    state.more_failed.set(true);
                    let weak = Rc::downgrade(&state);
                    let message = format!("Couldn't load more. {}", describe_error(&err));
                    state.set_footer(Some(&paging_error(&message, move || {
                        if let Some(state) = weak.upgrade() {
                            state.more_failed.set(false);
                            state.load_more();
                        }
                    })));
                }
            }
        });
        self.more_task.replace(Some(task));
    }

    fn replace_items(&self, items: Vec<Item>) {
        if items.is_empty() {
            return;
        }
        match &*self.body.borrow() {
            Body::Tracks(list) => list.set_tracks(tracks_of(&items)),
            Body::Grid(store) => {
                let items: Vec<_> = items.into_iter().map(glib::BoxedAnyObject::new).collect();
                store.splice(0, store.n_items(), &items);
            }
            Body::Empty | Body::Column => {}
        }
    }

    fn append(&self, items: Vec<Item>) {
        match &*self.body.borrow() {
            Body::Tracks(list) => list.append(tracks_of(&items)),
            Body::Grid(store) => {
                let items: Vec<_> = items.into_iter().map(glib::BoxedAnyObject::new).collect();
                store.extend_from_slice(&items);
            }
            Body::Empty | Body::Column => {}
        }
    }

    fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        if let Body::Tracks(list) = &*self.body.borrow() {
            list.set_compact(compact);
        }
    }
}

fn tracks_of(items: &[Item]) -> Vec<TrackInfo> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Track(track) => Some(TrackInfo::from((**track).clone())),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paging_counts_what_tidal_sent() {
        // 50 sent (even if fewer were understood): the next page starts
        // at 50.
        let mut pager = Pager::after_first("list".into(), PAGE_SIZE, 120);
        assert_eq!(pager.offset, 50);
        assert!(pager.has_more());
        pager.advance(PAGE_SIZE, 120);
        assert_eq!(pager.offset, 100);
        // A short page ends the list even if the total said more.
        pager.advance(7, 130);
        assert_eq!(pager.offset, 107);
        assert!(!pager.has_more());
        // A first page shorter than asked for is the whole list.
        assert!(!Pager::after_first("list".into(), 12, 40).has_more());
        assert_eq!(sent_items(&serde_json::json!({"items": [1, {}, "x"]})), 3);
    }
}
