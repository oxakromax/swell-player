//! Loading placeholders with the shape of what is coming (docs/design.md
//! §4.7). They are presentation only; the container announces "Loading".
//!
//! They pulse only while on screen ([`animate_while_mapped`]): a CSS
//! animation on a hidden widget would still redraw the window every frame.

use gtk::prelude::*;

use crate::i18n::tr;

/// Lets the `.skeleton` parts inside `widget` pulse only while it is
/// mapped (the `skeleton-live` class), so placeholders left in a stack
/// under the content cost nothing.
pub fn animate_while_mapped(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.upcast_ref::<gtk::Widget>();
    if widget.is_mapped() {
        widget.add_css_class("skeleton-live");
    }
    widget.connect_map(|widget| widget.add_css_class("skeleton-live"));
    widget.connect_unmap(|widget| widget.remove_css_class("skeleton-live"));
}

fn block(width: i32, height: i32) -> gtk::Box {
    gtk::Box::builder()
        .width_request(width)
        .height_request(height)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .css_classes(["skeleton"])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build()
}

/// A track row: 48 px square and two bars (60 px tall).
pub fn row() -> gtk::Widget {
    let row = gtk::Box::builder()
        .spacing(12)
        .height_request(48)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    row.append(&block(48, 48));
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .valign(gtk::Align::Center)
        .build();
    text.append(&block(240, 12));
    text.append(&block(150, 12));
    row.append(&text);
    animate_while_mapped(&row);
    row.upcast()
}

/// A card: cover square and two bars.
pub fn card(size: i32) -> gtk::Widget {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let cover = block(size, size);
    cover.add_css_class("large");
    card.append(&cover);
    card.append(&block(size * 3 / 4, 12));
    card.append(&block(size / 2, 10));
    animate_while_mapped(&card);
    card.upcast()
}

/// A page header: cover (round for artists) and three bars.
pub fn header(cover: i32, round: bool) -> gtk::Widget {
    let header = gtk::Box::builder()
        .spacing(24)
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    let image = block(cover, cover);
    if round {
        // Rounder than any cover so it reads as an avatar.
        image.set_overflow(gtk::Overflow::Hidden);
        image.add_css_class("circular");
    }
    header.append(&image);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .valign(gtk::Align::Center)
        .build();
    text.append(&block(60, 10));
    text.append(&block(320, 28));
    text.append(&block(200, 14));
    header.append(&text);
    animate_while_mapped(&header);
    header.upcast()
}

/// A whole page: header and `rows` ghost rows, or a row of cards.
pub fn page(cover: i32, round: bool, rows: usize, cards: bool) -> gtk::Widget {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    page.update_property(&[gtk::accessible::Property::Label(tr("Loading"))]);
    page.update_state(&[gtk::accessible::State::Busy(true)]);
    page.append(&header(cover, round));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for _ in 0..rows {
        list.append(&row());
    }
    page.append(&list);
    if cards {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        for _ in 0..6 {
            row.append(&card(160));
        }
        page.append(&row);
    }
    let clamp = adw::Clamp::builder()
        .maximum_size(1200)
        .tightening_threshold(1200)
        .child(&page)
        .valign(gtk::Align::Start)
        .build();
    // Clipped rather than scrolled: the placeholder never asks for more
    // room than the page has.
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(&clamp)
        .build()
        .upcast()
}
