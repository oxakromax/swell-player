//! The mix and radio pages (docs/design.md §3.10): a mix from the feed or
//! the library, a track's radio or an artist's radio.
//!
//! Radios are mixes too: `tracks/{id}/mix` and `artists/{id}/mix` name the
//! mix (cached for a week; the id is stable), then the page is the mix's
//! `pages/mix` (cached for 30 minutes, revalidated in the background),
//! titled "<Track> Radio" / "<Artist> Radio". Tracks beyond the page's
//! first batch are fetched with `mixes/{id}/items`. Like the album page,
//! one virtualised list is the whole page with the header as its first
//! item.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use anyhow::anyhow;
use gtk::glib;

use super::super::context::Ctx;
use super::super::menus;
use super::super::widgets::page_header::PageHeader;
use super::super::widgets::section;
use super::super::widgets::skeleton;
use super::super::widgets::status::PageStack;
use super::super::widgets::track_list::{TrackList, Variant, compact_bin};
use crate::model::TrackInfo;
use crate::playback::Source;
use crate::runtime;
use crate::tidal::models::Mix;
use crate::tidal::pages::{self, Header, Item, Target};
use crate::tidal::{self, Base};

/// Mixes are personal and change daily at most.
const FRESH_FOR: Duration = Duration::from_secs(30 * 60);
/// A track's or artist's radio keeps its id.
const RADIO_ID_FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const PAGE: usize = 100;

pub enum Kind {
    Mix(String),
    TrackRadio(u64),
    ArtistRadio(u64),
}

impl Kind {
    fn overtitle(&self) -> &'static str {
        match self {
            Self::Mix(_) => "Mix",
            Self::TrackRadio(_) => "Track Radio",
            Self::ArtistRadio(_) => "Artist Radio",
        }
    }

    /// The overtitle for `mix`: radios opened as plain mixes (from the
    /// feed, the library) are named as radios too.
    fn overtitle_for(&self, mix: &Mix) -> &'static str {
        match (self, mix.kind.as_deref()) {
            (Self::Mix(_), Some("TRACK_MIX")) => "Track Radio",
            (Self::Mix(_), Some("ARTIST_MIX")) => "Artist Radio",
            _ => self.overtitle(),
        }
    }

    /// Where the radio's mix id is found.
    fn radio_path(&self) -> Option<String> {
        match self {
            Self::Mix(_) => None,
            Self::TrackRadio(id) => Some(format!("tracks/{id}/mix")),
            Self::ArtistRadio(id) => Some(format!("artists/{id}/mix")),
        }
    }
}

struct MixPage {
    ctx: Weak<Ctx>,
    kind: Kind,
    list: TrackList,
    stack: Rc<PageStack>,
    header_bar: adw::HeaderBar,
    title: adw::WindowTitle,
    page: glib::WeakRef<adw::NavigationPage>,
    header: RefCell<Option<PageHeader>>,
    mix: RefCell<Option<Mix>>,
    compact: Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    rest: RefCell<Option<glib::JoinHandle<()>>>,
}

pub fn page(ctx: &Rc<Ctx>, kind: Kind) -> adw::NavigationPage {
    let list = TrackList::new(ctx, Variant::Cover);
    let stack = PageStack::new(&skeleton::page(240, false, 8, false));
    stack.set_content(list.widget());

    let title = adw::WindowTitle::new("", "");
    let header_bar = adw::HeaderBar::builder()
        .title_widget(&title)
        .show_title(false)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header_bar);
    let page = adw::NavigationPage::builder()
        .title(kind.overtitle())
        .child(&toolbar)
        .build();

    let state = Rc::new(MixPage {
        ctx: Rc::downgrade(ctx),
        kind,
        list: list.clone(),
        stack: stack.clone(),
        header_bar,
        title,
        page: page.downgrade(),
        header: RefCell::default(),
        mix: RefCell::default(),
        compact: Cell::new(false),
        task: RefCell::default(),
        rest: RefCell::default(),
    });

    let weak = Rc::downgrade(&state);
    let bin = compact_bin(stack.widget(), move |compact| {
        if let Some(state) = weak.upgrade() {
            state.set_compact(compact);
        }
    });
    toolbar.set_content(Some(&bin));

    // The header bar shows the title once the page header scrolls away.
    if let Some(scrolled) = list.scrolled() {
        let weak = Rc::downgrade(&state);
        scrolled
            .vadjustment()
            .connect_value_changed(move |adjustment| {
                if let Some(state) = weak.upgrade() {
                    let height = state
                        .header
                        .borrow()
                        .as_ref()
                        .map_or(300, |h| h.widget().height());
                    state
                        .header_bar
                        .set_show_title(adjustment.value() > f64::from(height));
                }
            });
    }

    state.load();
    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(state) = owned.take() {
            state.abort();
        }
    });
    page
}

impl MixPage {
    fn abort(&self) {
        for task in [self.task.take(), self.rest.take()].into_iter().flatten() {
            task.abort();
        }
    }

    fn load(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.abort();
        if !self.stack.is_showing_content() {
            self.stack.show_loading();
        }
        let (Kind::TrackRadio(_) | Kind::ArtistRadio(_), Some(path)) =
            (&self.kind, self.kind.radio_path())
        else {
            if let Kind::Mix(id) = &self.kind {
                self.load_mix(id.clone());
            }
            return;
        };
        let mut answers = ctx
            .api
            .load_cached(Base::V1, path, RADIO_ID_FRESH_FOR, |v| {
                v.get("id")
                    .and_then(|id| id.as_str())
                    .map(str::to_owned)
                    .filter(|id| !id.is_empty())
            });
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            // The first answer (cached or not) is enough.
            let Some(answer) = answers.recv().await else {
                return;
            };
            let Some(state) = weak.upgrade() else {
                return;
            };
            match answer {
                Ok(Some(id)) => state.load_mix(id),
                Ok(None) => state.no_radio(),
                Err(err) if tidal::is_not_found(&err) => state.no_radio(),
                Err(err) => state.show_error(&err),
            }
        });
        self.task.replace(Some(task));
    }

    fn load_mix(self: &Rc<Self>, id: String) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let mut answers = ctx.api.load_cached(
            Base::V1,
            format!("pages/mix?mixId={id}"),
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
                    Ok(page) => state.show(&id, page),
                    Err(err) => state.show_error(&err),
                }
            }
        });
        // (Dropping the handle of the radio lookup that got here does not
        // stop it; it is finishing anyway.)
        self.task.replace(Some(task));
    }

    /// The track or artist has no radio.
    fn no_radio(&self) {
        let (title, description) = match self.kind {
            Kind::ArtistRadio(_) => (
                "No Radio for This Artist",
                "TIDAL has no radio for this artist yet.",
            ),
            _ => (
                "No Radio for This Track",
                "TIDAL has no radio for this track yet. Try the artist's radio instead.",
            ),
        };
        self.stack.show_empty(
            super::super::widgets::icon_name(
                "swell-radio-symbolic",
                "media-playlist-repeat-symbolic",
            ),
            title,
            description,
        );
    }

    fn show_error(self: &Rc<Self>, err: &anyhow::Error) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if tidal::is_auth_error(err) {
            ctx.report("Couldn't load the mix", err);
        }
        if self.stack.is_showing_content() {
            return;
        }
        if tidal::is_not_found(err) && !matches!(self.kind, Kind::Mix(_)) {
            return self.no_radio();
        }
        let weak = Rc::downgrade(self);
        self.stack.show_error(err, move || {
            if let Some(state) = weak.upgrade() {
                state.load();
            }
        });
    }

    fn show(self: &Rc<Self>, id: &str, page: pages::Page) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let Some(Header::Mix(mix)) = page.header.clone() else {
            return self.show_error(&anyhow!("TIDAL sent a mix page without its mix"));
        };
        let mut mix = *mix;
        if mix.id.is_empty() {
            mix.id = id.to_owned();
        }
        // A track radio's mix is named after the track, an artist's after
        // the artist.
        let overtitle = self.kind.overtitle_for(&mix);
        if overtitle != "Mix" && !mix.title.is_empty() && !mix.title.ends_with(" Radio") {
            mix.title = format!("{} Radio", mix.title);
        }

        let header = PageHeader::mix(&ctx, &mix, overtitle);
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

        let section = page.tracks();
        let tracks: Vec<TrackInfo> = section
            .map(|s| {
                s.items
                    .iter()
                    .filter_map(|item| match item {
                        Item::Track(track) => Some(TrackInfo::from((**track).clone())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let total = section.map_or(0, |s| s.total);
        header.set_playable(!tracks.is_empty());

        self.list.set_source(source(&mix));
        section::align_to_edge(header.widget(), 0);
        self.list.set_header(Some(header.widget()));
        let loaded = tracks.len();
        let empty = tracks.is_empty();
        self.list.set_tracks(tracks);
        self.list
            .set_footer(empty.then(|| empty_footer(&self.kind)).as_ref());

        self.title.set_title(&mix.title);
        self.title.set_subtitle(&mix.subtitle);
        if let Some(page) = self.page.upgrade() {
            page.set_title(&mix.title);
        }
        self.header.replace(Some(header));
        self.mix.replace(Some(mix.clone()));
        self.stack.show_content();

        if total > loaded && loaded > 0 {
            self.load_rest(mix.id, loaded);
        }
    }

    /// Fetches the tracks the page did not include.
    fn load_rest(self: &Rc<Self>, id: String, offset: usize) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.rest.take() {
            task.abort();
        }
        let api = ctx.api.clone();
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let mut offset = offset;
            loop {
                let api = api.clone();
                let request = id.clone();
                let page =
                    runtime::spawn(async move { api.mix_tracks(&request, offset, PAGE).await })
                        .await;
                let Some(state) = weak.upgrade() else {
                    return;
                };
                match page {
                    Ok(page) if !page.items.is_empty() => {
                        offset += page.items.len();
                        let total = page.total;
                        state
                            .list
                            .append(page.items.into_iter().map(TrackInfo::from).collect());
                        if offset >= total {
                            return;
                        }
                    }
                    Ok(_) => return,
                    Err(err) => {
                        if let Some(ctx) = state.ctx.upgrade() {
                            ctx.report("Couldn't load all the mix's tracks", &err);
                        }
                        return;
                    }
                }
            }
        });
        self.rest.replace(Some(task));
    }

    fn play(&self, shuffle: bool) {
        let (Some(ctx), Some(mix)) = (self.ctx.upgrade(), self.mix.borrow().clone()) else {
            return;
        };
        let tracks = self.list.tracks();
        if shuffle {
            menus::shuffle_tracks(&ctx, tracks, source(&mix));
        } else {
            menus::play_tracks(&ctx, tracks, 0, source(&mix), false);
        }
    }

    fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        self.list.set_compact(compact);
        if let Some(header) = self.header.borrow().as_ref() {
            header.set_compact(compact);
        }
    }
}

fn source(mix: &Mix) -> Source {
    Source {
        title: mix.title.clone(),
        target: Some(Target::Mix(mix.id.clone())),
    }
}

/// Shown under the header when the mix has no tracks.
fn empty_footer(kind: &Kind) -> gtk::Widget {
    super::playlist::empty_note(
        "view-list-symbolic",
        match kind {
            Kind::Mix(_) => "This Mix Is Empty",
            _ => "This Radio Is Empty",
        },
        "TIDAL has nothing in it right now. Try again later.",
    )
    .upcast()
}
