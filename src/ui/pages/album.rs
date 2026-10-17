//! The album page (docs/design.md §3.7) and its credits page.
//!
//! One virtualised list is the whole page: the header is its first item,
//! "Disc N" separators sit between volumes, and the footer (release date,
//! copyright, credits, review, "More by …" and related sections) is its
//! last item. Data comes from TIDAL's `pages/album` (cached for a day,
//! revalidated in the background); tracks beyond the page's first batch are
//! fetched with `albums/{id}/tracks`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use anyhow::anyhow;
use gtk::glib;

use super::super::context::Ctx;
use super::super::dialogs;
use super::super::menus;
use super::super::nav::Route;
use super::super::widgets::page_header::PageHeader;
use super::super::widgets::section::{self, section_widget};
use super::super::widgets::skeleton;
use super::super::widgets::status::PageStack;
use super::super::widgets::track_list::{TrackList, Variant, compact_bin};
use crate::model::TrackInfo;
use crate::playback::Source;
use crate::runtime;
use crate::tidal::models::Album;
use crate::tidal::pages::{self, Header, Item, SectionKind, Target};
use crate::tidal::{self, Base};

const FRESH_FOR: Duration = Duration::from_secs(24 * 60 * 60);
const PAGE: usize = 100;

struct AlbumPage {
    ctx: Weak<Ctx>,
    id: u64,
    list: TrackList,
    stack: Rc<PageStack>,
    header_bar: adw::HeaderBar,
    title: adw::WindowTitle,
    page: glib::WeakRef<adw::NavigationPage>,
    header: RefCell<Option<PageHeader>>,
    album: RefCell<Option<Album>>,
    compact: Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    rest: RefCell<Option<glib::JoinHandle<()>>>,
}

pub fn page(ctx: &Rc<Ctx>, id: u64) -> adw::NavigationPage {
    let list = TrackList::new(ctx, Variant::Numbered);
    list.set_group_discs(true);
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
        .title("Album")
        .child(&toolbar)
        .build();

    let state = Rc::new(AlbumPage {
        ctx: Rc::downgrade(ctx),
        id,
        list: list.clone(),
        stack: stack.clone(),
        header_bar,
        title,
        page: page.downgrade(),
        header: RefCell::default(),
        album: RefCell::default(),
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

impl AlbumPage {
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
        let mut answers = ctx.api.load_cached(
            Base::V1,
            format!("pages/album?albumId={}", self.id),
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
            ctx.report("Couldn't load the album", err);
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
        let Some(Header::Album {
            album,
            review,
            credits,
        }) = page.header.clone()
        else {
            return self.show_error(&anyhow!("TIDAL sent an album page without its album"));
        };
        let album = *album;

        let header = PageHeader::album(&ctx, &album);
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
                        Item::Track(track) => {
                            Some(complete(TrackInfo::from((**track).clone()), &album))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let total = section
            .map_or(0, |s| s.total)
            .max(album.number_of_tracks as usize);
        header.set_playable(!tracks.is_empty());

        let artists = album
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        self.list.set_album_artists(&artists);
        self.list.set_source(Source {
            title: album.title.clone(),
            target: Some(Target::Album(album.id)),
        });
        section::align_to_edge(header.widget(), 0);
        self.list.set_header(Some(header.widget()));
        let loaded = tracks.len();
        self.list.set_tracks(tracks);
        self.list.set_footer(Some(&self.footer(
            &ctx,
            &album,
            &page,
            review,
            !credits.is_empty(),
        )));

        self.title.set_title(&album.title);
        self.title.set_subtitle(&artists);
        if let Some(page) = self.page.upgrade() {
            page.set_title(&album.title);
        }
        self.header.replace(Some(header));
        self.album.replace(Some(album.clone()));
        self.stack.show_content();

        if total > loaded {
            self.load_rest(loaded, album);
        }
    }

    /// Fetches the tracks the page did not include.
    fn load_rest(self: &Rc<Self>, offset: usize, album: Album) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let api = ctx.api.clone();
        let id = self.id;
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let mut offset = offset;
            loop {
                let api = api.clone();
                let page =
                    runtime::spawn(async move { api.album_tracks(id, offset, PAGE).await }).await;
                let Some(state) = weak.upgrade() else {
                    return;
                };
                match page {
                    Ok(page) if !page.items.is_empty() => {
                        offset += page.items.len();
                        let total = page.total;
                        state.list.append(
                            page.items
                                .into_iter()
                                .map(|t| complete(TrackInfo::from(t), &album))
                                .collect(),
                        );
                        if let Some(header) = state.header.borrow().as_ref() {
                            header.set_playable(true);
                        }
                        if offset >= total {
                            return;
                        }
                    }
                    Ok(_) => return,
                    Err(err) => {
                        if let Some(ctx) = state.ctx.upgrade() {
                            ctx.report("Couldn't load all the album's tracks", &err);
                        }
                        return;
                    }
                }
            }
        });
        self.rest.replace(Some(task));
    }

    fn play(&self, shuffle: bool) {
        let (Some(ctx), Some(album)) = (self.ctx.upgrade(), self.album.borrow().clone()) else {
            return;
        };
        let source = Source {
            title: album.title.clone(),
            target: Some(Target::Album(album.id)),
        };
        let tracks = self.list.tracks();
        if shuffle {
            menus::shuffle_tracks(&ctx, tracks, source);
        } else {
            menus::play_tracks(&ctx, tracks, 0, source, false);
        }
    }

    fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        self.list.set_compact(compact);
        if let Some(header) = self.header.borrow().as_ref() {
            header.set_compact(compact);
        }
    }

    /// Release date, copyright, credits, review and the page's other
    /// sections ("More by …", related albums and artists).
    fn footer(
        &self,
        ctx: &Rc<Ctx>,
        album: &Album,
        page: &pages::Page,
        review: Option<String>,
        has_credits: bool,
    ) -> gtk::Widget {
        let footer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(36)
            .margin_top(24)
            .margin_bottom(36)
            .build();

        let about = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        if let Some(date) = album.release_date.as_deref().and_then(format_date) {
            about.append(&caption(&format!("Released {date}")));
        }
        if let Some(copyright) = album.copyright.as_deref().filter(|c| !c.is_empty()) {
            about.append(&caption(copyright));
        }
        let credits = gtk::Button::builder()
            .label("Credits")
            .halign(gtk::Align::Start)
            .margin_top(6)
            .css_classes(["pill"])
            .visible(has_credits)
            .build();
        let weak = Rc::downgrade(ctx);
        let id = album.id;
        credits.connect_clicked(move |_| {
            if let Some(ctx) = weak.upgrade() {
                ctx.navigate(Route::AlbumCredits(id));
            }
        });
        about.append(&credits);
        about.add_css_class("page-content");
        footer.append(&about);

        if let Some(review) = review.filter(|r| !r.trim().is_empty()) {
            let card = review_card(ctx, &album.title, review);
            card.add_css_class("page-content");
            footer.append(&card);
        }

        // The only track section is the album's own list.
        for section in page
            .sections
            .iter()
            .filter(|s| s.kind != SectionKind::Tracks)
        {
            footer.append(&section_widget(ctx, section));
        }
        footer.upcast()
    }
}

/// Album tracks from the page may lack the album's cover and title.
fn complete(mut track: TrackInfo, album: &Album) -> TrackInfo {
    if track.cover.is_none() {
        track.cover = album.cover.clone();
    }
    if track.album_id == 0 {
        track.album_id = album.id;
    }
    if track.album.is_empty() {
        track.album = album.title.clone();
    }
    track
}

fn caption(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption", "dimmed"])
        .build()
}

/// The review in a card, 6 lines, with "Read More".
fn review_card(ctx: &Rc<Ctx>, album: &str, review: String) -> gtk::Widget {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["card"])
        .build();
    let inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    inner.append(
        &gtk::Label::builder()
            .label("Review")
            .xalign(0.0)
            .css_classes(["heading"])
            .build(),
    );
    inner.append(
        &gtk::Label::builder()
            .label(&review)
            .xalign(0.0)
            .wrap(true)
            .lines(6)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    let more = gtk::Button::builder()
        .label("Read More")
        .halign(gtk::Align::Start)
        .css_classes(["flat"])
        .build();
    let weak = Rc::downgrade(ctx);
    let album = album.to_owned();
    more.connect_clicked(move |_| {
        if let Some(ctx) = weak.upgrade() {
            dialogs::credits::show_text(&ctx, "Review", &album, &review);
        }
    });
    inner.append(&more);
    card.append(&inner);
    card.upcast()
}

/// "2013-05-17" → "17 May 2013" (in the user's locale's month names).
fn format_date(date: &str) -> Option<String> {
    let day = date.get(..10)?;
    let parsed = glib::DateTime::from_iso8601(&format!("{day}T12:00:00Z"), None).ok()?;
    parsed.format("%-d %B %Y").ok().map(Into::into)
}

/// Credits of the whole album, by role.
pub fn credits_page(ctx: &Rc<Ctx>, id: u64) -> adw::NavigationPage {
    let spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let stack = PageStack::new(&spinner);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(stack.widget()));
    let page = adw::NavigationPage::builder()
        .title("Credits")
        .child(&toolbar)
        .build();
    load_credits(ctx, id, stack.clone());
    let keep = RefCell::new(Some(stack));
    page.connect_destroy(move |_| drop(keep.take()));
    page
}

fn load_credits(ctx: &Rc<Ctx>, id: u64, stack: Rc<PageStack>) {
    let api = ctx.api.clone();
    let weak_ctx = Rc::downgrade(ctx);
    let weak_stack = Rc::downgrade(&stack);
    let task = glib::spawn_future_local(async move {
        let result = runtime::spawn(async move { api.album_credits(id).await }).await;
        let (Some(ctx), Some(stack)) = (weak_ctx.upgrade(), weak_stack.upgrade()) else {
            return;
        };
        match result {
            Ok(credits) if credits.iter().all(|c| c.contributors.is_empty()) => stack.show_empty(
                "avatar-default-symbolic",
                "No Credits",
                "TIDAL has no credits for this album.",
            ),
            Ok(credits) => {
                stack.set_content(&dialogs::credits::credits_page(&ctx, &credits, || {}));
                stack.show_content();
            }
            Err(err) => {
                if tidal::is_auth_error(&err) {
                    ctx.report("Couldn't load the credits", &err);
                }
                let retry_ctx = Rc::downgrade(&ctx);
                let retry_stack = Rc::downgrade(&stack);
                stack.show_error(&err, move || {
                    if let (Some(ctx), Some(stack)) = (retry_ctx.upgrade(), retry_stack.upgrade()) {
                        stack.show_loading();
                        load_credits(&ctx, id, stack);
                    }
                });
            }
        }
    });
    // The stack lives as long as the page; so does the request.
    stack.widget().connect_destroy(move |_| task.abort());
}
