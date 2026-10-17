//! A titled row of cards that scrolls sideways (docs/design.md §4.3).
//!
//! At most [`MAX_CARDS`] cards are built (all at once: a row is short); the
//! rest is behind "View All". ‹ › scroll by a page of whole cards with a
//! 300 ms animation and are insensitive at the ends; they hide when every
//! card fits.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use super::media_card::MediaCard;
use crate::tidal::pages::Item;
use crate::ui::context::Ctx;
use crate::ui::menus::ItemMenu;

pub const MAX_CARDS: usize = 20;
const SPACING: i32 = 12;

pub struct CarouselSection {
    widget: gtk::Box,
    view_all: gtk::Button,
}

impl CarouselSection {
    pub fn new(ctx: &Rc<Ctx>, title: &str, subtitle: Option<&str>, items: &[Item]) -> Self {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();

        let cards = gtk::Box::builder().spacing(SPACING).build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&cards)
            // Covers load as their cards come near (see media_card).
            .css_classes([super::media_card::CAROUSEL_SCROLLER])
            .build();

        let back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();
        super::label_button(&back, "Scroll Back");
        let forward = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();
        super::label_button(&forward, "Scroll Forward");
        let view_all = gtk::Button::builder()
            .label("View All")
            .valign(gtk::Align::Center)
            .visible(false)
            .css_classes(["flat"])
            .build();

        // The title lines up with the page's content edge, and so do the
        // covers; the row itself scrolls from side to side of the page.
        let header = title_row(title, subtitle);
        header.add_css_class("page-content");
        super::section::align_to_edge(&cards, super::section::CARD_INSET);
        header.append(&back);
        header.append(&forward);
        header.append(&view_all);
        widget.append(&header);
        widget.append(&scrolled);

        let menu = ItemMenu::new(ctx, &widget);
        for item in items.iter().take(MAX_CARDS) {
            cards.append(MediaCard::new(ctx, item, Some(&menu)).widget());
        }

        let adjustment = scrolled.hadjustment();
        // Swipe/scroll replaces the arrows in narrow windows.
        let narrow = Rc::new(std::cell::Cell::new(false));
        let update = {
            let back = back.downgrade();
            let forward = forward.downgrade();
            let narrow = narrow.clone();
            move |adjustment: &gtk::Adjustment| {
                let (Some(back), Some(forward)) = (back.upgrade(), forward.upgrade()) else {
                    return;
                };
                let scrollable = !narrow.get() && adjustment.upper() > adjustment.page_size() + 1.0;
                back.set_visible(scrollable);
                forward.set_visible(scrollable);
                back.set_sensitive(adjustment.value() > 1.0);
                forward.set_sensitive(
                    adjustment.value() + adjustment.page_size() < adjustment.upper() - 1.0,
                );
            }
        };
        adjustment.connect_value_changed(update.clone());
        adjustment.connect_changed(update.clone());
        super::follow_narrow(&widget, {
            let adjustment = adjustment.downgrade();
            move |is_narrow| {
                narrow.set(is_narrow);
                if let Some(adjustment) = adjustment.upgrade() {
                    update(&adjustment);
                }
            }
        });

        let animation: Rc<RefCell<Option<adw::TimedAnimation>>> = Rc::default();
        for (button, direction) in [(&back, -1.0), (&forward, 1.0)] {
            let scrolled = scrolled.downgrade();
            let cards = cards.downgrade();
            let animation = animation.clone();
            button.connect_clicked(move |_| {
                let (Some(scrolled), Some(cards)) = (scrolled.upgrade(), cards.upgrade()) else {
                    return;
                };
                scroll_page(&scrolled, &cards, direction, &animation);
            });
        }

        Self { widget, view_all }
    }

    /// Shows "View All", which calls `f`.
    pub fn connect_view_all(&self, f: impl Fn() + 'static) {
        self.view_all.set_visible(true);
        self.view_all.connect_clicked(move |_| f());
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.widget.upcast_ref()
    }
}

/// Scrolls by the visible width rounded down to whole cards.
fn scroll_page(
    scrolled: &gtk::ScrolledWindow,
    cards: &gtk::Box,
    direction: f64,
    animation: &RefCell<Option<adw::TimedAnimation>>,
) {
    let adjustment = scrolled.hadjustment();
    let stride = cards
        .first_child()
        .map_or(172.0, |card| f64::from(card.width() + SPACING));
    let page = ((adjustment.page_size() / stride).floor() * stride).max(stride);
    let from = adjustment.value();
    let max = (adjustment.upper() - adjustment.page_size()).max(0.0);
    // Land on a card boundary.
    let to = (((from + direction * page) / stride).round() * stride).clamp(0.0, max);
    if let Some(running) = animation.take() {
        running.skip();
    }
    let target = adw::CallbackAnimationTarget::new({
        let adjustment = adjustment.clone();
        move |value| adjustment.set_value(value)
    });
    let timed = adw::TimedAnimation::builder()
        .widget(scrolled)
        .value_from(from)
        .value_to(to)
        .duration(300)
        .easing(adw::Easing::EaseOutCubic)
        .target(&target)
        .build();
    timed.play();
    animation.replace(Some(timed));
}

/// A section title (`title-3`) with an optional subtitle, as the first
/// child of a horizontal box the caller can append buttons to.
pub fn title_row(title: &str, subtitle: Option<&str>) -> gtk::Box {
    let row = gtk::Box::builder().spacing(6).build();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    text.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["title-3"])
            .accessible_role(gtk::AccessibleRole::Heading)
            .build(),
    );
    if let Some(subtitle) = subtitle.filter(|s| !s.is_empty()) {
        text.append(
            &gtk::Label::builder()
                .label(subtitle)
                .xalign(0.0)
                .wrap(true)
                .css_classes(["dimmed"])
                .build(),
        );
    }
    row.append(&text);
    row
}
