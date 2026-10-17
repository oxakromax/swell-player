//! The artist page (docs/design.md §3.8): header with picture, name, a bio
//! excerpt and Play / Shuffle / Follow / Radio; then the page's sections
//! (top tracks, albums, EPs and singles, appears on, playlists, mixes, fans
//! also like). Everything on it is bounded, so it is a plain scrolled box;
//! the top tracks are an embedded list of five with "View All".
//!
//! Play and Shuffle use all the top tracks (up to 100, fetched when
//! pressed), not just the five shown.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use anyhow::anyhow;
use gtk::glib;

use super::super::context::Ctx;
use super::super::dialogs;
use super::super::menus::{self, TrackQuery};
use super::super::widgets::carousel::title_row;
use super::super::widgets::page_header::PageHeader;
use super::super::widgets::section::{self, SHORT_LIST, section_widget, short_track_list};
use super::super::widgets::skeleton;
use super::super::widgets::status::PageStack;
use super::super::widgets::track_list::compact_bin;
use crate::model::TrackInfo;
use crate::playback::Source;
use crate::tidal::models::Artist;
use crate::tidal::pages::{self, Header, Item, SectionKind, Target};
use crate::tidal::{self, Base};

const FRESH_FOR: Duration = Duration::from_secs(6 * 60 * 60);

struct ArtistPage {
    ctx: Weak<Ctx>,
    id: u64,
    stack: Rc<PageStack>,
    content: gtk::Box,
    header_bar: adw::HeaderBar,
    title: adw::WindowTitle,
    page: glib::WeakRef<adw::NavigationPage>,
    header: RefCell<Option<PageHeader>>,
    artist: RefCell<Option<Artist>>,
    compact: Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    play_task: RefCell<Option<glib::JoinHandle<()>>>,
}

pub fn page(ctx: &Rc<Ctx>, id: u64) -> adw::NavigationPage {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(36)
        .margin_bottom(36)
        .build();
    let clamp = adw::Clamp::builder()
        .maximum_size(1200)
        .tightening_threshold(1200)
        .child(&content)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    let stack = PageStack::new(&skeleton::page(200, true, 5, true));
    stack.set_content(&scrolled);

    let title = adw::WindowTitle::new("", "");
    let header_bar = adw::HeaderBar::builder()
        .title_widget(&title)
        .show_title(false)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header_bar);
    let page = adw::NavigationPage::builder()
        .title("Artist")
        .child(&toolbar)
        .build();

    let state = Rc::new(ArtistPage {
        ctx: Rc::downgrade(ctx),
        id,
        stack: stack.clone(),
        content,
        header_bar,
        title,
        page: page.downgrade(),
        header: RefCell::default(),
        artist: RefCell::default(),
        compact: Cell::new(false),
        task: RefCell::default(),
        play_task: RefCell::default(),
    });

    let weak = Rc::downgrade(&state);
    let bin = compact_bin(stack.widget(), move |compact| {
        if let Some(state) = weak.upgrade() {
            state.set_compact(compact);
        }
    });
    toolbar.set_content(Some(&bin));
    state.set_compact(false);

    let weak = Rc::downgrade(&state);
    scrolled
        .vadjustment()
        .connect_value_changed(move |adjustment| {
            if let Some(state) = weak.upgrade() {
                let height = state
                    .header
                    .borrow()
                    .as_ref()
                    .map_or(260, |h| h.widget().height());
                state
                    .header_bar
                    .set_show_title(adjustment.value() > f64::from(height));
            }
        });

    state.load();
    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(state) = owned.take() {
            for task in [state.task.take(), state.play_task.take()]
                .into_iter()
                .flatten()
            {
                task.abort();
            }
        }
    });
    page
}

impl ArtistPage {
    fn load(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if !self.stack.is_showing_content() {
            self.stack.show_loading();
        }
        let mut answers = ctx.api.load_cached(
            Base::V1,
            format!("pages/artist?artistId={}", self.id),
            FRESH_FOR,
            pages::parse_v1,
        );
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            while let Some(answer) = answers.recv().await {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                match answer {
                    Ok(page) => state.show(page),
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
            ctx.report("Couldn't load the artist", err);
        }
        if self.stack.is_showing_content() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.stack.show_error(err, move || {
            if let Some(state) = weak.upgrade() {
                state.load();
            }
        });
    }

    fn show(self: &Rc<Self>, page: pages::Page) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let Some(Header::Artist { artist, bio }) = page.header.clone() else {
            return self.show_error(&anyhow!("TIDAL sent an artist page without its artist"));
        };
        let artist = *artist;

        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }

        let header = PageHeader::artist(&ctx, &artist);
        if let Some(bio) = bio.filter(|b| !b.trim().is_empty()) {
            header.add_detail(&bio_excerpt(&ctx, &artist.name, bio));
        }
        header.set_compact(self.compact.get());
        let weak = Rc::downgrade(self);
        header.connect_play(move || {
            if let Some(state) = weak.upgrade() {
                state.play(false);
            }
        });
        let weak = Rc::downgrade(self);
        header.connect_shuffle(move || {
            if let Some(state) = weak.upgrade() {
                state.play(true);
            }
        });
        // On the content edge, like the sections (which line up themselves).
        section::align_to_edge(header.widget(), 0);
        self.content.append(header.widget());

        let mut top_tracks_shown = false;
        for section in &page.sections {
            let widget = if section.kind == SectionKind::Tracks && !top_tracks_shown {
                top_tracks_shown = true;
                self.top_tracks(&ctx, &artist, section)
            } else {
                section_widget(&ctx, section)
            };
            self.content.append(&widget);
        }
        if !top_tracks_shown && page.sections.is_empty() {
            self.content
                .append(&super::super::widgets::status::empty_status(
                    "avatar-default-symbolic",
                    "Nothing Here Yet",
                    "TIDAL has no music from this artist in your country.",
                ));
        }

        self.title.set_title(&artist.name);
        if let Some(page) = self.page.upgrade() {
            page.set_title(&artist.name);
        }
        self.header.replace(Some(header));
        self.artist.replace(Some(artist));
        self.set_compact(self.compact.get());
        self.stack.show_content();
    }

    /// "Top Tracks": five rows and "View All".
    fn top_tracks(&self, ctx: &Rc<Ctx>, artist: &Artist, section: &pages::Section) -> gtk::Widget {
        let tracks: Vec<TrackInfo> = section
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Track(track) => Some(TrackInfo::from((**track).clone())),
                _ => None,
            })
            .take(SHORT_LIST)
            .collect();
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .css_classes(["page-content"])
            .build();
        let header = title_row(&section.title, None);
        if let Some(route) =
            section::view_all_route(section).filter(|_| section.total > tracks.len())
        {
            let button = gtk::Button::builder()
                .label("View All")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let ctx = Rc::downgrade(ctx);
            button.connect_clicked(move |_| {
                if let Some(ctx) = ctx.upgrade() {
                    ctx.navigate(route.clone());
                }
            });
            header.append(&button);
        }
        widget.append(&header);
        widget.append(&short_track_list(ctx, tracks, source(artist)));
        widget.upcast()
    }

    /// Plays every top track (fetched now), from the first or shuffled.
    fn play(self: &Rc<Self>, shuffle: bool) {
        let (Some(ctx), Some(artist)) = (self.ctx.upgrade(), self.artist.borrow().clone()) else {
            return;
        };
        let task = glib::spawn_future_local(async move {
            match TrackQuery::ArtistTop(artist.id)
                .fetch(ctx.api.clone())
                .await
            {
                Ok(tracks) if tracks.is_empty() => ctx.toast("There's nothing to play here"),
                Ok(tracks) if shuffle => menus::shuffle_tracks(&ctx, tracks, source(&artist)),
                Ok(tracks) => menus::play_tracks(&ctx, tracks, 0, source(&artist), false),
                Err(err) => ctx.report("Couldn't load the artist's tracks", &err),
            }
        });
        if let Some(old) = self.play_task.replace(Some(task)) {
            old.abort();
        }
    }

    fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        if let Some(header) = self.header.borrow().as_ref() {
            header.set_compact(compact);
        }
    }
}

fn source(artist: &Artist) -> Source {
    Source {
        title: artist.name.clone(),
        target: Some(Target::Artist(artist.id)),
    }
}

/// Three lines of the biography and "Read More".
fn bio_excerpt(ctx: &Rc<Ctx>, name: &str, bio: String) -> gtk::Widget {
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    let label = gtk::Label::builder()
        .label(
            bio.lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or_default(),
        )
        .xalign(0.0)
        .wrap(true)
        .lines(3)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(80)
        .css_classes(["dimmed"])
        .build();
    widget.append(&label);
    let more = gtk::Button::builder()
        .label("Read More")
        .halign(gtk::Align::Start)
        .css_classes(["flat"])
        .build();
    let weak = Rc::downgrade(ctx);
    let name = name.to_owned();
    more.connect_clicked(move |_| {
        if let Some(ctx) = weak.upgrade() {
            dialogs::credits::show_text(&ctx, "Biography", &name, &bio);
        }
    });
    widget.append(&more);
    widget.upcast()
}
