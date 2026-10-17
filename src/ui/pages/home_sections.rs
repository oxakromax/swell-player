//! The module renderer shared by Home, Explore, editorial pages and
//! multi-module "View all" pages (docs/design.md §3.2, §3.3): one grammar
//! of sections, learnt once.
//!
//! [`Scaffold`] is the page around the sections: a header bar whose title
//! appears once the page heading scrolls away, the loading / content /
//! error stack, and a clamped column of sections, each lining its content
//! up with the page's content edge (`.page-content`; card rows scroll from
//! side to side). [`section_widget`] turns a [`Section`] into its widget:
//! the kit's renderer, plus a banner or large tiles for promotions and
//! "View All" on category chips.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::super::context::Ctx;
use super::super::sidebar;
use super::super::widgets::carousel::title_row;
use super::super::widgets::media_card;
use super::super::widgets::section::{self, activate_item, view_all_route};
use super::super::widgets::skeleton;
use super::super::widgets::status::PageStack;
use super::super::widgets::track_list::compact_bin;

use crate::i18n::tr;
use crate::tidal::pages::{Item, Section, SectionKind};

/// Widest the column of sections gets.
const MAX_WIDTH: i32 = 1600;
/// Size of a promotion tile's image (TIDAL serves them at 550×400).
const FEATURED_WIDTH: i32 = 440;
const FEATURED_HEIGHT: i32 = 320;
/// Width of a promotion tile in narrow windows.
const NARROW_FEATURED_WIDTH: i32 = 300;

/// The frame of a page made of sections. It is kept by the page's state,
/// which the page owns, so it only refers to the page weakly.
pub struct Scaffold {
    page: glib::WeakRef<adw::NavigationPage>,
    pub title: adw::WindowTitle,
    pub stack: Rc<PageStack>,
    pub scrolled: gtk::ScrolledWindow,
    /// The page heading (`title-1`, `title-2` when compact).
    pub heading: gtk::Label,
    /// Where the sections go, below the heading.
    pub sections: gtk::Box,
    /// After the sections: the "loading more" spinner or a paging error.
    pub footer: gtk::Box,
}

/// What the skeleton looks like while the first answer is awaited.
#[derive(Clone, Copy)]
pub enum Shape {
    /// Shortcut tiles, then carousels (Home).
    Feed,
    /// Category chips, then carousels (Explore).
    Chips,
    /// Carousels only (editorial pages).
    Carousels,
}

impl Scaffold {
    /// A page titled `title`, and its frame. Section roots (`root`) get the
    /// "Show Sidebar" and narrow main-menu buttons in their header bar.
    pub fn new(
        ctx: &Rc<Ctx>,
        title: &str,
        root: bool,
        shape: Shape,
    ) -> (Self, adw::NavigationPage) {
        let heading = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .wrap(true)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["title-1", "page-content"])
            .accessible_role(gtk::AccessibleRole::Heading)
            .build();
        let sections = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(36)
            .build();
        let footer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Center)
            .build();
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .margin_top(12)
            .margin_bottom(36)
            .build();
        column.append(&heading);
        column.append(&sections);
        column.append(&footer);
        let clamp = adw::Clamp::builder()
            .maximum_size(MAX_WIDTH)
            .tightening_threshold(MAX_WIDTH)
            .child(&column)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let stack = PageStack::new(&skeleton_for(shape));
        stack.set_content(&scrolled);

        let window_title = adw::WindowTitle::new(title, "");
        let header_bar = adw::HeaderBar::builder()
            .title_widget(&window_title)
            .show_title(false)
            .build();
        if root {
            header_bar.pack_start(&sidebar::show_sidebar_button(&ctx.window));
            header_bar.pack_end(&sidebar::narrow_main_menu_button(&ctx.window));
        }
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header_bar);

        let bin = compact_bin(stack.widget(), {
            let heading = heading.downgrade();
            move |is_compact| {
                if let Some(heading) = heading.upgrade() {
                    set_compact(&heading, is_compact);
                }
            }
        });
        set_compact(&heading, false);
        toolbar.set_content(Some(&bin));

        // The header bar shows the title once the heading scrolls away.
        scrolled.vadjustment().connect_value_changed({
            let header_bar = header_bar.downgrade();
            let heading = heading.downgrade();
            move |adjustment| {
                if let (Some(header_bar), Some(heading)) = (header_bar.upgrade(), heading.upgrade())
                {
                    let height = f64::from(heading.height().max(1) + 12);
                    header_bar.set_show_title(adjustment.value() > height);
                }
            }
        });

        let page = adw::NavigationPage::builder()
            .title(title)
            .child(&toolbar)
            .build();
        let scaffold = Self {
            page: page.downgrade(),
            title: window_title,
            stack,
            scrolled,
            heading,
            sections,
            footer,
        };
        (scaffold, page)
    }

    /// Renames the page (heading, header bar and navigation title).
    pub fn set_title(&self, title: &str) {
        self.heading.set_label(title);
        self.title.set_title(title);
        if let Some(page) = self.page.upgrade() {
            page.set_title(title);
        }
    }

    /// Removes every section and the footer.
    pub fn clear(&self) {
        while let Some(child) = self.sections.first_child() {
            self.sections.remove(&child);
        }
        self.set_footer(None);
    }

    /// Shows `widget` after the sections (`None` empties the footer).
    pub fn set_footer(&self, widget: Option<&gtk::Widget>) {
        while let Some(child) = self.footer.first_child() {
            self.footer.remove(&child);
        }
        if let Some(widget) = widget {
            self.footer.append(widget);
        }
    }

    /// Calls `f` while the end of the content is less than a screen away
    /// (and when the content does not fill the screen).
    pub fn connect_near_end(&self, f: impl Fn() + 'static) {
        let f = Rc::new(f);
        let check = move |adjustment: &gtk::Adjustment| {
            let left = adjustment.upper() - adjustment.value() - adjustment.page_size();
            if adjustment.page_size() > 0.0 && left < adjustment.page_size() {
                f();
            }
        };
        let adjustment = self.scrolled.vadjustment();
        adjustment.connect_value_changed(check.clone());
        adjustment.connect_changed(check);
    }

    /// Runs `f` on F5 while the page is shown.
    pub fn connect_reload(&self, f: impl Fn() + 'static) {
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.set_scope(gtk::ShortcutScope::Managed);
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("F5"),
            Some(gtk::CallbackAction::new(move |_, _| {
                f();
                glib::Propagation::Stop
            })),
        ));
        self.scrolled.add_controller(shortcuts);
    }
}

fn set_compact(heading: &gtk::Label, compact: bool) {
    if compact {
        heading.remove_css_class("title-1");
        heading.add_css_class("title-2");
    } else {
        heading.remove_css_class("title-2");
        heading.add_css_class("title-1");
    }
}

/// The spinner shown at the end while more is loading.
pub fn loading_more() -> gtk::Widget {
    let spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .build();
    spinner
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label(tr("Loading more"))]);
    spinner.upcast()
}

/// "Couldn't load more" with a Retry button, after what is already shown.
pub fn paging_error(message: &str, retry: impl Fn() + 'static) -> gtk::Widget {
    let row = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    row.append(
        &gtk::Label::builder()
            .label(message)
            .wrap(true)
            .css_classes(["dimmed"])
            .build(),
    );
    let button = gtk::Button::builder()
        .label(tr("Retry"))
        .valign(gtk::Align::Center)
        .css_classes(["pill"])
        .build();
    button.connect_clicked(move |_| retry());
    row.append(&button);
    row.upcast()
}

/// The widget for `section`, or `None` for sections with nothing to show.
pub fn section_widget(ctx: &Rc<Ctx>, section: &Section) -> Option<gtk::Widget> {
    if section.items.is_empty() {
        return None;
    }
    Some(match section.kind {
        SectionKind::Featured => featured_section(ctx, section),
        SectionKind::Links => links_section(ctx, section),
        _ => section::section_widget(ctx, section),
    })
}

/// Category chips with "View All" when TIDAL has more of them.
fn links_section(ctx: &Rc<Ctx>, section: &Section) -> gtk::Widget {
    let Some(route) = view_all_route(section).filter(|_| section.total > section.items.len())
    else {
        return section::section_widget(ctx, section);
    };
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    let header = title_row(&section.title, section.subtitle.as_deref());
    header.add_css_class("page-content");
    header.append(&view_all_button(ctx, route));
    widget.append(&header);
    let untitled = Section {
        title: String::new(),
        subtitle: None,
        ..section.clone()
    };
    widget.append(&section::section_widget(ctx, &untitled));
    widget.upcast()
}

fn view_all_button(ctx: &Rc<Ctx>, route: super::super::nav::Route) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(tr("View All"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let ctx = Rc::downgrade(ctx);
    button.connect_clicked(move |_| {
        if let Some(ctx) = ctx.upgrade() {
            ctx.navigate(route.clone());
        }
    });
    button
}

/// Promotions: a lone one is a banner across the page (its image beside
/// its title, or above it in narrow windows); several are a row of large
/// landscape tiles that scrolls sideways.
fn featured_section(ctx: &Rc<Ctx>, section: &Section) -> gtk::Widget {
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    if !section.title.is_empty() {
        let title = title_row(&section.title, section.subtitle.as_deref());
        title.add_css_class("page-content");
        widget.append(&title);
    }
    if let [item] = section.items.as_slice() {
        widget.append(&featured_banner(ctx, item));
        return widget.upcast();
    }
    let row = gtk::Box::builder().spacing(12).build();
    for item in section
        .items
        .iter()
        .take(super::super::widgets::carousel::MAX_CARDS)
    {
        row.append(&featured_tile(ctx, item));
    }
    section::align_to_edge(&row, section::CARD_INSET);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&row)
        .css_classes([media_card::CAROUSEL_SCROLLER])
        .build();
    widget.append(&scrolled);
    widget.upcast()
}

/// Title and subtitle of a promotion.
fn featured_text(item: &Item) -> (String, String) {
    match item {
        Item::Link(link) => (
            link.title.clone(),
            link.subtitle.clone().unwrap_or_default(),
        ),
        other => (other_title(other), String::new()),
    }
}

/// The promotion's landscape image (TIDAL serves them at 550×400).
fn featured_picture(ctx: &Rc<Ctx>, item: &Item, width: i32) -> gtk::Picture {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["cover", "large", "cover-placeholder"])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build();
    if let Some(image) = media_card::item_image(item) {
        let weak = picture.downgrade();
        media_card::load_when_shown(ctx, &picture, image, width, move |texture| {
            if let Some(picture) = weak.upgrade() {
                picture.set_paintable(Some(texture));
            }
        });
    }
    picture
}

/// A flat button that opens `item`, described for screen readers.
fn featured_button(
    ctx: &Rc<Ctx>,
    item: &Item,
    content: &gtk::Box,
    classes: &[&str],
) -> gtk::Button {
    let (title, subtitle) = featured_text(item);
    let button = gtk::Button::builder()
        .child(content)
        .valign(gtk::Align::Start)
        .css_classes(classes)
        .build();
    let accessible = if subtitle.is_empty() {
        title
    } else {
        format!("{title}, {subtitle}")
    };
    button.update_property(&[gtk::accessible::Property::Label(&accessible)]);
    let weak = Rc::downgrade(ctx);
    let open = item.clone();
    button.connect_clicked(move |_| {
        if let Some(ctx) = weak.upgrade() {
            activate_item(&ctx, &open);
        }
    });
    button
}

/// One tile of a row of promotions; smaller in narrow windows.
fn featured_tile(ctx: &Rc<Ctx>, item: &Item) -> gtk::Widget {
    let (title, subtitle) = featured_text(item);
    let picture = featured_picture(ctx, item, FEATURED_WIDTH);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    content.append(&picture);
    content.append(&text_label(&title, &["heading"], 1));
    if !subtitle.is_empty() {
        content.append(&text_label(&subtitle, &["caption", "dimmed"], 1));
    }
    let size = |narrow: bool| {
        if narrow {
            (NARROW_FEATURED_WIDTH, NARROW_FEATURED_WIDTH * 8 / 11)
        } else {
            (FEATURED_WIDTH, FEATURED_HEIGHT)
        }
    };
    let apply = {
        let picture = picture.downgrade();
        let content = content.downgrade();
        move |narrow: bool| {
            let (width, height) = size(narrow);
            if let (Some(picture), Some(content)) = (picture.upgrade(), content.upgrade()) {
                picture.set_size_request(width, height);
                content.set_width_request(width);
            }
        }
    };
    apply(false);
    super::super::widgets::follow_narrow(&content, apply);
    featured_button(ctx, item, &content, &["flat", "media-card-button"]).upcast()
}

/// A lone promotion: image and text side by side across the page.
fn featured_banner(ctx: &Rc<Ctx>, item: &Item) -> gtk::Widget {
    let (title, subtitle) = featured_text(item);
    let picture = featured_picture(ctx, item, FEATURED_WIDTH);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    text.append(&text_label(&title, &["title-2"], 2));
    if !subtitle.is_empty() {
        text.append(&text_label(&subtitle, &["dimmed"], 3));
    }
    let content = gtk::Box::builder().spacing(24).build();
    content.append(&picture);
    content.append(&text);
    let apply = {
        let picture = picture.downgrade();
        let content = content.downgrade();
        move |narrow: bool| {
            let (Some(picture), Some(content)) = (picture.upgrade(), content.upgrade()) else {
                return;
            };
            if narrow {
                // Above the text, as wide as the page.
                content.set_orientation(gtk::Orientation::Vertical);
                content.set_spacing(12);
                picture.set_size_request(-1, NARROW_FEATURED_WIDTH * 8 / 11);
                picture.set_hexpand(true);
            } else {
                content.set_orientation(gtk::Orientation::Horizontal);
                content.set_spacing(24);
                picture.set_size_request(FEATURED_WIDTH, FEATURED_HEIGHT);
                picture.set_hexpand(false);
            }
        }
    };
    apply(false);
    super::super::widgets::follow_narrow(&content, apply);
    let button = featured_button(ctx, item, &content, &["flat", "media-card-button"]);
    section::align_to_edge(&button, section::CARD_INSET);
    button.upcast()
}

/// A left-aligned label of at most `lines` lines, ellipsized.
fn text_label(text: &str, classes: &[&str], lines: i32) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(lines > 1)
        .lines(lines)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(1)
        .tooltip_text(text)
        .css_classes(classes)
        .build()
}

fn other_title(item: &Item) -> String {
    match item {
        Item::Track(track) => track.title.clone(),
        Item::Album(album) => album.title.clone(),
        Item::Artist(artist) => artist.name.clone(),
        Item::Playlist(playlist) => playlist.title.clone(),
        Item::Mix(mix) => mix.title.clone(),
        Item::Link(link) => link.title.clone(),
    }
}

/// Ghost tiles, chips and carousels with the shape of the page.
fn skeleton_for(shape: Shape) -> gtk::Widget {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(36)
        .margin_top(12)
        .css_classes(["page-content"])
        .build();
    page.update_property(&[gtk::accessible::Property::Label(tr("Loading"))]);
    page.update_state(&[gtk::accessible::State::Busy(true)]);
    page.append(&bar(220, 32));
    match shape {
        Shape::Feed => {
            let tiles = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            for _ in 0..4 {
                let tile = gtk::Box::builder()
                    .spacing(12)
                    .hexpand(true)
                    .css_classes(["card"])
                    .build();
                tile.append(&bar(56, 56));
                tile.append(&bar(100, 12));
                tiles.append(&tile);
            }
            page.append(&clipped(&tiles));
        }
        Shape::Chips => {
            let chips = adw::WrapBox::builder()
                .child_spacing(6)
                .line_spacing(6)
                .build();
            for width in [80, 60, 110, 96, 70, 120, 64, 90, 76, 104] {
                let chip = bar(width, 34);
                chip.add_css_class("chip");
                chips.append(&chip);
            }
            page.append(&chips);
        }
        Shape::Carousels => {}
    }
    for _ in 0..2 {
        let carousel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        carousel.append(&bar(180, 20));
        let cards = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        for _ in 0..6 {
            cards.append(&skeleton::card(media_card::CARD_SIZE));
        }
        carousel.append(&clipped(&cards));
        page.append(&carousel);
    }
    let clamp = adw::Clamp::builder()
        .maximum_size(MAX_WIDTH)
        .tightening_threshold(MAX_WIDTH)
        .valign(gtk::Align::Start)
        .child(&page)
        .build();
    // Clipped rather than scrolled: it never asks for more room than the
    // page has.
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(&clamp)
        .build()
        .upcast()
}

/// `child` cut at the page's edge instead of widening the page.
fn clipped(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(child)
        .build()
}

fn bar(width: i32, height: i32) -> gtk::Box {
    gtk::Box::builder()
        .width_request(width)
        .height_request(height)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .css_classes(["skeleton"])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build()
}
