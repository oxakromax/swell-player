//! Turns a page [`Section`] into the right widget (shared by Home,
//! Explore, Search, Album and Artist pages), and what clicking an item does.
//!
//! | Kind | Widget |
//! |---|---|
//! | Tracks | title + a short embedded [`TrackList`] (5 rows) + "View All" |
//! | Cards | [`CarouselSection`] |
//! | Links | title + wrapping chips |
//! | Featured | carousel of the promoted items |
//! | Shortcuts | grid of horizontal tiles (56 px cover + title) |

use std::rc::Rc;

use gtk::prelude::*;

use super::carousel::{CarouselSection, title_row};
use super::media_card::{self, attach_menu};
use super::track_list::{TrackList, Variant};
use crate::model::TrackInfo;
use crate::playback::Source;
use crate::tidal::pages::{Item, Section, SectionKind, Target};
use crate::ui::context::Ctx;
use crate::ui::menus::{self, ItemMenu, Subject};
use crate::ui::nav::Route;

/// Rows shown by a track section before "View All".
pub const SHORT_LIST: usize = 5;

/// The widget for `section` (an empty box if it has no items).
pub fn section_widget(ctx: &Rc<Ctx>, section: &Section) -> gtk::Widget {
    if section.items.is_empty() {
        return gtk::Box::new(gtk::Orientation::Vertical, 0).upcast();
    }
    match section.kind {
        SectionKind::Tracks => tracks_section(ctx, section, SHORT_LIST),
        SectionKind::Cards | SectionKind::Featured => {
            let carousel = CarouselSection::new(
                ctx,
                &section.title,
                section.subtitle.as_deref(),
                &section.items,
            );
            if let Some(route) = view_all_route(section) {
                let ctx = Rc::downgrade(ctx);
                carousel.connect_view_all(move || {
                    if let Some(ctx) = ctx.upgrade() {
                        ctx.navigate(route.clone());
                    }
                });
            }
            carousel.widget().clone()
        }
        SectionKind::Links => links_section(ctx, section),
        SectionKind::Shortcuts => shortcuts_section(ctx, section),
    }
}

/// Where "View All" of a section leads, if it has more than it shows.
pub fn view_all_route(section: &Section) -> Option<Route> {
    let more = section.more.clone()?;
    Some(Route::More {
        title: section.title.clone(),
        more,
    })
}

/// A title, `limit` track rows and "View All" (when the section has more
/// than that and a place to see them).
pub fn tracks_section(ctx: &Rc<Ctx>, section: &Section, limit: usize) -> gtk::Widget {
    let tracks: Vec<TrackInfo> = section
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Track(track) => Some(TrackInfo::from((**track).clone())),
            _ => None,
        })
        .collect();
    let route = view_all_route(section);
    // Without a "View All" page, show what there is.
    let shown = if route.is_some() {
        limit
    } else {
        limit.max(10)
    };
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    let header = title_row(&section.title, section.subtitle.as_deref());
    if let Some(route) = route.filter(|_| section.total > shown.min(tracks.len())) {
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
    let list = short_track_list(
        ctx,
        tracks.into_iter().take(shown).collect(),
        Source {
            title: section.title.clone(),
            target: None,
        },
    );
    widget.append(&list);
    widget.upcast()
}

/// A non-scrolling list of a few tracks. It turns compact with the page:
/// it follows the nearest [`compact_bin`] around it.
///
/// [`compact_bin`]: super::track_list::compact_bin
pub fn short_track_list(ctx: &Rc<Ctx>, tracks: Vec<TrackInfo>, source: Source) -> gtk::Widget {
    let list = TrackList::embedded(ctx, Variant::Cover);
    list.set_source(source);
    list.set_tracks(tracks);
    list.follow_page_compact();
    list.widget().clone()
}

fn links_section(ctx: &Rc<Ctx>, section: &Section) -> gtk::Widget {
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    if !section.title.is_empty() {
        widget.append(&title_row(&section.title, section.subtitle.as_deref()));
    }
    let chips = adw::WrapBox::builder()
        .child_spacing(6)
        .line_spacing(6)
        .build();
    for item in &section.items {
        let Item::Link(link) = item else {
            continue;
        };
        let chip = gtk::Button::builder()
            .label(&link.title)
            .css_classes(["chip"])
            .build();
        let ctx = Rc::downgrade(ctx);
        let item = item.clone();
        chip.connect_clicked(move |_| {
            if let Some(ctx) = ctx.upgrade() {
                activate_item(&ctx, &item);
            }
        });
        chips.append(&chip);
    }
    widget.append(&chips);
    widget.upcast()
}

fn shortcuts_section(ctx: &Rc<Ctx>, section: &Section) -> gtk::Widget {
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    if !section.title.is_empty() {
        widget.append(&title_row(&section.title, section.subtitle.as_deref()));
    }
    let grid = gtk::FlowBox::builder()
        .min_children_per_line(1)
        .max_children_per_line(4)
        .homogeneous(true)
        .column_spacing(12)
        .row_spacing(12)
        .selection_mode(gtk::SelectionMode::None)
        .build();
    let menu = ItemMenu::new(ctx, &grid);
    for item in section.items.iter().take(8) {
        let (title, placeholder) = match item {
            Item::Track(track) => (track.title.clone(), "audio-x-generic-symbolic"),
            Item::Album(album) => (album.title.clone(), "audio-x-generic-symbolic"),
            Item::Artist(artist) => (artist.name.clone(), "avatar-default-symbolic"),
            Item::Playlist(playlist) => (playlist.title.clone(), "view-list-symbolic"),
            Item::Mix(mix) => (mix.title.clone(), "view-list-symbolic"),
            Item::Link(link) => (link.title.clone(), "folder-music-symbolic"),
        };
        let content = gtk::Box::builder().spacing(12).build();
        let art: gtk::Widget = if let Item::Artist(_) = item {
            media_card::avatar(ctx, media_card::item_image(item), &title, 56).upcast()
        } else {
            media_card::cover(ctx, media_card::item_image(item), 56, placeholder, false)
        };
        content.append(&art);
        content.append(
            &gtk::Label::builder()
                .label(&title)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .lines(2)
                .wrap(true)
                .max_width_chars(1)
                .css_classes(["heading"])
                .build(),
        );
        let tile = gtk::Button::builder()
            .child(&content)
            .tooltip_text(&title)
            .css_classes(["card"])
            .build();
        let weak = Rc::downgrade(ctx);
        let open = item.clone();
        tile.connect_clicked(move |_| {
            if let Some(ctx) = weak.upgrade() {
                activate_item(&ctx, &open);
            }
        });
        if let Some(subject) = Subject::from_item(item) {
            attach_menu(&tile, &menu, subject);
        }
        grid.append(&tile);
    }
    widget.append(&grid);
    widget.upcast()
}

/// What clicking an item does: open it, or play it for tracks.
pub fn activate_item(ctx: &Rc<Ctx>, item: &Item) {
    match item {
        Item::Track(track) => {
            let track = TrackInfo::from((**track).clone());
            let source = Subject::track(track.clone()).source();
            menus::play_tracks(ctx, vec![track], 0, source, true);
        }
        Item::Album(album) => ctx.navigate(Route::Album(album.id)),
        Item::Artist(artist) => ctx.navigate(Route::Artist(artist.id)),
        Item::Playlist(playlist) => ctx.navigate(Route::Playlist(playlist.uuid.clone())),
        Item::Mix(mix) => ctx.navigate(Route::Mix(mix.id.clone())),
        Item::Link(link) => match &link.target {
            Target::Page(path) => ctx.navigate(Route::Page {
                path: path.clone(),
                title: link.title.clone(),
            }),
            target => activate_target(ctx, target),
        },
    }
}

/// Opens a link target: a page, a collection view, a track (plays it) or
/// a web address (in the browser).
pub fn activate_target(ctx: &Rc<Ctx>, target: &Target) {
    match target {
        Target::Track(id) => menus::play_track_id(ctx, *id),
        Target::Url(url) => {
            gtk::UriLauncher::new(url).launch(
                Some(&ctx.window),
                gtk::gio::Cancellable::NONE,
                |result| {
                    if let Err(err) = result {
                        log::warn!("could not open a link: {err}");
                    }
                },
            );
        }
        other => {
            if let Some(route) = Route::from_target(other) {
                ctx.navigate(route);
            }
        }
    }
}
