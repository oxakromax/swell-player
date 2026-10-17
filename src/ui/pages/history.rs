//! Recently played (docs/design.md §3.12).
//!
//! One virtualised list: what played in this session (from the queue's
//! history, newest first, refreshed as tracks change) as rows, then, as the
//! list's footer, what TIDAL remembers across devices (the home feed's
//! "Recently played" module, `home/pages/CONTINUE_LISTEN_TO/view-all`) as
//! cards. The cards come from the response cache first.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::super::context::Ctx;
use super::super::menus::ItemMenu;
use super::super::sidebar;
use super::super::widgets::carousel::title_row;
use super::super::widgets::media_card::MediaCard;
use super::super::widgets::skeleton;
use super::super::widgets::status::{describe_error, empty_status};
use super::super::widgets::track_list::{TrackList, Variant, compact_bin};
use crate::model::TrackInfo;
use crate::playback::{Event, Source};
use crate::tidal::pages::{self, Section};
use crate::tidal::{self, Base};

/// The `viewAll` of the home feed's CONTINUE_LISTEN_TO module.
const PATH: &str = "home/pages/CONTINUE_LISTEN_TO/view-all?platform=WEB";
const FRESH_FOR: Duration = Duration::from_secs(2 * 60);

struct HistoryPage {
    ctx: Weak<Ctx>,
    list: TrackList,
    stack: gtk::Stack,
    /// "Tracks you play appear here" while the session has none.
    session_hint: gtk::Label,
    /// The TIDAL part: skeleton, cards or error.
    remote: gtk::Stack,
    /// Whether TIDAL has anything (for the empty state of the whole page).
    remote_items: std::cell::Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
}

pub fn page(ctx: &Rc<Ctx>) -> adw::NavigationPage {
    let list = TrackList::new(ctx, Variant::Cover);
    list.set_source(Source {
        title: "Recently Played".into(),
        target: None,
    });

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(18)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    header.append(&title_row("This Session", None));
    let session_hint = gtk::Label::builder()
        .label("Tracks you play in Swell appear here.")
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dimmed"])
        .build();
    header.append(&session_hint);
    list.set_header(Some(&header));

    let remote = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .vhomogeneous(false)
        .build();
    let ghosts = adw::WrapBox::builder()
        .child_spacing(12)
        .line_spacing(12)
        .build();
    for _ in 0..6 {
        ghosts.append(&skeleton::card(160));
    }
    remote.add_named(&ghosts, Some("loading"));
    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(30)
        .margin_bottom(36)
        .margin_start(12)
        .margin_end(12)
        .build();
    footer.append(&title_row(
        "On TIDAL",
        Some("What you played lately, on any device"),
    ));
    footer.append(&remote);
    list.set_footer(Some(&footer));

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(list.widget(), Some("content"));
    stack.add_named(
        &empty_status(
            "document-open-recent-symbolic",
            "Nothing Played Yet",
            "Music you play appears here.",
        ),
        Some("empty"),
    );

    let title = adw::WindowTitle::new("Recently Played", "");
    let header_bar = adw::HeaderBar::builder().title_widget(&title).build();
    header_bar.pack_start(&sidebar::show_sidebar_button(&ctx.window));
    header_bar.pack_end(&sidebar::narrow_main_menu_button(&ctx.window));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header_bar);
    let page = adw::NavigationPage::builder()
        .title("Recently Played")
        .child(&toolbar)
        .build();

    let state = Rc::new(HistoryPage {
        ctx: Rc::downgrade(ctx),
        list: list.clone(),
        stack,
        session_hint,
        remote,
        remote_items: std::cell::Cell::new(false),
        task: RefCell::default(),
    });
    let weak = Rc::downgrade(&state);
    let edges = [header.downgrade(), footer.downgrade()];
    let bin = compact_bin(&state.stack, move |compact| {
        if let Some(state) = weak.upgrade() {
            state.list.set_compact(compact);
        }
        // Narrow: the row's own padding is margin enough, and two columns
        // of cards fit.
        for edge in edges.iter().filter_map(|e| e.upgrade()) {
            edge.set_margin_start(if compact { 0 } else { 12 });
            edge.set_margin_end(if compact { 0 } else { 12 });
        }
    });
    toolbar.set_content(Some(&bin));

    let weak = Rc::downgrade(&state);
    let subscription = ctx.playback.subscribe(move |event| {
        if let (Event::Track(_), Some(state)) = (event, weak.upgrade()) {
            state.show_session();
        }
    });

    state.show_session();
    state.load_remote();
    let weak_ctx = Rc::downgrade(ctx);
    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(ctx) = weak_ctx.upgrade() {
            ctx.playback.unsubscribe(subscription);
        }
        if let Some(state) = owned.take()
            && let Some(task) = state.task.take()
        {
            task.abort();
        }
    });
    page
}

impl HistoryPage {
    /// The session's tracks, newest (the current one) first.
    fn show_session(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let mut tracks: Vec<TrackInfo> = ctx.playback.history();
        tracks.extend(ctx.playback.current());
        tracks.reverse();
        self.session_hint.set_visible(tracks.is_empty());
        self.list.set_tracks(tracks);
        self.update_empty();
    }

    fn update_empty(&self) {
        let empty = self.list.is_empty() && !self.remote_items.get();
        let loading = self.remote.visible_child_name().as_deref() == Some("loading");
        self.stack.set_visible_child_name(if empty && !loading {
            "empty"
        } else {
            "content"
        });
    }

    fn load_remote(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let mut answers =
            ctx.api
                .load_cached(Base::V2, PATH.into(), FRESH_FOR, pages::parse_view_all);
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            while let Some(answer) = answers.recv().await {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                match answer {
                    Ok(section) => state.show_remote(&section),
                    Err(err) => state.remote_failed(&err),
                }
            }
        });
        self.task.replace(Some(task));
    }

    fn show_remote(&self, section: &Section) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let cards = adw::WrapBox::builder()
            .child_spacing(12)
            .line_spacing(12)
            .build();
        let menu = ItemMenu::new(&ctx, &cards);
        for item in &section.items {
            cards.append(MediaCard::new(&ctx, item, Some(&menu)).widget());
        }
        let shown: gtk::Widget = if section.items.is_empty() {
            hint("Nothing yet.").upcast()
        } else {
            cards.upcast()
        };
        self.set_remote(&shown);
        self.remote_items.set(!section.items.is_empty());
        self.update_empty();
    }

    fn remote_failed(self: &Rc<Self>, err: &anyhow::Error) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if tidal::is_auth_error(err) {
            ctx.report("Couldn't load what you played", err);
        }
        let row = gtk::Box::builder().spacing(12).build();
        let label = hint(&format!("Couldn't load this. {}", describe_error(err)));
        label.set_hexpand(true);
        row.append(&label);
        let retry = gtk::Button::builder()
            .label("Retry")
            .valign(gtk::Align::Center)
            .build();
        let weak = Rc::downgrade(self);
        retry.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.remote.set_visible_child_name("loading");
                state.load_remote();
            }
        });
        row.append(&retry);
        self.set_remote(&row);
        self.update_empty();
    }

    fn set_remote(&self, widget: &impl IsA<gtk::Widget>) {
        if let Some(old) = self.remote.child_by_name("shown") {
            self.remote.remove(&old);
        }
        self.remote.add_named(widget, Some("shown"));
        self.remote.set_visible_child_name("shown");
    }
}

fn hint(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dimmed"])
        .build()
}
